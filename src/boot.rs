//! Phase ordering and task spawn

use std::io::Write as _;
use std::sync::Arc;
use std::time::Duration;

use esp_idf_svc::hal::peripherals::Peripherals;
use esp_idf_svc::nvs::EspDefaultNvsPartition;

use crate::config::{Outcome, Settings};
use crate::errors::{self, Target};
use crate::journal::{self, Phase};
use crate::station::StationId;
use crate::status::{self, ErrorLabel, LcdPins, LedPins, Pins, State, WarningLabel};
use crate::storage::fat::{WriteWindow, BASE_PATH};
use crate::storage::{self, volume, Partition, SdCard};
use crate::{net, reload, scan, station, warnings};

pub fn run() -> anyhow::Result<()> {
    // --- Set up process --------------------------------------------------
    esp_idf_svc::sys::link_patches();

    journal::spawn_log()?;
    crate::panic::install();

    log::info!(
        "boot {}: reset {}, {}",
        unsafe { esp_idf_svc::sys::beamer_boot_count() },
        reset_reason(),
        journal::heap_note(),
    );

    let p = Peripherals::take()?;
    let pins = p.pins;

    // --- ClaimIdentity ---------------------------------------------------
    status::spawn(Pins {
        led: LedPins {
            spi: p.spi3,
            clk: pins.gpio39,
            data: pins.gpio40,
        },
        button: pins.gpio0,
        lcd: LcdPins {
            spi: p.spi2,
            sclk: pins.gpio5,
            mosi: pins.gpio3,
            cs: pins.gpio4,
            dc: pins.gpio2,
            rst: pins.gpio1,
            bl: pins.gpio38,
        },
    })?;

    let nvs = EspDefaultNvsPartition::take()?;
    errors::init(nvs.clone());

    let id = match station::identity() {
        Ok(id) => id,
        Err(label) => errors::halt(
            label,
            "station",
            &[
                "the factory-programmed MAC is unset or all zeroes",
                "refusing to boot rather than invent an identity that would",
                "change on every reflash without anyone noticing",
            ],
        ),
    };
    log::info!("station {id}");
    status::set_name(&id.to_string());

    report_previous_boot();
    journal::report_previous();
    journal::stamp(unsafe { esp_idf_svc::sys::esp_reset_reason() }, unsafe {
        esp_idf_svc::sys::beamer_boot_count()
    });
    journal::count_reset(unsafe { esp_idf_svc::sys::esp_reset_reason() }, unsafe {
        esp_idf_svc::sys::beamer_boot_count()
    });
    journal::mark(Phase::ClaimIdentity);

    // --- MountCard -------------------------------------------------------
    journal::mark(Phase::MountCard);
    log::info!("probing the card");
    let sd = Arc::new(match SdCard::probe() {
        Ok(sd) => sd,
        Err(e) => {
            let detail = format!("{e}");
            errors::halt(
                ErrorLabel::NoSdCard,
                "sd",
                &["SDMMC would not initialise, or no card responded", &detail],
            )
        }
    });
    log::info!("card {} MB", sd.bytes() / (1024 * 1024));
    check_partition(&sd);

    // --- PrepareForBind --------------------------------------------------
    journal::mark(Phase::PrepareForBind);
    let outcome = write_window(&sd, &id);

    let settings = Settings::from(&outcome);
    status::set_brightness(settings.led_global);
    status::set_flipped(settings.flip_screen);
    if !settings.debug {
        journal::disable();
    }

    // --- BindCard --------------------------------------------------------
    journal::mark(Phase::BindCard);

    if let Err(e) = storage::msc::bind(&sd, &id) {
        let detail = format!("{e}");
        errors::halt(
            ErrorLabel::NoUsb,
            "usb",
            &["the device never enumerated on the host", &detail],
        );
    }
    log::info!(
        "bound in {:.3}s as {}",
        storage::msc::bind_time_s(),
        id.usb_serial()
    );

    // --- StartJournal ----------------------------------------------------
    journal::mark(Phase::StartJournal);
    if settings.debug {
        journal::spawn()?;
    }

    if let Err(e) = storage::fat::register_read_window(&sd) {
        log::error!("the read window would not register: {e}");
    }

    if let Err(e) = scan::spawn(
        sd.clone(),
        id.to_string(),
        settings.num_replays as usize,
        settings.replay_cap,
    ) {
        log::error!("the scan task would not start: {e}");
    }

    // --- EstablishNetworkServices ----------------------------------------
    journal::mark(Phase::EstablishNetworkServices);

    let station_id = id.to_string();
    let plan = net::Plan::from(&outcome, &station_id);
    let mut reloader = reload::Watcher::new(settings, plan.clone());
    if let Err(e) = net::spawn(p.modem, nvs, sd.clone(), plan) {
        errors::error(
            Target::Late,
            ErrorLabel::NoWifi,
            "net",
            &["the network task would not start", &format!("{e}")],
        );
        net::give_up();
    }

    // --- Running ------------------------------------------------------------
    journal::mark(Phase::Running);

    let mut last = State::Booting;
    let mut last_activity = (false, false);
    let mut usb_since = std::time::Instant::now();
    let mut warned_no_host = false;
    let mut reported_bad_read = false;
    let mut writing = Hold::new(WRITE_HOLD);
    let mut sending = Hold::new(SEND_HOLD);
    loop {
        let is_writing = writing.poll(storage::msc::writes_ok(), storage::msc::cache_dirty() > 0)
            || scan::game_live();
        let is_sending = sending.poll(net::transfers_started(), net::transfers_in_flight() > 0);
        let usb_ok = storage::msc::mounted() && storage::msc::reads_ok() > 0;
        let waiting = !usb_ok && storage::msc::media_present();
        if !waiting {
            usb_since = std::time::Instant::now();
        }
        let settled = !waiting || usb_since.elapsed() >= HOST_GRACE;

        let now = if !storage::msc::media_present() {
            State::Off
        } else if errors::session_has_errors() {
            State::Error
        } else if is_writing || is_sending {
            State::Busy
        } else if !settled || (net::result() == net::NetResult::Pending && usb_ok) {
            State::Booting
        } else if warnings::any() {
            State::Warning
        } else if usb_ok {
            State::Idle
        } else {
            State::Booting
        };

        let activity: (bool, bool) = (is_writing, is_sending);
        if activity != last_activity {
            status::set_activity(is_writing, is_sending);
            last_activity = activity;
        }
        if now != last {
            log::info!(
                "host {}, {} mount(s), {} sector(s) read",
                storage::msc::host_state(),
                storage::msc::mounts(),
                storage::msc::reads_ok(),
            );
            status::set(now);
            last = now;
        }

        if !warned_no_host && waiting && usb_since.elapsed() >= HOST_GRACE {
            warned_no_host = true;
            if storage::msc::mounted() {
                log::warn!(
                    "the host configured us but has read nothing: first transfer error 0x{:x}, {} mount(s)",
                    storage::msc::first_err(),
                    storage::msc::mounts(),
                );
            } else {
                log::warn!("nothing has enumerated us in {}s", HOST_GRACE.as_secs());
            }
            warnings::set(WarningLabel::NoHost, true);
        }
        if warned_no_host && !waiting {
            warned_no_host = false;
            warnings::set(WarningLabel::NoHost, false);
        }

        if !reported_bad_read {
            let err = storage::msc::first_err();
            if err != 0 {
                reported_bad_read = true;
                let detail = format!(
                    "first error 0x{err:x} after {} sector(s) read",
                    storage::msc::reads_ok(),
                );
                errors::error(
                    Target::Late,
                    ErrorLabel::SdUnreadable,
                    "sd",
                    &[
                        "the card stopped answering transfers",
                        &detail,
                        "a different card is the first thing to try",
                    ],
                );
            }
        }

        if storage::msc::take_eject() {
            eject(&sd, &id);
            shutdown(&sd, &id);
            last = State::Off;
            last_activity = (false, false);
            status::set_activity(false, false);
        }

        if storage::msc::take_load() {
            log::info!("host reloaded the medium");
        }

        if reloader.poll(&sd, &station_id, is_writing) == reload::Action::Restart {
            restart();
        }

        std::thread::sleep(Duration::from_millis(250));
    }
}

