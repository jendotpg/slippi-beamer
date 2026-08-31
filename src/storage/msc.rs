//! Safe wrapper over `components/beamer_msc`.

use crate::station::StationId;
use std::ffi::CString;

use esp_idf_svc::sys::{
    beamer_msc_bind_time_us, beamer_msc_census, beamer_msc_detach, beamer_msc_eject_seen,
    beamer_msc_first_err, beamer_msc_host_owns, beamer_msc_install, beamer_msc_last_cbw_us,
    beamer_msc_maxlun_asks, beamer_msc_media_present, beamer_msc_mounted, beamer_msc_mounts,
    beamer_msc_reads_ok, beamer_msc_set_media, beamer_msc_set_visible, beamer_msc_suspended,
    beamer_msc_take_dirty, beamer_msc_take_eject, beamer_msc_take_load, beamer_msc_umounts,
    beamer_msc_unsup_t, beamer_msc_unsupported, beamer_msc_writes_ok, beamer_wbc_dirty,
    beamer_wbc_flush_all, beamer_wbc_high_water, beamer_wbc_set_policy, beamer_wbc_stalls, esp,
    EspError,
};

use super::SdCard;

pub fn bind(sd: &SdCard, id: &StationId) -> Result<(), EspError> {
    let serial = CString::new(id.usb_serial()).expect("no interior NUL");
    esp!(unsafe { beamer_msc_install(sd.raw(), sd.lock(), serial.as_ptr()) })
}

pub fn host_state() -> &'static str {
    if unsafe { beamer_msc_suspended() } {
        "suspended"
    } else if unsafe { beamer_msc_mounted() } {
        "configured"
    } else {
        "not attached"
    }
}

pub fn mounted() -> bool {
    unsafe { beamer_msc_mounted() }
}

pub fn reads_ok() -> u32 {
    unsafe { beamer_msc_reads_ok() }
}

pub fn mounts() -> u32 {
    unsafe { beamer_msc_mounts() }
}

pub fn umounts() -> u32 {
    unsafe { beamer_msc_umounts() }
}

pub fn writes_ok() -> u32 {
    unsafe { beamer_msc_writes_ok() }
}

pub fn eject_seen() -> bool {
    unsafe { beamer_msc_eject_seen() }
}

pub fn maxlun_asks() -> u32 {
    unsafe { beamer_msc_maxlun_asks() }
}

pub fn set_visible(sectors: u32) {
    unsafe { beamer_msc_set_visible(sectors) }
}

pub fn last_cbw_us() -> i64 {
    unsafe { beamer_msc_last_cbw_us() }
}

// this ones for you nicolet :)
pub fn census() -> Vec<(u8, u32)> {
    const MAX: usize = 16;
    let mut buf = [beamer_msc_unsup_t { count: 0, op: 0 }; MAX];
    let n = unsafe { beamer_msc_census(buf.as_mut_ptr(), MAX) };
    buf[..n.min(MAX)].iter().map(|e| (e.op, e.count)).collect()
}

pub fn unsupported() -> Vec<(u8, u32)> {
    const MAX: usize = 8;
    let mut buf = [beamer_msc_unsup_t { count: 0, op: 0 }; MAX];
    let n = unsafe { beamer_msc_unsupported(buf.as_mut_ptr(), MAX) };
    buf[..n.min(MAX)].iter().map(|e| (e.op, e.count)).collect()
}

pub fn opcode_name(op: u8) -> &'static str {
    match op {
        0x00 => "TEST UNIT READY",
        0x03 => "REQUEST SENSE",
        0x12 => "INQUIRY",
        0x15 => "MODE SELECT(6)",
        0x1A => "MODE SENSE(6)",
        0x1B => "START STOP UNIT",
        0x1E => "PREVENT ALLOW MEDIUM REMOVAL",
        0x23 => "READ FORMAT CAPACITY",
        0x25 => "READ CAPACITY(10)",
        0x28 => "READ(10)",
        0x2A => "WRITE(10)",
        0x2F => "VERIFY(10)",
        0x35 => "SYNCHRONIZE CACHE(10)",
        0x5A => "MODE SENSE(10)",
        _ => "unnamed",
    }
}

pub fn first_err() -> i32 {
    unsafe { beamer_msc_first_err() }
}

pub fn host_owns() -> bool {
    unsafe { beamer_msc_host_owns() }
}

pub fn take_dirty() -> bool {
    unsafe { beamer_msc_take_dirty() }
}

pub fn take_eject() -> bool {
    unsafe { beamer_msc_take_eject() }
}

pub fn take_load() -> bool {
    unsafe { beamer_msc_take_load() }
}

pub fn set_media(present: bool) {
    unsafe { beamer_msc_set_media(present) }
}

pub fn media_present() -> bool {
    unsafe { beamer_msc_media_present() }
}

pub fn detach() {
    unsafe { beamer_msc_detach() }
}

pub use esp_idf_svc::sys::{
    beamer_wbc_policy_t_BEAMER_WBC_REFUSE as REFUSE,
    beamer_wbc_policy_t_BEAMER_WBC_WRITETHROUGH as WRITETHROUGH,
};

pub fn cache_dirty() -> u32 {
    unsafe { beamer_wbc_dirty() }
}

pub fn flush_all() -> Result<(), EspError> {
    esp!(unsafe { beamer_wbc_flush_all() })
}

pub fn set_policy(policy: esp_idf_svc::sys::beamer_wbc_policy_t) {
    unsafe { beamer_wbc_set_policy(policy) }
}

pub fn cache_high_water() -> u32 {
    unsafe { beamer_wbc_high_water() }
}

pub fn cache_stalls() -> u32 {
    unsafe { beamer_wbc_stalls() }
}

pub fn bind_time_s() -> f32 {
    (unsafe { beamer_msc_bind_time_us() }) as f32 / 1_000_000.0
}
