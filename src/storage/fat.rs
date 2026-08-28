//! FatFs views of the card: one read-write, one read-only.

use std::ffi::CString;
use std::sync::{Mutex, MutexGuard};

use esp_idf_svc::sys::{
    beamer_fat_ro_register, esp, esp_vfs_fat_register, esp_vfs_fat_unregister_path, f_mount,
    ff_diskio_get_drive, ff_diskio_register, ff_diskio_register_sdmmc, EspError, FATFS,
};

unsafe fn ff_diskio_release(pdrv: u8) {
    ff_diskio_register(pdrv, core::ptr::null());
}

use super::SdCard;

pub const BASE_PATH: &str = "/sd";
pub const RO_BASE_PATH: &str = "/ro";

const MAX_FILES: usize = 2;

struct Mount {
    pdrv: u8,
    base: CString,
    drive: CString,
}

impl Mount {
    fn open(
        base_path: &str,
        register: impl FnOnce(u8) -> Result<(), EspError>,
    ) -> Result<Mount, EspError> {
        let mut pdrv: u8 = 0;
        esp!(unsafe { ff_diskio_get_drive(&mut pdrv) })?;

        let drive = CString::new(format!("{pdrv}:")).expect("no interior NUL");
        let base = CString::new(base_path).expect("no interior NUL");

        if let Err(e) = register(pdrv) {
            unsafe { ff_diskio_release(pdrv) };
            return Err(e);
        }

        let mut fs: *mut FATFS = core::ptr::null_mut();
        if let Err(e) =
            esp!(unsafe { esp_vfs_fat_register(base.as_ptr(), drive.as_ptr(), MAX_FILES, &mut fs) })
        {
            unsafe { ff_diskio_release(pdrv) };
            return Err(e);
        }

        let res = unsafe { f_mount(fs, drive.as_ptr(), 1) };
        if res != 0 {
            unsafe {
                esp_vfs_fat_unregister_path(base.as_ptr());
                ff_diskio_release(pdrv);
            }
            log::error!("f_mount({base_path}) failed: FRESULT {res}");
            return Err(EspError::from_infallible::<
                { esp_idf_svc::sys::ESP_ERR_NOT_FOUND },
            >());
        }

        Ok(Mount { pdrv, base, drive })
    }
}

impl Drop for Mount {
    fn drop(&mut self) {
        unsafe {
            f_mount(core::ptr::null_mut(), self.drive.as_ptr(), 0);
            esp_vfs_fat_unregister_path(self.base.as_ptr());
            ff_diskio_release(self.pdrv);
        }
    }
}

pub struct WriteWindow(#[allow(dead_code)] Mount);

impl WriteWindow {
    pub fn open(sd: &SdCard) -> Result<WriteWindow, EspError> {
        Mount::open(BASE_PATH, |pdrv| {
            unsafe { ff_diskio_register_sdmmc(pdrv, sd.raw()) };
            Ok(())
        })
        .map(WriteWindow)
    }
}

static RO_LOCK: Mutex<()> = Mutex::new(());
static RO_VOLUME: std::sync::OnceLock<RoVolume> = std::sync::OnceLock::new();

struct RoVolume {
    #[allow(dead_code)]
    pdrv: u8,
    #[allow(dead_code)]
    base: CString,
    drive: CString,
    fs: *mut FATFS,
}

unsafe impl Send for RoVolume {}
unsafe impl Sync for RoVolume {}

pub fn register_read_window(sd: &SdCard) -> Result<(), EspError> {
    if RO_VOLUME.get().is_some() {
        return Ok(());
    }

    let mut pdrv: u8 = 0;
    esp!(unsafe { ff_diskio_get_drive(&mut pdrv) })?;

    let drive = CString::new(format!("{pdrv}:")).expect("no interior NUL");
    let base = CString::new(RO_BASE_PATH).expect("no interior NUL");

    if let Err(e) = esp!(unsafe { beamer_fat_ro_register(pdrv, sd.raw()) }) {
        unsafe { ff_diskio_release(pdrv) };
        return Err(e);
    }

    let mut fs: *mut FATFS = core::ptr::null_mut();
    if let Err(e) =
        esp!(unsafe { esp_vfs_fat_register(base.as_ptr(), drive.as_ptr(), MAX_FILES, &mut fs) })
    {
        unsafe { ff_diskio_release(pdrv) };
        return Err(e);
    }

    let _ = RO_VOLUME.set(RoVolume {
        pdrv,
        base,
        drive,
        fs,
    });
    log::info!("read window registered on drive {pdrv} at {RO_BASE_PATH}");
    Ok(())
}

pub struct ReadWindow {
    #[allow(dead_code)]
    guard: MutexGuard<'static, ()>,
}

impl ReadWindow {
    pub fn open(sd: &SdCard) -> Result<ReadWindow, EspError> {
        let _ = sd;
        let guard = RO_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        ReadWindow::mount_locked(guard)
    }

    pub fn try_open(sd: &SdCard) -> Result<Option<ReadWindow>, EspError> {
        let _ = sd;
        let guard = match RO_LOCK.try_lock() {
            Ok(g) => g,
            Err(std::sync::TryLockError::Poisoned(e)) => e.into_inner(),
            Err(std::sync::TryLockError::WouldBlock) => return Ok(None),
        };

        ReadWindow::mount_locked(guard).map(Some)
    }

    pub fn path(&self, rel: &str) -> String {
        format!("{RO_BASE_PATH}/{rel}")
    }

    fn mount_locked(guard: MutexGuard<'static, ()>) -> Result<ReadWindow, EspError> {
        let Some(vol) = RO_VOLUME.get() else {
            log::error!("read window used before it was registered");
            return Err(EspError::from_infallible::<
                { esp_idf_svc::sys::ESP_ERR_INVALID_STATE },
            >());
        };

        let res = unsafe { f_mount(vol.fs, vol.drive.as_ptr(), 1) };
        if res != 0 {
            log::error!("f_mount({RO_BASE_PATH}) failed: FRESULT {res}");
            return Err(EspError::from_infallible::<
                { esp_idf_svc::sys::ESP_ERR_NOT_FOUND },
            >());
        }

        Ok(ReadWindow { guard })
    }

    pub fn for_each_replay(
        &self,
        cap: u32,
        mut f: impl FnMut(&str),
    ) -> std::io::Result<(u32, bool)> {
        let mut n = 0u32;
        for entry in std::fs::read_dir(self.path("SLIPPI"))? {
            let entry = match entry {
                Ok(e) => e,
                Err(e) => {
                    log::warn!("SLIPPI/: skipping an unreadable entry: {e}");
                    continue;
                }
            };
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            if !crate::publish::is_replay_name(name) {
                continue;
            }
            if n >= cap {
                return Ok((cap, true));
            }
            n += 1;
            f(name);
        }
        Ok((n, false))
    }
}

impl Drop for ReadWindow {
    fn drop(&mut self) {
        if let Some(vol) = RO_VOLUME.get() {
            unsafe { f_mount(core::ptr::null_mut(), vol.drive.as_ptr(), 0) };
        }
    }
}
