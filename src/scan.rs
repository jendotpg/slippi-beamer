//! The expensive part of this is the directory walk: `Game_20260814T181203.slp`
//! needs three FAT32 entries cause of its long name, so about five files per
//! sector, and a 2000-file card is a 400-sector walk against the controller the
//! Wii is writing through. As an optimization, we don't walk the directory while
//! a game is live - a Wii can only update one game at once!
//!
//! In short:
//!     peek runs every [`TICK`]
//!     list runs as soon as a game is admitted,
//!     list runs on the next tick after a host write, if no game is live
//!     list runs every [`LIST_BACKSTOP_TICKS`] as a backstop -- writes reset it

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use esp_idf_svc::hal::cpu::Core;
use esp_idf_svc::hal::task::thread::ThreadSpawnConfiguration;

use crate::publish::PublishedSet;
use crate::report;
use crate::slp;
use crate::status;
use crate::storage::fat::ReadWindow;
use crate::storage::{msc, SdCard};
use crate::warnings::{self, WarningLabel};

const TICK: Duration = Duration::from_secs(1);
static REPLAY_CAP: AtomicU32 = AtomicU32::new(crate::config::REPLAY_CAP_DEFAULT);
const MAX_NEW: usize = 8;
static GAME_LIVE: AtomicBool = AtomicBool::new(false);

pub fn game_live() -> bool {
    GAME_LIVE.load(Ordering::Relaxed)
}

pub fn uptime_s() -> u64 {
    let us = unsafe { esp_idf_svc::sys::esp_timer_get_time() };
    (us.max(0) / 1_000_000) as u64
}

pub fn replay_cap() -> u32 {
    REPLAY_CAP.load(Ordering::Relaxed)
}

static FAST: Mutex<Option<report::Fast>> = Mutex::new(None);
static SET: Mutex<Option<PublishedSet>> = Mutex::new(None);

