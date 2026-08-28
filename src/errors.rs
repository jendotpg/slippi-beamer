//! Error storage. Errors reportable this boot (i.e. ocurring prebind) get
//! [`Target::Session`] and stay in RAM - errors ocurring after microSD card
//! bind get [`Target::Late`] and are put in NVS.
//!
//! During the boot process, init() below rotates the previous sessions Late
//! errors off of NVS and onto the disk at LOGS/error.txt
use std::sync::{Mutex, MutexGuard};

use esp_idf_svc::nvs::{EspDefaultNvsPartition, EspNvs, NvsDefault};

use crate::status::{self, ErrorLabel, State};

const NAMESPACE: &str = "beamer";
const KEY_LATE: &str = "err_late";
const KEY_PREV: &str = "err_prev";

const CAP: usize = 2048;
const TRUNCATED: &str = "... truncated\n";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    Session,
    Late,
}

struct Store {
    session: String,
    late: String,
    prev: String,
    distinct: u32,
    nvs: Option<EspNvs<NvsDefault>>,
}

static STORE: Mutex<Store> = Mutex::new(Store {
    session: String::new(),
    late: String::new(),
    prev: String::new(),
    distinct: 0,
    nvs: None,
});

fn store() -> MutexGuard<'static, Store> {
    STORE.lock().unwrap_or_else(|e| e.into_inner())
}

pub fn init(part: EspDefaultNvsPartition) {
    let mut s = store();

    let nvs = match EspNvs::new(part, NAMESPACE, true) {
        Ok(nvs) => nvs,
        Err(e) => {
            log::warn!("NVS unavailable; errors will not survive this boot: {e}");
            return;
        }
    };

    let late = read(&nvs, KEY_LATE);
    if late.is_empty() {
        let _ = nvs.remove(KEY_PREV);
    } else if let Err(e) = nvs.set_blob(KEY_PREV, late.as_bytes()) {
        log::warn!("could not rotate the error log: {e}");
    } else {
        let _ = nvs.remove(KEY_LATE);
    }

    s.prev = read(&nvs, KEY_PREV);
    s.nvs = Some(nvs);

    if !s.prev.is_empty() {
        log::info!(
            "previous boot recorded {} error line(s)",
            s.prev.lines().count()
        );
    }
}

pub fn error(target: Target, label: ErrorLabel, component: &str, lines: &[&str]) {
    let Some((head, rest)) = lines.split_first() else {
        return;
    };

    let mut s = store();
    let head_line = format!("[{component}] {head}");

    if s.blob(target).lines().any(|l| l == head_line) {
        log::error!("(already recorded) {head_line}");
        drop(s);
        quiesce(label);
        status::set(State::Error);
        return;
    }

    let indent = " ".repeat(component.len() + 3);
    let mut entry = head_line.clone();
    entry.push('\n');
    for line in rest {
        entry.push_str(&indent);
        entry.push_str(line);
        entry.push('\n');
    }

    append(s.blob_mut(target), &entry);
    s.distinct += 1;
    let more = s.distinct - 1;
    if target == Target::Late {
        s.persist_late();
    }
    drop(s);

    log::error!("{head_line}");
    for line in rest {
        log::error!("  {line}");
    }

    quiesce(label);

    status::set_error(label, head, more);
    status::set(State::Error);
}

/// Turns off the write-back cache
fn quiesce(label: ErrorLabel) {
    use crate::storage::msc;
    msc::set_policy(if label.is_storage_fault() {
        msc::REFUSE
    } else {
        msc::WRITETHROUGH
    });
}

pub fn record_previous(component: &str, lines: &[&str]) {
    let Some((head, rest)) = lines.split_first() else {
        return;
    };

    let indent = " ".repeat(component.len() + 3);
    let mut entry = format!("[{component}] {head}\n");
    for line in rest {
        entry.push_str(&indent);
        entry.push_str(line);
        entry.push('\n');
    }

    log::error!("[{component}] {head}");
    for line in rest {
        log::error!("  {line}");
    }

    append(&mut store().prev, &entry);
}

pub fn halt(label: ErrorLabel, component: &str, lines: &[&str]) -> ! {
    error(Target::Late, label, component, lines);
    loop {
        std::thread::sleep(std::time::Duration::from_secs(60));
    }
}

pub fn session_has_errors() -> bool {
    let s = store();
    !s.session.is_empty() || !s.late.is_empty()
}

pub fn json_errors() -> Vec<String> {
    let s = store();
    s.session
        .lines()
        .chain(s.late.lines())
        .map(str::to_owned)
        .collect()
}

fn have_errors(s: &Store) -> bool {
    !s.prev.is_empty() || !s.session.is_empty()
}

pub fn mirror(base: &str, station_id: &str) {
    let s = store();
    let path = format!("{base}/LOGS/error.txt");

    if !have_errors(&s) {
        if std::path::Path::new(&path).exists() {
            if let Err(e) = std::fs::remove_file(&path) {
                log::warn!("could not remove error.txt: {e}");
            }
        }
        return;
    }

    let mut body = format!("Beamer errors\nstation {station_id}\n\n");
    for line in s.prev.lines() {
        body.push_str("[previous boot] ");
        body.push_str(line);
        body.push('\n');
    }
    body.push_str(&s.session);
    body.push_str(EXPLANATION);
    drop(s);

    let crlf = body.replace('\n', "\r\n"); // notepad support :(
    if let Err(e) = std::fs::write(&path, crlf.as_bytes()) {
        log::warn!("could not write error.txt: {e}");
    } else {
        log::info!("wrote LOGS/error.txt");
    }
}

const EXPLANATION: &str = "
Fix CONFIG/config.txt on this drive, eject it, and plug it back in.

This file is always one boot behind. The LED and the screen are not: if the
LED is SOLID and the screen shows this station's name, the station is working
right now!
";

impl Store {
    fn blob(&self, target: Target) -> &String {
        match target {
            Target::Session => &self.session,
            Target::Late => &self.late,
        }
    }

    fn blob_mut(&mut self, target: Target) -> &mut String {
        match target {
            Target::Session => &mut self.session,
            Target::Late => &mut self.late,
        }
    }

    fn persist_late(&mut self) {
        let Some(nvs) = self.nvs.as_mut() else { return };
        if let Err(e) = nvs.set_blob(KEY_LATE, self.late.as_bytes()) {
            log::warn!("could not persist the error log: {e}");
        }
    }
}

fn append(blob: &mut String, entry: &str) {
    if blob.ends_with(TRUNCATED) {
        return;
    }
    if blob.len() + entry.len() > CAP {
        blob.push_str(TRUNCATED);
        return;
    }
    blob.push_str(entry);
}

fn read(nvs: &EspNvs<NvsDefault>, key: &str) -> String {
    let mut buf = vec![0u8; CAP + TRUNCATED.len()];
    match nvs.get_blob(key, &mut buf) {
        Ok(Some(bytes)) => String::from_utf8_lossy(bytes).into_owned(),
        Ok(None) => String::new(),
        Err(e) => {
            log::warn!("could not read {key}: {e}");
            String::new()
        }
    }
}
