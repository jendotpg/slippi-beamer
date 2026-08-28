use std::ffi::CString;
use std::net::Ipv4Addr;
use std::time::Duration;

use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::hal::modem::Modem;
use esp_idf_svc::handle::RawHandle;
use esp_idf_svc::nvs::EspDefaultNvsPartition;
use esp_idf_svc::sys::{
    esp, esp_netif_set_hostname, esp_wifi_set_country_code, esp_wifi_set_ps,
    wifi_ps_type_t_WIFI_PS_NONE,
};
use esp_idf_svc::wifi::{
    AuthMethod, BlockingWifi, ClientConfiguration, Configuration, EspWifi, ScanMethod,
};

use crate::status::{self, Label, Net};

pub struct Join {
    pub ssid: String,
    pub password: Option<String>,
    pub country: String,
    pub hidden: bool,
}

const DHCP_TIMEOUT: Duration = Duration::from_secs(30);

pub struct Radio {
    wifi: BlockingWifi<EspWifi<'static>>,
    ssid: String,
}

impl Radio {
    pub fn up(
        modem: Modem<'static>,
        sysloop: EspSystemEventLoop,
        nvs: EspDefaultNvsPartition,
        hostname: &str,
        join: &Join,
    ) -> Result<Radio, ()> {
        let wifi = EspWifi::new(modem, sysloop.clone(), Some(nvs)).map_err(|e| {
            fail(
                Label::NoWifi,
                &["the WiFi driver would not initialise", &e.to_string()],
            )
        })?;
        let mut wifi = BlockingWifi::wrap(wifi, sysloop).map_err(|e| {
            fail(
                Label::NoWifi,
                &["the WiFi event wrapper would not start", &e.to_string()],
            )
        })?;

        set_hostname(&mut wifi, hostname);

        let auth = match &join.password {
            None => AuthMethod::None,
            Some(_) => AuthMethod::WPA2Personal,
        };
        let conf = ClientConfiguration {
            ssid: truncating(&join.ssid),
            password: truncating(join.password.as_deref().unwrap_or("")),
            auth_method: auth,
            scan_method: if join.hidden {
                ScanMethod::CompleteScan(Default::default())
            } else {
                ScanMethod::FastScan
            },
            ..Default::default()
        };
        wifi.set_configuration(&Configuration::Client(conf))
            .map_err(|e| {
                fail(
                    Label::NoWifi,
                    &["the WiFi configuration was rejected", &e.to_string()],
                )
            })?;

        wifi.start().map_err(|e| {
            fail(
                Label::NoWifi,
                &["the radio would not start", &e.to_string()],
            )
        })?;

        set_country(&join.country);

        no_power_save();

        log::info!(
            "associating with {:?} ({})",
            join.ssid,
            if join.hidden { "hidden" } else { "broadcast" }
        );
        wifi.connect().map_err(|e| {
            fail(
                Label::NoWifi,
                &[
                    "did not associate with the configured SSID",
                    &format!("SSID {:?}: {e}", join.ssid),
                    "check SSID and PASSWORD in CONFIG/config.txt, and that the AP is 2.4 GHz",
                ],
            )
        })?;

        if let Some(actual) = associated_ssid() {
            if actual != join.ssid {
                fail(
                    Label::WrongWifi,
                    &[
                        "associated with a different network than config.txt asks for",
                        &format!("asked for {:?}, joined {actual:?}", join.ssid),
                    ],
                );
                return Err(());
            }
        }

        wifi.ip_wait_while(|| wifi.is_up().map(|up| !up), Some(DHCP_TIMEOUT))
            .map_err(|e| {
                fail(
                    Label::NoIp,
                    &[
                        "associated, but the network handed out no address",
                        &format!("no DHCP lease after {}s: {e}", DHCP_TIMEOUT.as_secs()),
                    ],
                )
            })?;

        let ip = current_ip(&wifi).ok_or_else(|| {
            fail(
                Label::NoIp,
                &["associated, but the interface reports no address"],
            )
        })?;

        log::info!(
            "associated with {:?}, address {ip}, hostname {hostname}",
            join.ssid
        );
        status::set_net(Net::Up(ip));

        Ok(Radio {
            wifi,
            ssid: join.ssid.clone(),
        })
    }

    pub fn tick(&mut self) {
        let connected = self.wifi.is_connected().unwrap_or(false);
        let up = self.wifi.is_up().unwrap_or(false);
        if connected && up {
            if let Some(ip) = current_ip(&self.wifi) {
                status::set_net(Net::Up(ip));
            }
            return;
        }

        log::warn!(
            "lost {:?} (connected {connected}, up {up}); reconnecting",
            self.ssid
        );
        status::set_net(Net::Offline);
        if let Err(e) = self.wifi.connect() {
            log::warn!("reconnect failed: {e}");
            return;
        }
        if let Err(e) = self
            .wifi
            .ip_wait_while(|| self.wifi.is_up().map(|up| !up), Some(DHCP_TIMEOUT))
        {
            log::warn!("reconnected, but no address: {e}");
            return;
        }
        if let Some(ip) = current_ip(&self.wifi) {
            log::info!("back on {:?}, address {ip}", self.ssid);
            status::set_net(Net::Up(ip));
        }
    }
}

fn current_ip(wifi: &BlockingWifi<EspWifi<'static>>) -> Option<Ipv4Addr> {
    let info = wifi.wifi().sta_netif().get_ip_info().ok()?;
    let ip = info.ip;
    if ip == Ipv4Addr::UNSPECIFIED {
        None
    } else {
        Some(ip)
    }
}

fn associated_ssid() -> Option<String> {
    let mut ap = esp_idf_svc::sys::wifi_ap_record_t::default();

    if unsafe { esp_idf_svc::sys::esp_wifi_sta_get_ap_info(&mut ap) } != esp_idf_svc::sys::ESP_OK {
        return None;
    }

    let end = ap
        .ssid
        .iter()
        .position(|&b| b == 0)
        .unwrap_or(ap.ssid.len());
    Some(String::from_utf8_lossy(&ap.ssid[..end]).into_owned())
}

fn set_hostname(wifi: &mut BlockingWifi<EspWifi<'static>>, hostname: &str) {
    let Ok(c) = CString::new(hostname) else {
        log::warn!("hostname {hostname:?} has an interior NUL; leaving the default");
        return;
    };
    let handle = wifi.wifi().sta_netif().handle();

    if let Err(e) = esp!(unsafe { esp_netif_set_hostname(handle, c.as_ptr()) }) {
        log::warn!("could not set hostname {hostname:?}: {e}");
    }
}

fn no_power_save() {
    if let Err(e) = esp!(unsafe { esp_wifi_set_ps(wifi_ps_type_t_WIFI_PS_NONE) }) {
        log::warn!("could not disable WiFi power save: {e}");
    }
}

fn set_country(country: &str) {
    let Ok(c) = CString::new(country) else { return };

    if let Err(e) = esp!(unsafe { esp_wifi_set_country_code(c.as_ptr(), true) }) {
        log::warn!("could not set country {country:?}: {e}");
    }
}

fn truncating<const N: usize>(s: &str) -> heapless::String<N> {
    let mut out = heapless::String::new();
    for ch in s.chars() {
        if out.push(ch).is_err() {
            log::warn!("truncated a {N}-byte WiFi field");
            break;
        }
    }
    out
}

fn fail(label: Label, lines: &[&str]) {
    crate::errors::error(crate::errors::Target::Session, label, "net", lines);
}
