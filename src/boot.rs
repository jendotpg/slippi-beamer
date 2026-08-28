//! Phase ordering and task spawn

use std::io::Write as _;
use std::sync::Arc;
use std::time::Duration;

use esp_idf_svc::hal::peripherals::Peripherals;
use esp_idf_svc::nvs::EspDefaultNvsPartition;

use crate::config::Outcome;
use crate::errors::{self, Target};
use crate::journal::{self, Phase};
use crate::station::StationId;
use crate::status::{self, Label, LcdPins, LedPins, Pins, State};
use crate::storage::fat::{WriteWindow, BASE_PATH};
use crate::storage::{self, volume, Partition, SdCard};
use crate::{net, scan, station};

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
                Label::NoSdCard,
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

    // --- BindCard --------------------------------------------------------
    journal::mark(Phase::BindCard);

    if let Err(e) = storage::msc::bind(&sd, &id) {
        let detail = format!("{e}");
        errors::halt(
            Label::NoUsb,
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
    journal::spawn()?;

    if let Err(e) = storage::fat::register_read_window(&sd) {
        log::error!("the read window would not register: {e}");
    }

    let cap = match &outcome {
        Outcome::Applied(cfg) => cfg.num_replays(),
        Outcome::Rejected(_) | Outcome::Unreadable(_) => crate::config::KEEP_DEFAULT,
    };
    if let Err(e) = scan::spawn(sd.clone(), id.to_string(), cap as usize) {
        log::error!("the scan task would not start: {e}");
    }

    // --- EstablishNetworkServices ----------------------------------------
    journal::mark(Phase::EstablishNetworkServices);

    let plan = net::Plan::from(&outcome, &id.to_string());
    if let Err(e) = net::spawn(p.modem, nvs, sd.clone(), plan) {
        errors::error(
            Target::Session,
            Label::NoWifi,
            "net",
            &["the network task would not start", &format!("{e}")],
        );
        net::give_up();
    }

    // --- Running ------------------------------------------------------------
    journal::mark(Phase::Running);

    let mut last = State::Booting;
    let mut last_activity = (false, false);
    let mut configured_since: Option<std::time::Instant> = None;
    let mut reported_no_read = false;
    let mut reported_bad_read = false;
    let mut writing = Hold::new(WRITE_HOLD);
    let mut sending = Hold::new(SEND_HOLD);
    loop {
        let is_writing = writing.poll(storage::msc::writes_ok(), storage::msc::cache_dirty() > 0)
            || scan::game_live();
        let is_sending = sending.poll(net::transfers_started(), net::transfers_in_flight() > 0);

        let now = if !storage::msc::media_present() {
            State::Off
        } else if errors::session_has_errors() {
            State::Error
        } else if is_writing || is_sending {
            State::Busy
        } else if net::result() == net::NetResult::Pending
            && storage::msc::mounted()
            && storage::msc::reads_ok() > 0
        {
            State::Booting
        } else if storage::msc::mounted() && storage::msc::reads_ok() > 0 {
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

        match (storage::msc::mounted(), storage::msc::reads_ok()) {
            (true, 0) => {
                let since = configured_since.get_or_insert_with(std::time::Instant::now);
                if !reported_no_read && since.elapsed() >= READ_GRACE {
                    reported_no_read = true;
                    let detail = format!(
                        "first transfer error 0x{:x}, {} mount(s)",
                        storage::msc::first_err(),
                        storage::msc::mounts(),
                    );
                    errors::error(
                        Target::Late,
                        Label::NoUsb,
                        "usb",
                        &[
                            "the host configured us but has read nothing",
                            &detail,
                            "the card answered at probe; the transfer path did not",
                        ],
                    );
                }
            }
            _ => configured_since = None,
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
                    Label::SdUnreadable,
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
            last = State::Off;
            last_activity = (false, false);
            status::set_activity(false, false);
        }

        if storage::msc::take_load() {
            log::info!("host reloaded the medium");
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

    if let Err(e) = storage::msc::flush_all() {
        errors::error(
            Target::Late,
            Label::SdUnreadable,
            "sd",
            &["could not flush the write cache on eject", &format!("{e}")],
        );
        return;
    }

    journal::persist_now();
    storage::msc::set_media(false);

    match WriteWindow::open(sd) {
        Ok(window) => {
            errors::mirror(BASE_PATH, &id.to_string());
            drop(window);
        }
        Err(e) => log::warn!("eject: could not open the write window: {e}"),
    }

    status::set(State::Off);
    log::info!("eject complete: safe to unplug");
}

const READ_GRACE: Duration = Duration::from_secs(10);

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
                "the partition is {} GB; a Beamer volume may be at most 16 GiB",
                sectors / (2 * 1024 * 1024),
            );
            errors::error(
                Target::Session,
                Label::WrongFormat,
                "sd",
                &[
                    "this card is partitioned larger than a Beamer supports",
                    &detail,
                    "repartition it to 16 GB or smaller; see the README",
                ],
            );
        }
        Partition::Missing => {
            errors::error(
                Target::Session,
                Label::WrongFormat,
                "sd",
                &[
                    "no FAT32 partition on this card",
                    "exFAT, GPT and unpartitioned cards all look like this",
                    "format it MS-DOS (FAT32), 16 GB or smaller; see the README",
                ],
            );
        }
        Partition::Unreadable(e) => {
            let detail = format!("{e}");
            errors::error(
                Target::Session,
                Label::SdUnreadable,
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
                Label::SdUnreadable,
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
    let outcome = match std::fs::read(&path) {
        Ok(bytes) => Outcome::parse_bytes(&bytes),
        Err(e) => Outcome::unreadable(format!("{e}")),
    };

    let station_id = id.to_string();

    match &outcome {
        Outcome::Applied(cfg) => {
            log::info!(
                "config: accepted, station-name {:?}, {} replays served, hostname {}",
                cfg.display_name(&station_id),
                cfg.num_replays(),
                cfg.hostname(&station_id),
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
                errors::error(Target::Session, Label::BadConfig, "config", &[head, detail]);
            }
        }
        Outcome::Unreadable(why) => {
            errors::error(
                Target::Session,
                Label::NoConfig,
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

    volume::write_debug(BASE_PATH, &window, &station_id, reset_reason());

    drop(window);
    log::info!("write window closed");

    outcome
}

fn seed_volume() {
    for dir in ["CONFIG", "SLIPPI", ".fseventsd"] {
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
# The Beamer reads this file at every boot.
#
# Until SSID is filled in, this station has no network. That is expected.
# After a reboot, if CONFIG/error.txt exists it says what went wrong. The
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

SSID=
PASSWORD=
COUNTRY=US
HIDDEN=false
STATION-NAME=
NUM-REPLAYS-SERVED=10
";
