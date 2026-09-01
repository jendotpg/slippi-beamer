use core::fmt;

pub const KEEP_DEFAULT: u8 = 10;
pub const KEEP_MAX: u8 = 16;
pub const STATION_NAME_MAX: usize = 63;
pub const SSID_MAX_BYTES: usize = 32;
pub const PSK_MIN: usize = 8;
pub const PSK_MAX: usize = 63;
pub const HOSTNAME_SLUG_MAX: usize = 56;
pub const REPLAY_CAP_DEFAULT: u32 = 512;
pub const REPLAY_CAP_MAX: u32 = 2048;
pub const LED_PCT_DEFAULT: u8 = 20;
pub const LED_PCT_MAX: u8 = 100;
pub const DEBUG_DEFAULT: bool = false;
pub const FLIP_SCREEN_DEFAULT: bool = false;

const STRICT_FLAGS: bool = true;

// --- errors ---------------------------------------------------------------
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigError {
    pub summary: String,
    pub detail: &'static str,
}

impl ConfigError {
    fn new(summary: impl Into<String>, detail: &'static str) -> Self {
        ConfigError {
            summary: summary.into(),
            detail,
        }
    }

    pub fn lines(&self) -> [&str; 2] {
        [&self.summary, self.detail]
    }
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}", self.summary, self.detail)
    }
}

impl std::error::Error for ConfigError {}

