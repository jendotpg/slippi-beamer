pub mod font;
pub mod labels;
pub mod lcd;
pub mod led;

use std::net::Ipv4Addr;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU8, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use esp_idf_svc::hal::cpu::Core;
use esp_idf_svc::hal::gpio::{
    Gpio0, Gpio1, Gpio2, Gpio3, Gpio38, Gpio39, Gpio4, Gpio40, Gpio5, Input, PinDriver, Pull,
};
use esp_idf_svc::hal::spi::{SPI2, SPI3};
use esp_idf_svc::hal::task::thread::ThreadSpawnConfiguration;

pub use labels::{ErrorLabel, WarningLabel};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum State {
    Booting = 0,
    Idle = 1,
    Error = 2,
    Off = 3,
    Busy = 4,
    Warning = 5,
}

impl State {
    fn from_u8(v: u8) -> State {
        match v {
            1 => State::Idle,
            2 => State::Error,
            3 => State::Off,
            4 => State::Busy,
            5 => State::Warning,
            _ => State::Booting,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Net {
    #[default]
    Offline,
    NotSet,
    Up(Ipv4Addr),
}

#[derive(Debug, Clone, Default)]
pub struct Detail {
    pub name: String,
    pub writing: bool, // to sd card
    pub sending: bool, // over wifi
    pub net: Net,
    pub files: Option<(u32, u32)>, // replays on the card, and the file cap
    pub label: Option<ErrorLabel>, // of the FIRST error
    pub head: String,              // of the FIRST error
    pub more: u32,
    pub warn: Option<WarningLabel>, // the most severe warning standing
    pub warn_more: u32,
}

static STATE: AtomicU8 = AtomicU8::new(State::Booting as u8);
static PAINTED: AtomicU8 = AtomicU8::new(State::Booting as u8);
static LED_GLOBAL: AtomicU8 = AtomicU8::new(crate::config::LedBrightness::DEFAULT.global());
static DETAIL: Mutex<Option<Detail>> = Mutex::new(None);
static GEN: AtomicU32 = AtomicU32::new(0);
static FLIPPED: AtomicBool = AtomicBool::new(false);

fn detail() -> MutexGuard<'static, Option<Detail>> {
    DETAIL.lock().unwrap_or_else(|e| e.into_inner())
}

fn publish(f: impl FnOnce(&mut Detail)) {
    f(detail().get_or_insert_with(Detail::default));
    GEN.fetch_add(1, Ordering::Release);
}

pub fn set(state: State) {
    STATE.store(state as u8, Ordering::Relaxed);
}

pub fn get() -> State {
    State::from_u8(STATE.load(Ordering::Relaxed))
}

pub fn painted() -> State {
    State::from_u8(PAINTED.load(Ordering::Relaxed))
}

pub fn set_brightness(global: u8) {
    LED_GLOBAL.store(global, Ordering::Relaxed);
}

pub fn set_flipped(flipped: bool) {
    FLIPPED.store(flipped, Ordering::Relaxed);
}

pub fn toggle_flipped() {
    FLIPPED.fetch_xor(true, Ordering::Relaxed);
}

pub fn set_name(name: &str) {
    publish(|d| {
        d.name.clear();
        d.name.push_str(name);
    });
}

pub fn set_net(net: Net) {
    publish(|d| d.net = net);
}

pub fn set_activity(writing: bool, sending: bool) {
    publish(|d| {
        d.writing = writing;
        d.sending = sending;
    });
}

pub fn set_files(files: u32, cap: u32) {
    publish(|d| d.files = Some((files, cap)));
}

pub(crate) fn set_error(label: ErrorLabel, head: &str, more: u32) {
    publish(|d| {
        if d.label.is_none() {
            d.label = Some(label);
            d.head.clear();
            d.head.push_str(head);
        }
        d.more = more;
    });
}

pub(crate) fn set_warning(warn: Option<WarningLabel>, more: u32) {
    publish(|d| {
        d.warn = warn;
        d.warn_more = more;
    });
}

const TICK: Duration = Duration::from_millis(20);
const BOOT_HALF_MS: u64 = 500; // ~1 Hz
const ERROR_HALF_MS: u64 = 100; // ~5 Hz
const DEBOUNCE_TICKS: u8 = 3; // 60 ms at TICK

pub const SPINNER_STEPS: u64 = 12;

const DOTS_MS: u64 = 330;
pub const DOTS_MAX: u64 = 3;

fn led_bright(state: State, ms: u64) -> bool {
    match state {
        State::Booting | State::Warning => (ms / BOOT_HALF_MS).is_multiple_of(2),
        State::Error => (ms / ERROR_HALF_MS).is_multiple_of(2),
        // Solid, in their own colours. `Off` never reads this.
        State::Idle | State::Busy | State::Off => true,
    }
}

fn spinner_frame(ms: u64) -> u64 {
    (ms % (BOOT_HALF_MS * 2)) * SPINNER_STEPS / (BOOT_HALF_MS * 2)
}

fn dots_frame(ms: u64) -> u64 {
    (ms / DOTS_MS) % DOTS_MAX + 1
}

struct Button<'d> {
    pin: PinDriver<'d, Input>,
    level: bool,
    pending: u8,
}

impl<'d> Button<'d> {
    fn new(pin: Gpio0<'static>) -> Result<Button<'d>, esp_idf_svc::sys::EspError> {
        let pin: PinDriver<'_, Input> = PinDriver::input(pin, Pull::Up)?;
        let level = pin.is_high();
        Ok(Button {
            pin,
            level,
            pending: 0,
        })
    }

    fn pressed(&mut self) -> bool {
        let now = self.pin.is_high();
        if now == self.level {
            self.pending = 0;
            return false;
        }
        self.pending += 1;
        if self.pending < DEBOUNCE_TICKS {
            return false;
        }
        self.pending = 0;
        self.level = now;
        !now
    }
}

pub struct Pins {
    pub led: LedPins,
    pub lcd: LcdPins,
    pub button: Gpio0<'static>,
}

pub struct LedPins {
    pub spi: SPI3<'static>,
    pub clk: Gpio39<'static>,
    pub data: Gpio40<'static>,
}

pub struct LcdPins {
    pub spi: SPI2<'static>,
    pub sclk: Gpio5<'static>,
    pub mosi: Gpio3<'static>,
    pub cs: Gpio4<'static>,
    pub dc: Gpio2<'static>,
    pub rst: Gpio1<'static>,
    pub bl: Gpio38<'static>,
}

pub fn spawn(pins: Pins) -> anyhow::Result<()> {
    ThreadSpawnConfiguration {
        name: Some(c"status"),
        stack_size: 4096,
        priority: 3,
        pin_to_core: Some(Core::Core0),
        ..Default::default()
    }
    .set()?;

    std::thread::Builder::new()
        .stack_size(4096)
        .spawn(move || render(pins))?;

    ThreadSpawnConfiguration::default().set()?;
    Ok(())
}

fn render(pins: Pins) {
    let mut led = match led::Led::new(pins.led.spi, pins.led.clk, pins.led.data) {
        Ok(led) => Some(led),
        Err(e) => {
            log::error!("LED unavailable: {e}");
            None
        }
    };

    if let Some(led) = led.as_mut() {
        led.render(State::Booting, true, LED_GLOBAL.load(Ordering::Relaxed));
    }

    let mut panel = match lcd::Lcd::new(pins.lcd) {
        Ok(lcd) => {
            log::info!("panel up");
            Some(lcd)
        }
        Err(e) => {
            log::warn!("panel unavailable, LED only: {e}");
            None
        }
    };

    let mut button = match Button::new(pins.button) {
        Ok(button) => Some(button),
        Err(e) => {
            log::warn!("button unavailable, screen cannot be flipped by hand: {e}");
            None
        }
    };

    let start = Instant::now();
    let mut local = Detail::default();
    let mut applied_flip = false;
    let mut last_state: Option<State> = None;
    let mut last_gen = u32::MAX;
    let mut last_frame = u64::MAX;

    loop {
        let ms = start.elapsed().as_millis() as u64;
        let state = get();
        let gen = GEN.load(Ordering::Acquire);

        if let Some(led) = led.as_mut() {
            led.render(
                state,
                led_bright(state, ms),
                LED_GLOBAL.load(Ordering::Relaxed),
            );
        }

        if button.as_mut().is_some_and(Button::pressed) {
            toggle_flipped();
        }

        if let Some(lcd) = panel.as_mut() {
            let want_flip = FLIPPED.load(Ordering::Relaxed);
            let turned = want_flip != applied_flip;
            if turned {
                log::info!("screen {}", if want_flip { "flipped" } else { "upright" });
                lcd.set_flipped(want_flip);
                applied_flip = want_flip;
            }

            if Some(state) != last_state || gen != last_gen || turned {
                if let Some(d) = detail().as_ref() {
                    local.clone_from(d);
                }
                log::info!("paint {state:?} (gen {gen}, name {:?})", local.name);
                lcd.paint(state, &local);
                last_frame = u64::MAX; // the animation owes a fresh frame
            }
            match state {
                State::Booting => {
                    let frame = spinner_frame(ms);
                    if frame != last_frame {
                        lcd.spinner(frame);
                        last_frame = frame;
                    }
                }
                State::Busy => {
                    let frame = dots_frame(ms);
                    if frame != last_frame {
                        lcd.dots(&local, frame);
                        last_frame = frame;
                    }
                }
                _ => {}
            }
        }

        last_state = Some(state);
        last_gen = gen;
        PAINTED.store(state as u8, Ordering::Relaxed);
        std::thread::sleep(TICK);
    }
}
