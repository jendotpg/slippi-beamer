use std::sync::atomic::{AtomicU16, Ordering};

static N: AtomicU16 = AtomicU16::new(1);

pub fn reset() {
    set(1);
}

pub fn current() -> String {
    format!("Station {}", N.load(Ordering::Relaxed))
}

pub fn bump() {
    set(N.load(Ordering::Relaxed).saturating_add(1));
}

pub fn lower() {
    set(N.load(Ordering::Relaxed).saturating_sub(1));
}

fn set(n: u16) {
    N.store(n.max(1), Ordering::Relaxed);
    let name = current();
    crate::status::set_name(&name);
    crate::net::announce::set_name(&name);

    let mut id = crate::net::check::identity();
    if !id.station.is_empty() {
        id.station_name = name;
        crate::net::check::set(id);
    }
}
