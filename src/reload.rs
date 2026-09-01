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

pub struct Watcher {
    raw: Vec<u8>,
    settings: Settings,
    plan: Plan,
    quiet_since: Option<Instant>,
    armed: bool,
}

impl Watcher {
    pub fn new(raw: Vec<u8>, settings: Settings, plan: Plan) -> Watcher {
        Watcher {
            raw,
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
            Read::Ok(raw) if raw == self.raw => {
                self.armed = false;
                Action::None
            }
            Read::Ok(raw) => {
                self.armed = false;
                self.apply(raw, station_id)
            }
        }
    }

    fn apply(&mut self, raw: Vec<u8>, station_id: &str) -> Action {
        let outcome = Outcome::parse_bytes(&raw);

        if let Outcome::Rejected(problems) = &outcome {
            self.raw = raw;
            log::error!(
                "reload: {} problem(s); the file is rejected whole",
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
            return Action::None;
        }

        let settings = Settings::from(&outcome);
        let plan = Plan::from(&outcome, station_id);
        self.raw = raw;

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
    Ok(Vec<u8>),
}

fn read(sd: &SdCard) -> Read {
    let window = match ReadWindow::try_open(sd) {
        Ok(Some(w)) => w,
        Ok(None) => return Read::Busy,
        Err(e) => return Read::Failed(format!("{e}")),
    };

    match std::fs::read(window.path(PATH)) {
        Ok(bytes) => Read::Ok(bytes),
        Err(e) => Read::Failed(format!("{e}")),
    }
}
