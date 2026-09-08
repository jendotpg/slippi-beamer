use std::ffi::CString;
use std::net::Ipv4Addr;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::{Duration, Instant};

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

use crate::status::{self, ErrorLabel, Net};
use crate::warnings::{self, WarningLabel};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Join {
    pub ssid: String,
    pub password: Option<String>,
    pub country: String,
    pub hidden: bool,
}

const DHCP_TIMEOUT: Duration = Duration::from_secs(30);

const BACKOFF_MIN: Duration = Duration::from_secs(3);
const BACKOFF_MAX: Duration = Duration::from_secs(30);

pub struct Radio {
    wifi: BlockingWifi<EspWifi<'static>>,
    ssid: String,
    backoff: Duration,
    next_attempt: Instant,
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
                ErrorLabel::NoWifi,
                &["the WiFi driver would not initialise", &e.to_string()],
            )
        })?;
        let wifi = BlockingWifi::wrap(wifi, sysloop).map_err(|e| {
            fail(
                ErrorLabel::NoWifi,
                &["the WiFi event wrapper would not start", &e.to_string()],
            )
        })?;

        let mut radio = Radio {
            wifi,
            ssid: String::new(),
            backoff: BACKOFF_MIN,
            next_attempt: Instant::now(),
        };

        let _ = radio.associate(hostname, join);
        Ok(radio)
    }

    pub fn associated(&self) -> bool {
        self.wifi.is_connected().unwrap_or(false) && self.wifi.is_up().unwrap_or(false)
    }

    pub fn rejoin(&mut self, hostname: &str, join: &Join) -> Result<(), ()> {
        log::info!("re-joining: {:?} -> {:?}", self.ssid, join.ssid);
        status::set_net(Net::Offline);

        if let Err(e) = self.wifi.disconnect() {
            log::warn!("disconnect before re-join: {e}");
        }
        if let Err(e) = self.wifi.stop() {
            log::warn!("stop before re-join: {e}");
        }

        self.reset_backoff();
        let _ = self.associate(hostname, join);
        Ok(())
    }

    fn reset_backoff(&mut self) {
        self.backoff = BACKOFF_MIN;
        self.next_attempt = Instant::now();
    }

    fn defer_retry(&mut self) {
        warnings::set(WarningLabel::WifiNotAssociated, true);

        self.next_attempt = Instant::now() + self.backoff;
        log::warn!(
            "not associated with {:?}; next attempt in {}s",
            self.ssid,
            self.backoff.as_secs()
        );
        self.backoff = (self.backoff * 2).min(BACKOFF_MAX);
    }

    fn associate(&mut self, hostname: &str, join: &Join) -> Result<(), ()> {
        self.ssid = join.ssid.clone();
        set_hostname(&mut self.wifi, hostname);

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
        self.wifi
            .set_configuration(&Configuration::Client(conf))
            .map_err(|e| {
                fail(
                    ErrorLabel::NoWifi,
                    &["the WiFi configuration was rejected", &e.to_string()],
                )
            })?;

        self.wifi.start().map_err(|e| {
            fail(
                ErrorLabel::NoWifi,
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
        if let Err(e) = self.wifi.connect() {
            log::warn!("could not associate with {:?}: {e}", join.ssid);
            self.defer_retry();
            return Err(());
        }

        if let Some(actual) = associated_ssid() {
            if actual != join.ssid {
                log::warn!("asked for SSID {:?}, joined {actual:?}", join.ssid);
            }
        }

        let wifi = &self.wifi;
        if let Err(e) = wifi.ip_wait_while(|| wifi.is_up().map(|up| !up), Some(DHCP_TIMEOUT)) {
            log::warn!("no DHCP lease after {}s: {e}", DHCP_TIMEOUT.as_secs());
            warnings::set(WarningLabel::WifiNoDHCPLease, true);
            self.defer_retry();
            return Err(());
        }

        let Some(ip) = current_ip(&self.wifi) else {
            log::warn!("associated, but the interface reports no address");
            warnings::set(WarningLabel::WifiNoDHCPLease, true);
            self.defer_retry();
            return Err(());
        };

        log::info!(
            "associated with {:?}, address {ip}, hostname {hostname}",
            join.ssid
        );
        status::set_net(Net::Up(ip));
        self.reset_backoff();
        clear_wifi_warnings();
        Ok(())
    }

    pub fn tick(&mut self) {
        let connected = self.wifi.is_connected().unwrap_or(false);
        let up = self.wifi.is_up().unwrap_or(false);
        if connected && up {
            sample_link();
            if let Some(ip) = current_ip(&self.wifi) {
                status::set_net(Net::Up(ip));
                self.reset_backoff();
                clear_wifi_warnings();
            }
            return;
        }

        status::set_net(Net::Offline);
        TICK_FAST.store(true, Ordering::Relaxed);

        if Instant::now() < self.next_attempt {
            return;
        }

        log::warn!(
            "lost {:?} (connected {connected}, up {up}); reconnecting",
            self.ssid
        );

        if let Err(e) = self.wifi.wifi_mut().connect() {
            log::warn!("reconnect failed: {e}");
        }
        self.defer_retry();
    }
}

fn clear_wifi_warnings() {
    warnings::set(WarningLabel::WifiNotAssociated, false);
    warnings::set(WarningLabel::WifiNoDHCPLease, false);
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

#[derive(Debug, Clone, Copy)]
pub struct Link {
    pub rssi: i32,
    pub phy: &'static str,
    pub channel: u8,
}

static LINK: std::sync::Mutex<Option<Link>> = std::sync::Mutex::new(None);

pub fn link() -> Option<Link> {
    *LINK.lock().unwrap_or_else(|e| e.into_inner())
}

fn phy_name(mode: esp_idf_svc::sys::wifi_phy_mode_t) -> &'static str {
    use esp_idf_svc::sys::*;
    #[allow(non_upper_case_globals)]
    match mode {
        wifi_phy_mode_t_WIFI_PHY_MODE_LR => "LR",
        wifi_phy_mode_t_WIFI_PHY_MODE_11B => "11B",
        wifi_phy_mode_t_WIFI_PHY_MODE_11G => "11G",
        wifi_phy_mode_t_WIFI_PHY_MODE_11A => "11A",
        wifi_phy_mode_t_WIFI_PHY_MODE_HT20 => "HT20",
        wifi_phy_mode_t_WIFI_PHY_MODE_HT40 => "HT40",
        wifi_phy_mode_t_WIFI_PHY_MODE_HE20 => "HE20",
        wifi_phy_mode_t_WIFI_PHY_MODE_VHT20 => "VHT20",
        _ => "unknown",
    }
}

fn sample_link() {
    use esp_idf_svc::sys::*;

    let mut rssi: core::ffi::c_int = 0;
    let rssi = if unsafe { esp_wifi_sta_get_rssi(&mut rssi) } == ESP_OK {
        rssi
    } else {
        update_weak_link(None);
        return;
    };

    let mut mode: wifi_phy_mode_t = 0;
    let phy = if unsafe { esp_wifi_sta_get_negotiated_phymode(&mut mode) } == ESP_OK {
        phy_name(mode)
    } else {
        "unknown"
    };

    let mut ap = wifi_ap_record_t::default();
    let channel = if unsafe { esp_wifi_sta_get_ap_info(&mut ap) } == ESP_OK {
        ap.primary
    } else {
        0
    };

    *LINK.lock().unwrap_or_else(|e| e.into_inner()) = Some(Link { rssi, phy, channel });

    TICK_FAST.store(rssi < WEAK_ON, Ordering::Relaxed);
    update_weak_link(Some(rssi));
}

static TICK_FAST: AtomicBool = AtomicBool::new(false);
pub fn tick_interval() -> Duration {
    if TICK_FAST.load(Ordering::Relaxed) {
        FAST_TICK
    } else {
        ASSOCIATION_TICK
    }
}

pub const ASSOCIATION_TICK: Duration = Duration::from_secs(10);
pub const FAST_TICK: Duration = Duration::from_secs(3);

const WEAK_ON: i32 = -70;
const WEAK_OFF: i32 = -65;
const WEAK_RUN: u32 = 3;

fn update_weak_link(rssi: Option<i32>) {
    static WEAK: AtomicBool = AtomicBool::new(false);
    static RUN: AtomicU32 = AtomicU32::new(0);

    let Some(rssi) = rssi else {
        RUN.store(0, Ordering::Relaxed);
        return;
    };

    let weak = WEAK.load(Ordering::Relaxed);
    let crossed = if weak {
        rssi > WEAK_OFF
    } else {
        rssi < WEAK_ON
    };

    if !crossed {
        RUN.store(0, Ordering::Relaxed);
        return;
    }
    if RUN.fetch_add(1, Ordering::Relaxed) + 1 < WEAK_RUN {
        return;
    }

    RUN.store(0, Ordering::Relaxed);
    WEAK.store(!weak, Ordering::Relaxed);
    warnings::set(WarningLabel::WeakLink, !weak);
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

fn fail(label: ErrorLabel, lines: &[&str]) {
    crate::errors::error(crate::errors::Target::Late, label, "net", lines);
}
