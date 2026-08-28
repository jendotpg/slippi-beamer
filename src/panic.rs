//! Rust panic handler
//!
//! Only Rust panics reach this. It is not a general crash handler. Any other
//! crash - FreeRTOS stack overflow, allocation issues, C-side aborts - reach
//! the ESP-IDF panic handler (CONFIG_ESP_SYSTEM_PANIC_PRINT_HALT).
//!
//! Note also that this can't cover a panic in the status task handler, so
//! don't panic there!

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crate::errors::{self, Target};
use crate::journal;
use crate::status::Label;

static IN_PANIC: AtomicBool = AtomicBool::new(false);

pub fn install() {
    std::panic::set_hook(Box::new(|info| {
        if IN_PANIC
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return;
        }

        let head = format!("in task {}", task_name());
        let detail = info.to_string().replace('\n', " ");

        let mut frames = [0u8; 256];
        let n = unsafe {
            esp_idf_svc::sys::beamer_backtrace(
                frames.as_mut_ptr() as *mut core::ffi::c_char,
                frames.len(),
            )
        };
        let backtrace = format!(
            "backtrace {}",
            String::from_utf8_lossy(&frames[..n.min(frames.len())])
        );

        errors::error(
            Target::Late,
            Label::Crashed,
            "panic",
            &[
                &head,
                &detail,
                &backtrace,
                // the frames are raw addresses and only decode against the ELF
                // that was flashed, so say which tool and which file!
                "decode: xtensa-esp32s3-elf-addr2line -pfiaC -e \
                 target/xtensa-esp32s3-espidf/release/beamer <backtrace>",
                "the faulting task was parked; the station is still recording",
            ],
        );

        journal::persist_now();

        loop {
            std::thread::sleep(Duration::from_secs(60));
        }
    }));
}

fn task_name() -> String {
    let raw = unsafe { esp_idf_svc::sys::pcTaskGetName(core::ptr::null_mut()) };
    if raw.is_null() {
        return "?".to_string();
    }
    unsafe { core::ffi::CStr::from_ptr(raw) }
        .to_string_lossy()
        .into_owned()
}
