use std::io::ErrorKind;
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use esp_idf_svc::hal::cpu::Core;
use esp_idf_svc::hal::task::thread::ThreadSpawnConfiguration;

use crate::report;
use crate::scan;
use crate::status::{self, Net};

const TICK: Duration = Duration::from_secs(60);
const PROBE_TIMEOUT: Duration = Duration::from_secs(2);

static HEALTH: Mutex<Option<report::Health>> = Mutex::new(None);

fn lock<T>(m: &'static Mutex<T>) -> MutexGuard<'static, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

pub fn health() -> report::Health {
    lock(&HEALTH).clone().unwrap_or_default()
}

pub struct Identity {
    pub station: String,
    pub station_name: String,
    pub host: String,
    pub ssid: Option<String>,
    pub mdns: bool,
}

pub fn spawn(id: Identity) -> anyhow::Result<()> {
    ThreadSpawnConfiguration {
        name: Some(c"health"),
        stack_size: 6144,
        priority: 2,
        pin_to_core: Some(Core::Core0),
        ..Default::default()
    }
    .set()?;

    std::thread::Builder::new()
        .stack_size(6144)
        .spawn(move || run(id))?;

    ThreadSpawnConfiguration::default().set()?;
    Ok(())
}

fn run(id: Identity) {
    loop {
        tick(&id);
        std::thread::sleep(TICK);
    }
}

fn tick(id: &Identity) {
    let net = status::net();
    let result = super::result();

    let ip = match net {
        Net::Up(ip) => Some(ip),
        _ => None,
    };

    let httpd = if ip.is_some() {
        probe_loopback()
    } else {
        false
    };

    let health = report::Health {
        station: id.station.clone(),
        station_name: id.station_name.clone(),
        host: id.host.clone(),
        uptime_s: scan::uptime_s(),
        wifi: id.ssid.clone(),
        network: network_text(id.ssid.as_deref(), result, ip),
        ip,
        httpd,
        sshd: false, // esp32 beamers have no ssh!
        mdns: id.mdns,
    };

    *lock(&HEALTH) = Some(health);
}

fn probe_loopback() -> bool {
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, 80));
    match TcpStream::connect_timeout(&addr, PROBE_TIMEOUT) {
        Ok(_) => true,
        Err(e) => {
            if e.kind() != ErrorKind::TimedOut {
                log::warn!("health: nothing answered on 127.0.0.1:80: {e}");
            } // only logged - server error is thrown in http.rs
            false
        }
    }
}

fn network_text(
    ssid: Option<&str>,
    result: super::NetResult,
    ip: Option<Ipv4Addr>,
) -> Option<String> {
    let ssid = ssid?;
    match result {
        super::NetResult::Pending => None,
        super::NetResult::Offline => None,
        super::NetResult::Ok => Some(format!("associated with {ssid:?}")),
        super::NetResult::Fail if ip.is_some() => Some(format!("associated with {ssid:?}")),
        super::NetResult::Fail => Some(format!("NOT associated with {ssid:?}")),
    }
}