const WRITE_HOLD: Duration = Duration::from_millis(250);
const SEND_HOLD: Duration = Duration::from_millis(500);

struct Hold {
    seen: u32,
    until: Option<std::time::Instant>,
    hold: Duration,
}

impl Hold {
    fn new(hold: Duration) -> Hold {
        Hold {
            seen: 0,
            until: None,
            hold,
        }
    }

    fn poll(&mut self, counter: u32, live: bool) -> bool {
        let now = std::time::Instant::now();

        if counter != self.seen || live {
            self.seen = counter;
            self.until = Some(now + self.hold);
        }

        match self.until {
            Some(until) if now < until => true,
            _ => {
                self.until = None;
                false
            }
        }
    }
}

fn eject(sd: &SdCard, id: &StationId) {
    log::info!("host ejected: flushing");

    status::set_activity(true, false);
    status::set(State::Busy);

    if let Err(e) = flush_fully() {
        let detail = format!("{} sector(s) still dirty: {e}", storage::msc::cache_dirty());
        errors::error(
            Target::Late,
            ErrorLabel::SdUnreadable,
            "sd",
            &[
                "could not flush the write cache on eject",
                &detail,
                "the medium stays present in case the host reloads it",
            ],
        );
        return;
    }

    journal::persist_now();
    storage::msc::set_media(false);
    mirror_in_window(sd, id);
    storage::msc::invalidate_all();

    status::set(State::Off);
    log::info!("eject complete: safe to unplug");
}