// --- validated values -----------------------------------------------------
macro_rules! str_newtype {
    ($(#[$m:meta])* $name:ident) => {
        $(#[$m])*
        #[derive(Debug, Clone, PartialEq, Eq)]
        pub struct $name(String);

        impl $name {
            pub fn as_str(&self) -> &str { &self.0 }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str { &self.0 }
        }
    };
}

str_newtype! {
    Ssid
}
str_newtype! {
    Psk
}
str_newtype! {
    Country
}
str_newtype! {
    StationName
}

impl Ssid {
    pub fn new(s: &str) -> Result<Self, ConfigError> {
        let len = s.len();
        if len > SSID_MAX_BYTES {
            return Err(ConfigError::new(
                format!("SSID is {len} bytes; the maximum is {SSID_MAX_BYTES}."),
                "Shorten it in CONFIG/config.txt.",
            ));
        }
        Ok(Ssid(s.to_owned()))
    }
}

impl Psk {
    pub fn new(s: &str) -> Result<Self, ConfigError> {
        let len = s.chars().count();
        if !(PSK_MIN..=PSK_MAX).contains(&len) {
            return Err(ConfigError::new(
                format!("PASSWORD must be {PSK_MIN}-{PSK_MAX} characters (got {len})."),
                "Fix it in CONFIG/config.txt, or leave PASSWORD blank for an open network.",
            ));
        }
        Ok(Psk(s.to_owned()))
    }
}

impl Country {
    pub fn new(s: &str) -> Result<Self, ConfigError> {
        if s.len() != 2 || !s.bytes().all(|b| b.is_ascii_alphabetic()) {
            return Err(ConfigError::new(
                format!("COUNTRY must be two letters (got \"{s}\")."),
                "Use a code like US, CA, JP or GB in CONFIG/config.txt.",
            ));
        }
        Ok(Country(s.to_ascii_uppercase()))
    }
}

impl StationName {
    pub fn new(s: &str) -> Result<Self, ConfigError> {
        let len = s.chars().count();
        if len > STATION_NAME_MAX {
            return Err(ConfigError::new(
                format!("STATION-NAME is {len} characters; the maximum is {STATION_NAME_MAX}."),
                "Shorten it in CONFIG/config.txt.",
            ));
        }
        if s.chars().any(char::is_control) {
            return Err(ConfigError::new(
                "STATION-NAME contains a control character, which cannot be stored.",
                "Remove it in CONFIG/config.txt.",
            ));
        }
        Ok(StationName(s.to_owned()))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReplayCount(u8);

impl ReplayCount {
    pub fn new(raw: &str) -> Result<Self, ConfigError> {
        let bad = || {
            ConfigError::new(
                format!(
                    "NUM-REPLAYS-SERVED must be a whole number from 1 to {KEEP_MAX} (got \"{raw}\")."
                ),
                "Fix it in CONFIG/config.txt.",
            )
        };
        if raw.is_empty() || !raw.bytes().all(|b| b.is_ascii_digit()) {
            return Err(bad());
        }
        match raw.parse::<u32>() {
            Ok(n) if (1..=KEEP_MAX as u32).contains(&n) => Ok(ReplayCount(n as u8)),
            _ => Err(bad()),
        }
    }

    pub fn get(self) -> u8 {
        self.0
    }
}

impl Default for ReplayCount {
    fn default() -> Self {
        ReplayCount(KEEP_DEFAULT)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReplayCap(u32);

impl ReplayCap {
    pub fn new(raw: &str) -> Result<Self, ConfigError> {
        let bad = || {
            ConfigError::new(
                format!(
                    "REPLAY-CAP must be a whole number from 1 to {REPLAY_CAP_MAX} (got \"{raw}\")."
                ),
                "Fix it in CONFIG/config.txt.",
            )
        };
        if raw.is_empty() || !raw.bytes().all(|b| b.is_ascii_digit()) {
            return Err(bad());
        }
        match raw.parse::<u32>() {
            Ok(n) if (1..=REPLAY_CAP_MAX).contains(&n) => Ok(ReplayCap(n)),
            _ => Err(bad()),
        }
    }

    pub fn get(self) -> u32 {
        self.0
    }
}

impl Default for ReplayCap {
    fn default() -> Self {
        ReplayCap(REPLAY_CAP_DEFAULT)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LedBrightness(u8);

impl LedBrightness {
    pub const DEFAULT: LedBrightness = LedBrightness(LED_PCT_DEFAULT);

    pub fn new(raw: &str) -> Result<Self, ConfigError> {
        let bad = || {
            ConfigError::new(
                format!(
                    "LED-BRIGHTNESS must be a whole number from 0 to {LED_PCT_MAX} (got \"{raw}\")."
                ),
                "Fix it in CONFIG/config.txt. 0 turns the LED off entirely.",
            )
        };
        if raw.is_empty() || !raw.bytes().all(|b| b.is_ascii_digit()) {
            return Err(bad());
        }
        match raw.parse::<u32>() {
            Ok(n) if n <= LED_PCT_MAX as u32 => Ok(LedBrightness(n as u8)),
            _ => Err(bad()),
        }
    }

    pub fn get(self) -> u8 {
        self.0
    }

    pub const fn is_off(self) -> bool {
        self.0 == 0
    }

    pub const fn global(self) -> u8 {
        if self.is_off() {
            return 0;
        }
        let scaled = (self.0 as u32 * 31 + 50) / 100;
        if scaled == 0 {
            1
        } else {
            scaled as u8
        }
    }
}

impl Default for LedBrightness {
    fn default() -> Self {
        LedBrightness::DEFAULT
    }
}

// --- the config -----------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Network {
    /// blank SSID is green, not red - otherwise first boot throws errors
    Offline,
    Join {
        ssid: Ssid,
        password: Option<Psk>,
        country: Country,
        hidden: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    network: Network,
    station_name: Option<StationName>,
    num_replays: ReplayCount,
    replay_cap: ReplayCap,
    led_brightness: LedBrightness,
    flip_screen: bool,
    debug: bool,
}

impl Config {
    pub fn network(&self) -> &Network {
        &self.network
    }

    pub fn station_name(&self) -> Option<&StationName> {
        self.station_name.as_ref()
    }

    pub fn num_replays(&self) -> u8 {
        self.num_replays.get()
    }

    pub fn replay_cap(&self) -> u32 {
        self.replay_cap.get()
    }

    pub fn led_brightness(&self) -> LedBrightness {
        self.led_brightness
    }

    pub fn flip_screen(&self) -> bool {
        self.flip_screen
    }

    pub fn debug(&self) -> bool {
        self.debug
    }

    pub fn display_name<'a>(&'a self, station_id: &'a str) -> &'a str {
        self.station_name
            .as_ref()
            .map_or(station_id, StationName::as_str)
    }

    pub fn hostname(&self, station_id: &str) -> String {
        let mut slug = hostname_slug(self.display_name(station_id));
        if slug.is_empty() {
            slug = hostname_slug(station_id);
        }
        format!("beamer-{slug}")
    }

    pub fn parse(src: &str) -> Result<Config, Vec<ConfigError>> {
        let raw = Raw::scan(src);
        let mut errors = Vec::new();

        let station_name = match raw.station_name.as_deref() {
            None | Some("") => None,
            Some(s) => match StationName::new(s) {
                Ok(v) => Some(v),
                Err(e) => {
                    errors.push(e);
                    None
                }
            },
        };

        let num_replays = match raw.num_replays.as_deref() {
            None | Some("") => ReplayCount::default(),
            Some(s) => match ReplayCount::new(s) {
                Ok(v) => v,
                Err(e) => {
                    errors.push(e);
                    ReplayCount::default()
                }
            },
        };

        let replay_cap = match raw.replay_cap.as_deref() {
            None | Some("") => ReplayCap::default(),
            Some(s) => match ReplayCap::new(s) {
                Ok(v) => v,
                Err(e) => {
                    errors.push(e);
                    ReplayCap::default()
                }
            },
        };

        let led_brightness = match raw.led_brightness.as_deref() {
            None | Some("") => LedBrightness::default(),
            Some(s) => match LedBrightness::new(s) {
                Ok(v) => v,
                Err(e) => {
                    errors.push(e);
                    LedBrightness::default()
                }
            },
        };

        let ssid = match raw.ssid.as_deref() {
            None | Some("") => None,
            Some(s) => match Ssid::new(s) {
                Ok(v) => Some(v),
                Err(e) => {
                    errors.push(e);
                    None
                }
            },
        };

        let mut password = None;
        let mut country = None;
        if raw.ssid.as_deref().is_some_and(|s| !s.is_empty()) {
            if let Some(p) = raw.password.as_deref().filter(|p| !p.is_empty()) {
                match Psk::new(p) {
                    Ok(v) => password = Some(v),
                    Err(e) => errors.push(e),
                }
            }
            match Country::new(raw.country.as_deref().unwrap_or("")) {
                Ok(v) => country = Some(v),
                Err(e) => errors.push(e),
            }
        }

        let hidden = match parse_flag("HIDDEN", raw.hidden.as_deref()) {
            Ok(v) => v,
            Err(e) => {
                errors.push(e);
                false
            }
        };

        let flip_screen = match parse_flag("FLIP-SCREEN", raw.flip_screen.as_deref()) {
            Ok(v) => v,
            Err(e) => {
                errors.push(e);
                FLIP_SCREEN_DEFAULT
            }
        };

        let debug = match parse_flag("DEBUG", raw.debug.as_deref()) {
            Ok(v) => v,
            Err(e) => {
                errors.push(e);
                DEBUG_DEFAULT
            }
        };

        if !errors.is_empty() {
            return Err(errors);
        }

        let network = match ssid {
            None => Network::Offline,
            Some(ssid) => Network::Join {
                ssid,
                password,
                country: country.expect("a joined network always has a country"),
                hidden,
            },
        };

        Ok(Config {
            network,
            station_name,
            num_replays,
            replay_cap,
            led_brightness,
            flip_screen,
            debug,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Applied(Config),
    Rejected(Vec<ConfigError>),
    Unreadable(String),
}

impl Outcome {
    pub fn parse(src: &str) -> Outcome {
        match Config::parse(src) {
            Ok(c) => Outcome::Applied(c),
            Err(e) => Outcome::Rejected(e),
        }
    }
    pub fn parse_bytes(src: &[u8]) -> Outcome {
        match core::str::from_utf8(src) {
            Ok(s) => Outcome::parse(s),
            Err(e) => Outcome::Rejected(vec![ConfigError::new(
                format!(
                    "CONFIG/config.txt is not valid text (bad byte at offset {}).",
                    e.valid_up_to()
                ),
                "Save it as plain UTF-8 text and try again.",
            )]),
        }
    }

    pub fn unreadable(reason: impl Into<String>) -> Outcome {
        Outcome::Unreadable(reason.into())
    }
}

// --- scanning -------------------------------------------------------------
#[derive(Debug, Default)]
struct Raw {
    ssid: Option<String>,
    password: Option<String>,
    country: Option<String>,
    hidden: Option<String>,
    station_name: Option<String>,
    num_replays: Option<String>,
    replay_cap: Option<String>,
    led_brightness: Option<String>,
    flip_screen: Option<String>,
    debug: Option<String>,
}

impl Raw {
    fn scan(src: &str) -> Raw {
        let mut raw = Raw::default();
        for line in src.split('\n') {
            let line = line.strip_suffix('\r').unwrap_or(line);
            let line = trim(line);

            // lines that are empty, start with #, or contain no = are skipped
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };

            let value = unquote(trim(value)).to_owned();
            match trim(key).to_ascii_uppercase().as_str() {
                "SSID" => raw.ssid = Some(value),
                "PASSWORD" => raw.password = Some(value),
                "COUNTRY" => raw.country = Some(value),
                "HIDDEN" => raw.hidden = Some(value),
                "STATION-NAME" | "STATION_NAME" => raw.station_name = Some(value),
                "NUM-REPLAYS-SERVED" | "NUM_REPLAYS_SERVED" => raw.num_replays = Some(value),
                "REPLAY-CAP" | "REPLAY_CAP" => raw.replay_cap = Some(value),
                "LED-BRIGHTNESS" | "LED_BRIGHTNESS" => raw.led_brightness = Some(value),
                "FLIP-SCREEN" | "FLIP_SCREEN" => raw.flip_screen = Some(value),
                "DEBUG" => raw.debug = Some(value),
                _ => {}
            }
        }
        raw
    }
}

fn trim(s: &str) -> &str {
    s.trim_matches(|c: char| c.is_ascii_whitespace())
}

fn unquote(s: &str) -> &str {
    let b = s.as_bytes();
    if b.len() >= 2 && (b[0] == b'"' || b[0] == b'\'') && b[b.len() - 1] == b[0] {
        &s[1..s.len() - 1]
    } else {
        s
    }
}

fn parse_flag(key: &str, raw: Option<&str>) -> Result<bool, ConfigError> {
    let Some(v) = raw else { return Ok(false) };
    match v.to_ascii_lowercase().as_str() {
        "true" | "yes" | "1" => Ok(true),
        "false" | "no" | "0" | "" => Ok(false),
        _ if STRICT_FLAGS => Err(ConfigError::new(
            format!("{key} must be true or false (got \"{v}\")."),
            "Fix it in CONFIG/config.txt.",
        )),
        _ => Ok(false),
    }
}

pub fn hostname_slug(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for c in name.chars() {
        let c = c.to_ascii_lowercase();
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            out.push(c);
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    let s = out.trim_matches('-');
    let s = &s[..s.len().min(HOSTNAME_SLUG_MAX)];
    s.trim_end_matches('-').to_owned()
}
