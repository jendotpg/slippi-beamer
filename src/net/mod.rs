pub mod check;
pub mod gz;
pub mod http;
pub mod mdns;
pub mod wifi;

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
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
static SHUTDOWN: AtomicBool = AtomicBool::new(false);
static DOWN: AtomicBool = AtomicBool::new(false);

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
    down(NetResult::Fail);
}

pub fn shut_down(timeout: Duration) -> bool {
    SHUTDOWN.store(true, Ordering::Relaxed);

    let start = std::time::Instant::now();
    while !DOWN.load(Ordering::Relaxed) {
        if start.elapsed() >= timeout {
            return false;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    true
}

pub fn transfers_in_flight() -> u32 {
    IN_FLIGHT.load(Ordering::Relaxed)
}

pub fn transfers_started() -> u32 {
    TRANSFERS.load(Ordering::Relaxed)
}

pub const CONN_HEAP: u32 = 8 * 1_536;
pub const HEAP_FLOOR: u32 = CONN_HEAP + 8 * 1024;

pub fn heap_too_low() -> Option<u32> {
    let (free, _) = crate::journal::heap_now();
    (free < HEAP_FLOOR).then_some(free)
}

pub fn oom_count() -> u32 {
    unsafe { esp_idf_svc::sys::beamer_oom_count() }
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

#[derive(Debug, Clone, PartialEq, Eq)]
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

static PENDING: Mutex<Option<Plan>> = Mutex::new(None);

pub fn reconfigure(plan: Plan) {
    *PENDING.lock().unwrap_or_else(|e| e.into_inner()) = Some(plan);
}

fn take_pending() -> Option<Plan> {
    PENDING.lock().unwrap_or_else(|e| e.into_inner()).take()
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
            down(NetResult::Fail);
            return;
        }
    };

    if crate::errors::session_has_errors() {
        log::warn!("station is already in the error state: not bringing the network up");
        down(NetResult::Fail);
        return;
    }

    let Some(join) = plan.join else {
        log::info!("no SSID configured: this station has no network");
        crate::status::set_net(crate::status::Net::NotSet);
        down(NetResult::Offline);
        return;
    };

    let mut radio = match wifi::Radio::up(modem, sysloop, nvs, &plan.hostname, &join) {
        Ok(r) => r,
        Err(()) => {
            down(NetResult::Fail);
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

    let mut mdns = None;

    check::set(check::Identity {
        station: plan.station.clone(),
        station_name: plan.station_name.clone(),
        ssid: Some(join.ssid.clone()),
    });

    if server.is_none() {
        set_result(NetResult::Fail);
    }

    let mut since_tick = Duration::ZERO;
    let mut hostname = plan.hostname;
    let ejected = loop {
        if SHUTDOWN.load(Ordering::Relaxed) {
            break true;
        }
        if crate::errors::session_has_errors() {
            break false;
        }

        if let Some(next) = take_pending() {
            let Some(join) = next.join else {
                log::warn!("net: a pending plan has no SSID; ignoring it");
                continue;
            };

            if next.hostname != hostname {
                drop(mdns.take());
                hostname = next.hostname;
            }

            if radio.rejoin(&hostname, &join).is_err() {
                break false;
            }

            if mdns.is_none() {
                mdns = mdns::advertise(&hostname)
                    .map_err(|e| log::warn!("mdns did not advertise, retrying: {e}"))
                    .ok();
            }

            check::set(check::Identity {
                station: next.station,
                station_name: next.station_name.clone(),
                ssid: Some(join.ssid),
            });
            crate::status::set_name(&next.station_name);
            since_tick = Duration::ZERO;
            log::info!("net: re-applied config -- http://{hostname}.local/");
            continue;
        }

        if mdns.is_none() && server.is_some() && radio.associated() {
            mdns = match mdns::advertise(&hostname) {
                Ok(m) => {
                    set_result(NetResult::Ok);
                    log::info!(
                        "net up: http://{hostname}.local/ -- {}",
                        crate::journal::heap_note(),
                    );
                    Some(m)
                }
                Err(e) => {
                    log::warn!("mdns did not advertise, retrying: {e}");
                    None
                }
            };
        }

        std::thread::sleep(RED_POLL);

        since_tick += RED_POLL;
        if since_tick >= wifi::tick_interval() {
            since_tick = Duration::ZERO;
            radio.tick();

            crate::warnings::set(
                crate::status::WarningLabel::LowMemory,
                heap_too_low().is_some(),
            );

            let failed = oom_count();
            if failed > 0 {
                fail(
                    crate::status::ErrorLabel::OutOfMemory,
                    &[
                        "the heap ran out and packets were dropped",
                        &format!("{failed} allocation(s) failed"),
                        "the station stops answering when this happens",
                    ],
                );
                break false;
            }
        }
    };

    stand_down(server, mdns, radio, ejected);
}

const RED_POLL: Duration = Duration::from_millis(250);

fn stand_down(
    server: Option<esp_idf_svc::http::server::EspHttpServer<'static>>,
    mdns: Option<esp_idf_svc::mdns::EspMdns>,
    radio: wifi::Radio,
    ejected: bool,
) {
    if ejected {
        log::info!("ejected: standing down -- no HTTP, no discovery, no radio");
    } else {
        log::warn!("station is red: standing down -- no HTTP, no discovery");
    }

    drop(server);
    drop(mdns);
    drop(radio);

    crate::status::set_net(crate::status::Net::Offline);

    if ejected {
        down(NetResult::Offline);
        log::info!("network down");
    } else {
        down(NetResult::Fail);
        log::warn!("network down; the card is still recording");
    }
}

fn down(r: NetResult) {
    set_result(r);
    DOWN.store(true, Ordering::Relaxed);
}

fn fail(label: crate::status::ErrorLabel, lines: &[&str]) {
    crate::errors::error(crate::errors::Target::Late, label, "net", lines);
}
