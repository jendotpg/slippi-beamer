pub mod check;
pub mod http;
pub mod mdns;
pub mod wifi;

use std::sync::atomic::{AtomicU32, AtomicU8, Ordering};
use std::sync::Arc;
use std::time::Duration;

use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::hal::cpu::Core;
use esp_idf_svc::hal::modem::Modem;
use esp_idf_svc::hal::task::thread::ThreadSpawnConfiguration;
use esp_idf_svc::nvs::EspDefaultNvsPartition;

use crate::config::Outcome;
use crate::storage::SdCard;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum NetResult {
    Pending = 0,
    Ok = 1,
    Offline = 2,
    Fail = 3,
}

static RESULT: AtomicU8 = AtomicU8::new(NetResult::Pending as u8);
static IN_FLIGHT: AtomicU32 = AtomicU32::new(0);
static TRANSFERS: AtomicU32 = AtomicU32::new(0);

pub fn result() -> NetResult {
    match RESULT.load(Ordering::Relaxed) {
        1 => NetResult::Ok,
        2 => NetResult::Offline,
        3 => NetResult::Fail,
        _ => NetResult::Pending,
    }
}

fn set_result(r: NetResult) {
    RESULT.store(r as u8, Ordering::Relaxed);
}

pub fn give_up() {
    set_result(NetResult::Fail);
}

pub fn transfers_in_flight() -> u32 {
    IN_FLIGHT.load(Ordering::Relaxed)
}

pub fn transfers_started() -> u32 {
    TRANSFERS.load(Ordering::Relaxed)
}

pub struct Transfer;

impl Transfer {
    pub fn begin() -> Transfer {
        IN_FLIGHT.fetch_add(1, Ordering::Relaxed);
        TRANSFERS.fetch_add(1, Ordering::Relaxed);
        Transfer
    }
}

impl Drop for Transfer {
    fn drop(&mut self) {
        IN_FLIGHT.fetch_sub(1, Ordering::Relaxed);
    }
}

pub struct Plan {
    pub join: Option<wifi::Join>,
    pub hostname: String,
    pub station: String,
    pub station_name: String,
}

impl Plan {
    pub fn from(outcome: &Outcome, station_id: &str) -> Plan {
        use crate::config::Network;

        match outcome {
            Outcome::Applied(cfg) => Plan {
                join: match cfg.network() {
                    Network::Offline => None,
                    Network::Join {
                        ssid,
                        password,
                        country,
                        hidden,
                    } => Some(wifi::Join {
                        ssid: ssid.as_str().to_owned(),
                        password: password.as_ref().map(|p| p.as_str().to_owned()),
                        country: country.as_str().to_owned(),
                        hidden: *hidden,
                    }),
                },
                hostname: cfg.hostname(station_id),
                station: station_id.to_owned(),
                station_name: cfg.display_name(station_id).to_owned(),
            },
            Outcome::Rejected(_) | Outcome::Unreadable(_) => Plan {
                join: None,
                hostname: format!("beamer-{}", crate::config::hostname_slug(station_id)),
                station: station_id.to_owned(),
                station_name: station_id.to_owned(),
            },
        }
    }
}

pub fn spawn(
    modem: Modem<'static>,
    nvs: EspDefaultNvsPartition,
    sd: Arc<SdCard>,
    plan: Plan,
) -> anyhow::Result<()> {
    ThreadSpawnConfiguration {
        name: Some(c"net"),
        stack_size: 8192,
        priority: 4,
        pin_to_core: Some(Core::Core0),
        ..Default::default()
    }
    .set()?;

    std::thread::Builder::new()
        .stack_size(8192)
        .spawn(move || run(modem, nvs, sd, plan))?;

    ThreadSpawnConfiguration::default().set()?;
    Ok(())
}

fn run(modem: Modem<'static>, nvs: EspDefaultNvsPartition, sd: Arc<SdCard>, plan: Plan) {
    let sysloop = match EspSystemEventLoop::take() {
        Ok(l) => l,
        Err(e) => {
            fail(
                crate::status::ErrorLabel::NoWifi,
                &["the system event loop would not start", &format!("{e}")],
            );
            set_result(NetResult::Fail);
            return;
        }
    };

    if crate::errors::session_has_errors() {
        log::warn!("station is already in the error state: not bringing the network up");
        set_result(NetResult::Fail);
        return;
    }

    let Some(join) = plan.join else {
        log::info!("no SSID configured: this station has no network");
        crate::status::set_net(crate::status::Net::NotSet);
        set_result(NetResult::Offline);
        return;
    };

    let mut radio = match wifi::Radio::up(modem, sysloop, nvs, &plan.hostname, &join) {
        Ok(r) => r,
        Err(()) => {
            set_result(NetResult::Fail);
            return;
        }
    };

    let server = match http::serve(sd) {
        Ok(s) => Some(s),
        Err(e) => {
            fail(
                crate::status::ErrorLabel::NoHttp,
                &["nothing answered on port 80", &format!("{e}")],
            );
            None
        }
    };

    let mdns = match mdns::advertise(&plan.hostname) {
        Ok(m) => Some(m),
        Err(e) => {
            fail(
                crate::status::ErrorLabel::NoMdns,
                &[
                    "this station will not appear in a discovery browse",
                    &format!("{e}"),
                    "the Wii keeps recording replays to the card",
                ],
            );
            None
        }
    };

    let identity = check::Identity {
        station: plan.station.clone(),
        station_name: plan.station_name.clone(),
        host: plan.hostname.clone(),
        ssid: Some(join.ssid.clone()),
        mdns: mdns.is_some(),
    };

    if server.is_some() && mdns.is_some() {
        set_result(NetResult::Ok);
        log::info!(
            "net up: http://{}.local/ -- {}",
            plan.hostname,
            crate::journal::heap_note(),
        );
    } else {
        set_result(NetResult::Fail);
    }

    if let Err(e) = check::spawn(identity) {
        log::warn!("the health task would not start: {e}");
    }

    let mut since_tick = Duration::ZERO;
    loop {
        if crate::errors::session_has_errors() {
            break;
        }
        std::thread::sleep(RED_POLL);

        since_tick += RED_POLL;
        if since_tick >= ASSOCIATION_TICK {
            since_tick = Duration::ZERO;
            radio.tick();
        }
    }

    stand_down(server, mdns, radio);
}

const RED_POLL: Duration = Duration::from_millis(250);

const ASSOCIATION_TICK: Duration = Duration::from_secs(10);

fn stand_down(
    server: Option<esp_idf_svc::http::server::EspHttpServer<'static>>,
    mdns: Option<esp_idf_svc::mdns::EspMdns>,
    radio: wifi::Radio,
) {
    log::warn!("station is red: standing down -- no HTTP, no discovery");

    drop(server);
    drop(mdns);
    drop(radio);

    crate::status::set_net(crate::status::Net::Offline);
    set_result(NetResult::Fail);

    log::warn!("network down; the card is still recording");
}

fn fail(label: crate::status::ErrorLabel, lines: &[&str]) {
    crate::errors::error(crate::errors::Target::Session, label, "net", lines);
}
