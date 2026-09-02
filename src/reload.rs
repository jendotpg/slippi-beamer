use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use crate::config::{Outcome, Settings};
use crate::errors::{self, Target};
use crate::net::{self, check, NetResult, Plan};
use crate::scan;
use crate::status::{self, ErrorLabel};
use crate::storage::fat::ReadWindow;
use crate::storage::SdCard;

const SETTLE: Duration = Duration::from_millis(50);

const PATH: &str = "CONFIG/config.txt";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    None,
    Restart,
}

static SCRATCH: Mutex<crate::config::ConfigBytes> = Mutex::new(crate::config::ConfigBytes::new());
static SEEN: Mutex<u64> = Mutex::new(0);

fn hash(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

fn lock(
    b: &'static Mutex<crate::config::ConfigBytes>,
) -> MutexGuard<'static, crate::config::ConfigBytes> {
    b.lock().unwrap_or_else(|e| e.into_inner())
}

pub fn load_initial(path: &str) -> Outcome {
    let mut scratch = lock(&SCRATCH);
    match crate::config::read_file(path, &mut scratch) {
        Ok(()) => {
            *SEEN.lock().unwrap_or_else(|e| e.into_inner()) = hash(&scratch);
            Outcome::parse_bytes(&scratch)
        }
        Err(e) => {
            scratch.clear();
            Outcome::unreadable(format!("{e}"))
        }
    }
}

pub struct Watcher {
    settings: Settings,
    plan: Plan,
    quiet_since: Option<Instant>,
    armed: bool,
}

impl Watcher {
    pub fn new(settings: Settings, plan: Plan) -> Watcher {
        Watcher {
            settings,
            plan,
            quiet_since: None,
            armed: false,
        }
    }

    pub fn poll(&mut self, sd: &SdCard, station_id: &str, writing: bool) -> Action {
        if writing {
            self.armed = true;
            self.quiet_since = None;
            return Action::None;
        }

        if !self.armed {
            return Action::None;
        }

        let since = *self.quiet_since.get_or_insert_with(Instant::now);
        if since.elapsed() < SETTLE {
            return Action::None;
        }

        match read(sd) {
            Read::Busy => Action::None, // a transfer has the volume; try again next tick
            Read::Failed(why) => {
                self.armed = false;
                log::warn!("reload: {PATH} would not read: {why}");
                Action::None
            }
            Read::Ok
                if hash(&lock(&SCRATCH)) == *SEEN.lock().unwrap_or_else(|e| e.into_inner()) =>
            {
                self.armed = false;
                Action::None
            }
            Read::Ok => {
                self.armed = false;
                self.apply(station_id)
            }
        }
    }

    fn apply(&mut self, station_id: &str) -> Action {
        let outcome = Outcome::parse_bytes(&lock(&SCRATCH));

        if let Outcome::Rejected(problems) = &outcome {
            promote();
            log::error!(
                "reload: {} problem(s); the file is rejected whole",
                problems.len()
            );
            for e in problems {
                let [head, detail] = e.lines();
                errors::error(
                    Target::Late,
                    ErrorLabel::BadConfig,
                    "config",
                    &[head, detail],
                );
            }
            return Action::None;
        }

        let settings = Settings::from(&outcome);
        let plan = Plan::from(&outcome, station_id);
        promote();

        // the radio cannot come back from any of these without a re-boot
        if errors::session_has_errors()
            || net::result() != NetResult::Ok
            || plan.join.is_none()
            || self.plan.join.is_none()
        {
            log::info!("reload: accepted, but the radio needs a boot to follow it");
            return Action::Restart;
        }

        self.apply_settings(settings);

        if plan.join != self.plan.join || plan.hostname != self.plan.hostname {
            log::info!(
                "reload: handing the net thread {:?} as {}",
                plan.join.as_ref().map(|j| j.ssid.as_str()),
                plan.hostname,
            );
            net::reconfigure(plan.clone());
        } else if plan.station_name != self.plan.station_name {
            log::info!("reload: station name is now {:?}", plan.station_name);
            status::set_name(&plan.station_name);
            check::set(check::Identity {
                station: plan.station.clone(),
                station_name: plan.station_name.clone(),
                ssid: plan.join.as_ref().map(|j| j.ssid.clone()),
            });
        }
        self.plan = plan;

        Action::None
    }

    fn apply_settings(&mut self, next: Settings) {
        let was = self.settings;
        self.settings = next;

        if next.led_global != was.led_global {
            status::set_brightness(next.led_global);
        }

        if next.flip_screen != was.flip_screen {
            status::set_flipped(next.flip_screen);
        }

        if next.num_replays != was.num_replays {
            scan::set_keep(next.num_replays as usize);
        }

        if next.replay_cap != was.replay_cap {
            scan::set_replay_cap(next.replay_cap);
        }

        if next.debug != was.debug {
            log::info!("reload: DEBUG is read at boot only; it takes effect on the next one");
        }

        if next != was {
            log::info!(
                "reload: {} served, cap {}, LED {}, flipped {}",
                next.num_replays,
                next.replay_cap,
                next.led_global,
                next.flip_screen,
            );
        }
    }
}

enum Read {
    Busy,
    Failed(String),
    Ok,
}

fn promote() {
    *SEEN.lock().unwrap_or_else(|e| e.into_inner()) = hash(&lock(&SCRATCH));
}

fn read(sd: &SdCard) -> Read {
    let window = match ReadWindow::try_open(sd) {
        Ok(Some(w)) => w,
        Ok(None) => return Read::Busy,
        Err(e) => return Read::Failed(format!("{e}")),
    };

    let mut scratch = lock(&SCRATCH);
    match crate::config::read_file(&window.path(PATH), &mut scratch) {
        Ok(()) => Read::Ok,
        Err(e) => Read::Failed(format!("{e}")),
    }
}
