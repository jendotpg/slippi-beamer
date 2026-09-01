use crate::journal;
use crate::storage::fat::WriteWindow;

const DIR: &str = "LOGS";
const STEM: &str = "debug";

fn next_name(dir: &str) -> String {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) => {
            log::warn!("could not list {DIR}/, overwriting {STEM}_0001.txt: {e}");
            return format!("{STEM}_0001.txt");
        }
    };

    let prefix = format!("{STEM}_");
    let mut highest = 0u32;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let Some(rest) = name.strip_suffix(".txt") else {
            continue;
        };
        let n = if rest == STEM {
            1
        } else {
            match rest.strip_prefix(&prefix).map(str::parse::<u32>) {
                Some(Ok(n)) => n,
                _ => continue,
            }
        };
        highest = highest.max(n);
    }

    format!("{STEM}_{:04}.txt", highest + 1)
}

pub fn write_debug(base: &str, _window: &WriteWindow, station_id: &str, reset: &str) {
    let dir = format!("{base}/{DIR}");
    let name = next_name(&dir);
    let path = format!("{dir}/{name}");

    let mut body = String::with_capacity(1024);
    body.push_str("Beamer debug\n");
    body.push_str(&format!("station {station_id}\n"));

    let boot = unsafe { esp_idf_svc::sys::beamer_boot_count() };
    body.push_str(&format!("this boot is boot {boot}, after {reset}\n"));

    for l in journal::census_lines() {
        body.push_str(&format!("since flashing, {l}\n"));
    }
    body.push_str(HEADER);

    match journal::previous_progress() {
        Some((phase, reset, boots)) => {
            body.push_str(&format!("  reached phase: {phase}\n"));
            body.push_str(&format!(
                "  was boot {boots} of that power cycle, started after {}\n",
                journal::reset_name(reset)
            ));
            if boots > 1 {
                body.push_str("  ** THE STATION RESET WHILE THE HOST HELD THE DRIVE **\n");
            }
        }
        None => body.push_str("  phase not recorded\n"),
    }

    let lines = journal::previous_lines();
    if lines.is_empty() {
        body.push_str("  nothing was recorded. either this is the first boot since\n");
        body.push_str("  the firmware was flashed, or the previous boot ended before\n");
        body.push_str("  anything had a chance to be written down.\n");
    } else {
        for l in &lines {
            body.push_str("  ");
            body.push_str(l);
            body.push('\n');
        }
    }

    let log = journal::previous_log_lines();
    body.push_str("\n[previous boot log]\n");
    if log.is_empty() {
        body.push_str("  nothing was captured.\n");
    } else {
        for l in &log {
            body.push_str("  ");
            body.push_str(l);
            body.push('\n');
        }
    }

    let crlf = body.replace('\n', "\r\n"); // windows love <3
    match std::fs::write(&path, crlf.as_bytes()) {
        Ok(()) => log::info!("wrote {DIR}/{name}"),
        Err(e) => log::warn!("could not write {DIR}/{name}: {e}"),
    }
}

const HEADER: &str = "
Everything below happened during the PREVIOUS boot - if you just pulled this
out of a Wii, you can see what happened while it was on that Wii.

[previous boot]
";

pub fn wipe_replays(sd: &super::SdCard) -> Result<u32, String> {
    use crate::status::{self, State};

    let previous = status::get();
    status::set_activity(true, false);
    status::set(State::Busy);

    let result = wipe_inner(sd);

    status::set_activity(false, false);
    status::set(previous);
    result
}

fn wipe_inner(sd: &super::SdCard) -> Result<u32, String> {
    use crate::storage::msc;

    msc::flush_all()
        .map_err(|e| format!("could not flush the write cache before the reset: {e}"))?;

    msc::set_media(false);

    let outcome = (|| {
        let window =
            WriteWindow::open(sd).map_err(|e| format!("could not open the write window: {e}"))?;
        let n = unlink_replays();
        drop(window);
        Ok(n)
    })();

    msc::invalidate_all();
    msc::set_media(true);
    outcome
}

fn unlink_replays() -> u32 {
    let dir = format!("{}/SLIPPI", crate::storage::fat::BASE_PATH);
    let entries = match std::fs::read_dir(&dir) {
        Ok(e) => e,
        Err(e) => {
            log::warn!("reset: could not open SLIPPI/: {e}");
            return 0;
        }
    };

    let mut names = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name();
        if let Some(name) = name.to_str() {
            if crate::publish::is_replay_name(name) {
                names.push(name.to_owned());
            }
        }
    }

    let mut n = 0;
    for name in names {
        match std::fs::remove_file(format!("{dir}/{name}")) {
            Ok(()) => n += 1,
            Err(e) => log::warn!("reset: could not remove {name}: {e}"),
        }
    }
    n
}