const HOST_GRACE: Duration = Duration::from_secs(10); // time until "NO WII" warning

const EJECT_GRACE: Duration = Duration::from_secs(3);
const FLUSH_TIMEOUT: Duration = Duration::from_secs(30);
const FLUSH_RETRY: Duration = Duration::from_millis(20);

const DRAIN_TIMEOUT: Duration = Duration::from_secs(5);
const NET_DOWN_TIMEOUT: Duration = Duration::from_secs(5);
const DARK_TIMEOUT: Duration = Duration::from_secs(1);
const POLL: Duration = Duration::from_millis(100);

fn shutdown(sd: &SdCard, id: &StationId) {
    if reloaded() {
        log::info!("host reloaded the medium inside the grace window: staying up");
        return;
    }

    drain();

    if !net::shut_down(NET_DOWN_TIMEOUT) {
        log::warn!(
            "the net task did not stand down in {}s; sleeping anyway",
            NET_DOWN_TIMEOUT.as_secs()
        );
    }
    scan::park();

    flush_before_sleep(sd, id);

    wait_dark();

    storage::msc::detach();
    std::thread::sleep(Duration::from_millis(50));

    log::info!("shutdown complete: safe to unplug");
    journal::persist_now();
    deep_sleep();
}

fn mirror_in_window(sd: &SdCard, id: &StationId) {
    match WriteWindow::open(sd) {
        Ok(window) => {
            errors::mirror(BASE_PATH, &id.to_string());
            drop(window);
        }
        Err(e) => log::warn!("could not open a write window to mirror: {e}"),
    }
}