fn lock<T>(m: &'static Mutex<T>) -> MutexGuard<'static, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

pub fn fast() -> report::Fast {
    lock(&FAST).clone().unwrap_or_default()
}

pub fn is_published(name: &str) -> bool {
    lock(&SET).as_ref().is_some_and(|s| s.contains(name))
}

pub fn index_json() -> Vec<u8> {
    lock(&SET)
        .as_ref()
        .map(|s| s.index_json().to_vec())
        .unwrap_or_else(|| report::index_json("", &[]))
}

pub fn forget_all() {
    {
        let mut guard = lock(&SET);
        let Some(s) = guard.as_mut() else { return };
        s.clear();
    }
    let cap = replay_cap();
    warnings::set_fill(0, cap);
    status::set_files(0, cap);
}

pub fn refresh() {
    lock(&TRACKER).get_or_insert_with(Tracker::new).list();
}

static TRACKER: Mutex<Option<Tracker>> = Mutex::new(None);

pub fn spawn(sd: Arc<SdCard>, station: String, cap: usize, replay_cap: u32) -> anyhow::Result<()> {
    REPLAY_CAP.store(replay_cap, Ordering::Relaxed);
    *lock(&SET) = Some(PublishedSet::new(station, cap));
    *lock(&TRACKER) = Some(Tracker::with_card(sd));

    ThreadSpawnConfiguration {
        name: Some(c"scan"),
        stack_size: 8192,
        priority: 4,
        pin_to_core: Some(Core::Core0),
        ..Default::default()
    }
    .set()?;

    std::thread::Builder::new().stack_size(8192).spawn(run)?;

    ThreadSpawnConfiguration::default().set()?;
    Ok(())
}

fn run() {
    loop {
        std::thread::sleep(TICK);

        let mut guard = lock(&TRACKER);
        let Some(t) = guard.as_mut() else { continue };

        if msc::take_dirty() {
            t.pending_peek = true;
            t.pending_list = true;
            t.ticks_since_list = 0;
        }
        t.ticks_since_list = t.ticks_since_list.saturating_add(1);

        if t.live.is_some() {
            t.ticks_since_peek = t.ticks_since_peek.saturating_add(1);
            if t.pending_peek || t.ticks_since_peek >= PEEK_BACKSTOP_TICKS {
                t.peek_tracked();
            }
        } else if t.pending_list || t.ticks_since_list >= LIST_BACKSTOP_TICKS {
            t.list();
        }
    }
}

struct Tracker {
    sd: Option<Arc<SdCard>>,
    seen: Vec<u32>,
    has_baseline: bool,
    live: Option<String>,
    pending_peek: bool,
    ticks_since_peek: u32,
    pending_list: bool,
    ticks_since_list: u32,
    mount_fails: u32,
}

const MOUNT_FAILS_BEFORE_ERROR: u32 = 5;
const PEEK_BACKSTOP_TICKS: u32 = 10;
const LIST_BACKSTOP_TICKS: u32 = 60;

impl Tracker {
    fn new() -> Tracker {
        Tracker {
            sd: None,
            seen: Vec::new(),
            has_baseline: false,
            live: None,
            pending_peek: false,
            ticks_since_peek: 0,
            pending_list: true, // the baseline listing, on the first tick
            ticks_since_list: 0,
            mount_fails: 0,
        }
    }

    fn with_card(sd: Arc<SdCard>) -> Tracker {
        Tracker {
            sd: Some(sd),
            ..Tracker::new()
        }
    }

    fn peek_tracked(&mut self) {
        let Some(name) = self.live.clone() else {
            return;
        };
        let Some(sd) = self.sd.clone() else { return };

        let window = match ReadWindow::try_open(&sd) {
            Ok(Some(w)) => w,
            Ok(None) => return,
            Err(e) => {
                log::warn!(
                    "scan: could not mount to peek {name}: {e} ({})",
                    crate::journal::heap_note()
                ); // not an error - sometimes this fails because an .slp is in flight over wifi
                return;
            }
        };

        self.pending_peek = false;
        self.ticks_since_peek = 0;

        match peek(&window, &name) {
            Some(game) => {
                let live = game.live;
                self.publish_game(&game);
                if !live {
                    let size = size_of(&window, &name);
                    drop(window);
                    self.admit(&name, size);
                    self.stop_tracking();
                    self.list();
                }
            }
            None => {
                log::warn!("scan: {name} is no longer readable; stopped tracking it");
                self.stop_tracking();
                GAME_LIVE.store(false, Ordering::Relaxed);
            }
        }
    }

    fn list(&mut self) {
        let Some(sd) = self.sd.clone() else { return };

        let window = match ReadWindow::try_open(&sd) {
            Ok(Some(w)) => w,
            Ok(None) => {
                log::info!("scan: skipping the listing, the volume is busy");
                return;
            } //not an error - sometimes the listing is being served or similar
            Err(e) => {
                log::warn!(
                    "scan: could not mount to list: {e} ({})",
                    crate::journal::heap_note()
                );
                warnings::set(WarningLabel::DriveFailing, true);
                self.note_mount_failure(&format!("{e}"));
                return;
            }
        };

        let mut hashes: Vec<u32> = Vec::new();
        let mut fresh: Vec<String> = Vec::new();
        let known = &self.seen;
        let walk = window.for_each_replay(REPLAY_CAP.load(Ordering::Relaxed), |name| {
            let h = hash(name);
            hashes.push(h);
            if known.binary_search(&h).is_err() && fresh.len() < MAX_NEW {
                fresh.push(name.to_owned());
            }
        });

        let (count, _capped) = match walk {
            Ok(v) => v,
            Err(e) => {
                log::warn!("scan: could not list SLIPPI/: {e}");
                warnings::set(WarningLabel::DriveFailing, true);
                return;
            }
        };

        self.mount_fails = 0;
        self.pending_list = false;
        self.ticks_since_list = 0;

        hashes.sort_unstable();
        let first = !self.has_baseline;
        self.seen = hashes;
        self.has_baseline = true;

        {
            let mut f = lock(&FAST);
            f.get_or_insert_with(report::Fast::default).replay_count = count;
        }

        let cap = replay_cap();
        warnings::set(WarningLabel::DriveFailing, false);
        warnings::set_fill(count, cap);
        status::set_files(count, cap);

        if first {
            log::info!("scan: baseline -- {count} replay(s) on the card, none served");
            return;
        }

        if fresh.len() >= MAX_NEW {
            log::warn!(
                "scan: {}+ new replays at once -- treating as a volume change, not a game",
                fresh.len()
            );
            return;
        }

        let mut finished: Vec<(String, u64)> = Vec::new();
        for name in &fresh {
            let Some(game) = peek(&window, name) else {
                continue;
            };
            if game.live {
                log::info!("scan: {name} is being written");
                self.publish_game(&game);
                self.live = Some(name.clone());
                self.pending_peek = false;
                self.ticks_since_peek = 0;
                finished.clear();
                break;
            }
            self.publish_game(&game); // if a game started and finished between ticks, publish it
                                      // this is worth thinking further about - maybe dont? do
                                      // we need 10 second games at all? on the other hand, maybe
                                      // the tick can be bumped when facing performance issues, and
                                      // sami singles fox dittos can actually be thta fast...
            finished.push((name.clone(), size_of(&window, name)));
        }

        drop(window);
        for (name, size) in finished {
            self.admit(&name, size);
        }
    }

    fn stop_tracking(&mut self) {
        self.live = None;
        self.pending_peek = false;
        self.ticks_since_peek = 0;
    }

    fn note_mount_failure(&mut self, detail: &str) {
        self.mount_fails += 1;
        if self.mount_fails != MOUNT_FAILS_BEFORE_ERROR {
            return;
        }
        crate::errors::error(
            crate::errors::Target::Late,
            status::ErrorLabel::SdUnreadable,
            "sd",
            &[
                "the volume has stopped mounting for reading",
                detail,
                "replays are still being recorded; this station cannot serve them",
            ],
        );
    }

    fn admit(&mut self, name: &str, size: u64) {
        let at = uptime_s();
        let mut guard = lock(&SET);
        let Some(set) = guard.as_mut() else { return };
        if set.admit(name, size, at) {
            let n = set.len();
            drop(guard);
            log::info!("scan: publishing {name} ({size} B); {n} replay(s) served");
        }
    }

    fn publish_game(&self, game: &slp::Game) {
        GAME_LIVE.store(game.live, Ordering::Relaxed);
        let mut f = lock(&FAST);
        f.get_or_insert_with(report::Fast::default).game = Some(game.to_json());
    }
}

fn peek(window: &ReadWindow, name: &str) -> Option<slp::Game> {
    let path = window.path(&format!("SLIPPI/{name}"));
    let file = match std::fs::File::open(&path) {
        Ok(f) => f,
        Err(e) => {
            log::warn!("scan: {path}: {e}");
            return None;
        }
    };
    match slp::peek_reader(file) {
        Ok(Ok(game)) => Some(game),
        Ok(Err(e)) => {
            log::warn!("scan: {path}: {e}");
            None
        }
        Err(e) => {
            log::warn!("scan: {path}: could not read the header: {e}");
            None
        }
    }
}

fn size_of(window: &ReadWindow, name: &str) -> u64 {
    std::fs::metadata(window.path(&format!("SLIPPI/{name}")))
        .map(|m| m.len())
        .unwrap_or(0)
}

fn hash(s: &str) -> u32 {
    let mut h: u32 = 0x811c_9dc5;
    for b in s.as_bytes() {
        h ^= *b as u32;
        h = h.wrapping_mul(0x0100_0193);
    }
    h
}
