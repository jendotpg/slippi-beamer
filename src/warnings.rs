use std::sync::atomic::{AtomicU32, Ordering};

pub use crate::status::labels::{WarningLabel, WARNINGS};

const FILLING_PCT: u32 = 75;

static ACTIVE: AtomicU32 = AtomicU32::new(0);

pub fn set(w: WarningLabel, on: bool) {
    let bit = w.bit();
    let before = if on {
        ACTIVE.fetch_or(bit, Ordering::Relaxed)
    } else {
        ACTIVE.fetch_and(!bit, Ordering::Relaxed)
    };

    if (before & bit != 0) == on {
        return;
    }

    if on {
        log::warn!("warning: {w} -- {}", w.reason());
    } else {
        log::info!("warning cleared: {w}");
    }
    publish();
}

pub fn set_fill(files: u32, cap: u32) {
    let full = cap > 0 && files >= cap;
    let filling = !full && cap > 0 && files * 100 >= cap * FILLING_PCT;
    set(WarningLabel::DriveFull, full);
    set(WarningLabel::DriveFilling, filling);
}

pub fn any() -> bool {
    ACTIVE.load(Ordering::Relaxed) != 0
}

pub fn first() -> Option<WarningLabel> {
    let active = ACTIVE.load(Ordering::Relaxed);
    WARNINGS.into_iter().find(|w| active & w.bit() != 0)
}

pub fn count() -> u32 {
    ACTIVE.load(Ordering::Relaxed).count_ones()
}

pub fn labels() -> Vec<&'static str> {
    let active = ACTIVE.load(Ordering::Relaxed);
    WARNINGS
        .into_iter()
        .filter(|w| active & w.bit() != 0)
        .map(|w| w.as_str())
        .collect()
}

fn publish() {
    crate::status::set_warning(first(), count().saturating_sub(1));
}