fn flush_before_sleep(sd: &SdCard, id: &StationId) {
    let dirty = storage::msc::cache_dirty();
    if dirty == 0 {
        return;
    }

    log::warn!("{dirty} sector(s) still dirty: retrying before sleep");
    status::set_activity(true, false);
    status::set(State::Busy);

    if let Err(e) = flush_fully() {
        let detail = format!("{} sector(s) lost: {e}", storage::msc::cache_dirty());
        errors::error(
            Target::Late,
            ErrorLabel::SdUnreadable,
            "sd",
            &[
                "slept with sectors the card never took",
                &detail,
                "copy the replays off this card before reusing it",
            ],
        );
        return;
    }

    log::info!("flush: the card took the rest of the cache");
    storage::msc::set_media(false);
    mirror_in_window(sd, id);
    storage::msc::invalidate_all();
    status::set_activity(false, false);
    status::set(State::Off);
}

fn reloaded() -> bool {
    let start = std::time::Instant::now();
    while start.elapsed() < EJECT_GRACE {
        if storage::msc::take_load() {
            return true;
        }
        std::thread::sleep(POLL);
    }
    false
}

fn restart() -> ! {
    log::info!("reload: restarting to pick up the new config");
    status::set(State::Busy);

    drain();
    net::shut_down(NET_DOWN_TIMEOUT);
    scan::park();

    if let Err(e) = flush_fully() {
        log::warn!(
            "could not flush the cache before the restart: {} sector(s) lost: {e}",
            storage::msc::cache_dirty(),
        );
    }
    journal::persist_now();
    storage::msc::detach();
    std::thread::sleep(Duration::from_millis(50));

    unsafe { esp_idf_svc::sys::esp_restart() }
}

fn flush_fully() -> Result<(), esp_idf_svc::sys::EspError> {
    let start = std::time::Instant::now();
    let mut last = storage::msc::cache_dirty();

    loop {
        match storage::msc::flush_all() {
            Ok(()) => return Ok(()),
            Err(e) if start.elapsed() >= FLUSH_TIMEOUT => {
                log::error!("flush: giving up after {}s", FLUSH_TIMEOUT.as_secs());
                return Err(e);
            }
            Err(e) => {
                let dirty = storage::msc::cache_dirty();
                if dirty != last {
                    log::warn!("flush: retrying, {dirty} sector(s) left ({e})");
                    last = dirty;
                }
                std::thread::sleep(FLUSH_RETRY);
            }
        }
    }
}

fn drain() {
    let start = std::time::Instant::now();
    while net::transfers_in_flight() > 0 {
        if start.elapsed() >= DRAIN_TIMEOUT {
            log::warn!(
                "{} transfer(s) still in flight after {}s; cutting them off",
                net::transfers_in_flight(),
                DRAIN_TIMEOUT.as_secs()
            );
            return;
        }
        std::thread::sleep(POLL);
    }
}

