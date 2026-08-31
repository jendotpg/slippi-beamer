use std::sync::{Mutex, MutexGuard};

#[derive(Debug, Clone, Default)]
pub struct Identity {
    pub station: String,
    pub station_name: String,
    pub ssid: Option<String>,
}

static IDENTITY: Mutex<Option<Identity>> = Mutex::new(None);

fn lock<T>(m: &'static Mutex<T>) -> MutexGuard<'static, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

pub fn set(id: Identity) {
    *lock(&IDENTITY) = Some(id);
}

pub fn identity() -> Identity {
    lock(&IDENTITY).clone().unwrap_or_default()
}
