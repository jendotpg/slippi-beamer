pub mod fat;
pub mod msc;
pub mod volume;

use esp_idf_svc::sys::{
    beamer_part_t, beamer_sd_bytes, beamer_sd_init, beamer_sd_probe_partition, sdmmc_card_t,
    EspError, SemaphoreHandle_t, BEAMER_MAX_VOLUME_SECTORS, ESP_ERR_INVALID_SIZE,
    ESP_ERR_NOT_FOUND, ESP_OK,
};

pub enum Partition {
    Ok { sectors: u32, end: u32 },
    TooBig { sectors: u32 },
    Missing,
    Unreadable(EspError),
}

pub struct SdCard {
    card: *mut sdmmc_card_t,
    lock: SemaphoreHandle_t,
}

unsafe impl Send for SdCard {}
unsafe impl Sync for SdCard {}

impl SdCard {
    pub fn probe() -> Result<SdCard, EspError> {
        let mut card: *mut sdmmc_card_t = core::ptr::null_mut();
        let mut lock: SemaphoreHandle_t = core::ptr::null_mut();
        esp_idf_svc::sys::esp!(unsafe { beamer_sd_init(&mut card, &mut lock) })?;
        Ok(SdCard { card, lock })
    }

    pub fn raw(&self) -> *mut sdmmc_card_t {
        self.card
    }

    pub fn lock(&self) -> SemaphoreHandle_t {
        self.lock
    }

    pub fn partition(&self) -> Partition {
        let mut part = beamer_part_t {
            start: 0,
            sectors: 0,
            type_: 0,
        };
        let err = unsafe {
            beamer_sd_probe_partition(self.card, self.lock, BEAMER_MAX_VOLUME_SECTORS, &mut part)
        };
        match err {
            ESP_OK => Partition::Ok {
                sectors: part.sectors,
                end: part.start.saturating_add(part.sectors),
            },
            ESP_ERR_INVALID_SIZE => Partition::TooBig {
                sectors: part.sectors,
            },
            ESP_ERR_NOT_FOUND => Partition::Missing,
            other => {
                Partition::Unreadable(EspError::from(other).unwrap_or_else(|| {
                    EspError::from_infallible::<{ esp_idf_svc::sys::ESP_FAIL }>()
                }))
            }
        }
    }

    pub fn bytes(&self) -> u64 {
        unsafe { beamer_sd_bytes(self.card) }
    }
}