fn wait_dark() {
    let start = std::time::Instant::now();
    while status::painted() != State::Off {
        if start.elapsed() >= DARK_TIMEOUT {
            log::warn!("the readout has not gone dark; sleeping anyway");
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

const PIN_BACKLIGHT: i32 = 38;
const PIN_LED_CLK: i32 = 39;
const PIN_LED_DATA: i32 = 40;

#[allow(unreachable_code)]
fn deep_sleep() -> ! {
    use esp_idf_svc::sys::{esp_deep_sleep_start, gpio_deep_sleep_hold_en, gpio_hold_en};

    for pin in [PIN_BACKLIGHT, PIN_LED_CLK, PIN_LED_DATA] {
        unsafe { gpio_hold_en(pin as _) };
    }
    unsafe { gpio_deep_sleep_hold_en() };

    unsafe { esp_deep_sleep_start() };

    loop {
        std::thread::sleep(Duration::from_secs(60));
    }
}

fn reset_reason() -> &'static str {
    journal::reset_name(unsafe { esp_idf_svc::sys::esp_reset_reason() })
}

#[allow(non_upper_case_globals)]
fn report_reset() {
    use esp_idf_svc::sys::*;

    let raw = unsafe { esp_reset_reason() };
    let bad = matches!(
        raw,
        esp_reset_reason_t_ESP_RST_PANIC
            | esp_reset_reason_t_ESP_RST_INT_WDT
            | esp_reset_reason_t_ESP_RST_TASK_WDT
            | esp_reset_reason_t_ESP_RST_WDT
            | esp_reset_reason_t_ESP_RST_BROWNOUT
            | esp_reset_reason_t_ESP_RST_CPU_LOCKUP
            | esp_reset_reason_t_ESP_RST_PWR_GLITCH
    );
    if !bad {
        return;
    }

    let boot = unsafe { beamer_boot_count() };
    let head = format!("the previous boot was ended by {}", reset_reason());
    let detail = format!("this is boot {boot} since power was last removed");
    let hint = match raw {
        esp_reset_reason_t_ESP_RST_BROWNOUT => {
            "the supply sagged; try a powered hub before suspecting the firmware"
        }
        esp_reset_reason_t_ESP_RST_PANIC => "the panic itself is on the line below",
        _ => "something held a core for too long",
    };
    errors::record_previous("reset", &[&head, &detail, hint]);
}

fn report_previous_boot() {
    report_reset();

    let mut buf = [0u8; 512];
    let found = unsafe {
        esp_idf_svc::sys::beamer_panic_take(buf.as_mut_ptr() as *mut core::ffi::c_char, buf.len())
    };
    if !found {
        return;
    }

    let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    let text = String::from_utf8_lossy(&buf[..end]).into_owned();
    let lines: Vec<&str> = text.lines().collect();
    if lines.is_empty() {
        return;
    }
    errors::record_previous("panic", &lines);
}

fn check_partition(sd: &SdCard) {
    match sd.partition() {
        Partition::Ok { sectors, end } => {
            storage::msc::set_visible(end);
            log::info!(
                "replay partition {} MB, within the limit; host sees {} MB",
                sectors / 2048,
                end / 2048,
            );
        }
        Partition::TooBig { sectors } => {
            let detail = format!(
                "the partition is {} MB; a Beamer volume may be at most 4 GiB",
                sectors / 2048,
            );
            errors::error(
                Target::Session,
                ErrorLabel::WrongFormat,
                "sd",
                &[
                    "this card is partitioned larger than a Beamer supports",
                    &detail,
                    "repartition it to 4 GB or smaller; see the README",
                ],
            );
        }
        Partition::Missing => {
            errors::error(
                Target::Session,
                ErrorLabel::WrongFormat,
                "sd",
                &[
                    "no FAT32 partition on this card",
                    "exFAT, GPT and unpartitioned cards all look like this",
                    "format it MS-DOS (FAT32), 4 GB or smaller; see the README",
                ],
            );
        }
        Partition::Unreadable(e) => {
            let detail = format!("{e}");
            errors::error(
                Target::Session,
                ErrorLabel::SdUnreadable,
                "sd",
                &["could not read the partition table", &detail],
            );
        }
    }
}

fn write_window(sd: &SdCard, id: &StationId) -> Outcome {
    let window = match WriteWindow::open(sd) {
        Ok(w) => w,
        Err(e) => {
            let detail = format!("{e}");
            errors::error(
                Target::Late,
                ErrorLabel::SdUnreadable,
                "sd",
                &[
                    "a card is present but its filesystem will not mount",
                    &detail,
                ],
            );
            return Outcome::unreadable(detail);
        }
    };

    log::info!("write window open, volume mounted");

    seed_volume();
    log::info!("volume seeded");

    let path = format!("{BASE_PATH}/CONFIG/config.txt");
    let outcome = reload::load_initial(&path);

    let station_id = id.to_string();

    match &outcome {
        Outcome::Applied(cfg) => {
            log::info!(
                "config: accepted, station-name {:?}, {} replays served, hostname {}",
                cfg.display_name(&station_id),
                cfg.num_replays(),
                cfg.hostname(&station_id),
            );
            log::info!(
                "config: file cap {}, LED {}%, debug {}",
                cfg.replay_cap(),
                cfg.led_brightness().get(),
                cfg.debug(),
            );
            status::set_name(cfg.display_name(&station_id));
        }
        Outcome::Rejected(problems) => {
            log::error!(
                "config: {} problem(s); the file is rejected whole",
                problems.len()
            );
            for e in problems {
                let [head, detail] = e.lines();
                errors::error(
                    Target::Session,
                    ErrorLabel::BadConfig,
                    "config",
                    &[head, detail],
                );
            }
        }
        Outcome::Unreadable(why) => {
            errors::error(
                Target::Session,
                ErrorLabel::NoConfig,
                "config",
                &[
                    "CONFIG/config.txt could not be read",
                    why,
                    "network settings are left as they were",
                ],
            );
        }
    }

    log::info!("mirroring error.txt");
    errors::mirror(BASE_PATH, &station_id);

    if Settings::from(&outcome).debug {
        volume::write_debug(BASE_PATH, &window, &station_id, reset_reason());
    }

    drop(window);
    log::info!("write window closed");

    outcome
}

fn seed_volume() {
    for dir in ["CONFIG", "LOGS", "SLIPPI", ".fseventsd"] {
        let path = format!("{BASE_PATH}/{dir}");
        if !std::path::Path::new(&path).exists() {
            if let Err(e) = std::fs::create_dir(&path) {
                log::warn!("could not create {dir}/: {e}");
            }
        }
    }

    write_if_absent("CONFIG/config.txt", CONFIG_TEMPLATE);

    // tell macOS to leave the volume fucking alone holy fuck. this took so long.
    write_if_absent(".metadata_never_index", "");
    write_if_absent(".fseventsd/no_log", "");
}

fn write_if_absent(rel: &str, body: &str) {
    let path = format!("{BASE_PATH}/{rel}");
    if std::path::Path::new(&path).exists() {
        return;
    }

    let crlf: String = body.replace('\n', "\r\n"); // lots of TOs use windoze...
    match std::fs::File::create(&path).and_then(|mut f| f.write_all(crlf.as_bytes())) {
        Ok(()) => log::info!("wrote {rel}"),
        Err(e) => log::warn!("could not write {rel}: {e}"),
    }
}

const CONFIG_TEMPLATE: &str = "\
# Beamer station configuration.
#
# Fill this in, save, eject this drive, then move the cable back to the Wii.
#
# The Beamer reads this file at every boot and again after every edit  - so
# a station that is already running will pick up an edit on its own, 
# restarting itself if there was an error. DEBUG is the one key read at 
# boot only - there's no way to turn it on without a manual reboot.
#
# Until SSID is filled in, this station has no network. That is expected.
# After a reboot, if LOGS/error.txt exists it says what went wrong. The
# station's live state is the LED, the screen, and http://<station>/status.
#
# SSID / PASSWORD    the network to join. Leave PASSWORD blank for an open
#                    network; otherwise it is 8-63 characters.
# COUNTRY            two-letter regulatory domain: US, CA, JP, GB...
# HIDDEN             true or false - whether the network broadcasts its name.
# STATION-NAME       what to call this station, on the status file and over the
#                    network. Blank means use the station's ID.
# NUM-REPLAYS-SERVED how many of the newest replays this station hands out over
#                    HTTP. 1 to 16.
# REPLAY-CAP         how many replays this station counts on the card before it
#                    stops counting. 1 to 2048.
# LED-BRIGHTNESS     0 to 100 percent. 0 turns the status LED off completely -
#                    the screen then becomes the only readout.
# FLIP-SCREEN        true or false - whether the screen starts rotated 180
#                    degrees, for a Wii the Beamer plugs into upside down. The
#                    button on the side flips it either way at any time.
# DEBUG              true or false - whether to keep a LOGS/debug_N.txt of each
#                    boot. Off by default. These files are never deleted.

SSID=
PASSWORD=
COUNTRY=US
HIDDEN=false
STATION-NAME=
NUM-REPLAYS-SERVED=10
REPLAY-CAP=512
LED-BRIGHTNESS=20
FLIP-SCREEN=false
DEBUG=false
";
