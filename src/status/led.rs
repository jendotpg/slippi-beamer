use esp_idf_svc::hal::gpio::{AnyIOPin, AnyOutputPin, Gpio39, Gpio40};
use esp_idf_svc::hal::spi::config::Config as SpiConfig;
use esp_idf_svc::hal::spi::{SpiDeviceDriver, SpiDriver, SpiDriverConfig, SPI3};
use esp_idf_svc::hal::units::FromValueType;

use super::State;

const BRIGHTNESS: u8 = 6;

fn dim(c: u8) -> u8 {
    c / 6
}

pub struct Led<'d> {
    spi: SpiDeviceDriver<'d, SpiDriver<'d>>,
    last: Option<(u8, u8, u8)>,
}

impl<'d> Led<'d> {
    pub fn new(spi: SPI3<'d>, sclk: Gpio39<'d>, data: Gpio40<'d>) -> anyhow::Result<Self> {
        let spi = SpiDeviceDriver::new_single(
            spi,
            sclk,
            data,
            Option::<AnyIOPin>::None,
            Option::<AnyOutputPin>::None,
            &SpiDriverConfig::new(),
            &SpiConfig::new().baudrate(1.MHz().into()),
        )?;
        Ok(Led { spi, last: None })
    }

    pub fn set(&mut self, r: u8, g: u8, b: u8) {
        if self.last == Some((r, g, b)) {
            return;
        }
        self.last = Some((r, g, b));

        let frame = [
            0x00,
            0x00,
            0x00,
            0x00, // start frame
            0xE0 | (BRIGHTNESS & 0x1F),
            b,
            g,
            r,
            0xFF,
            0xFF,
            0xFF,
            0xFF, // end frame
        ];
        if let Err(e) = self.spi.write(&frame) {
            log::warn!("LED write failed: {e}");
            self.last = None;
        }
    }

    pub fn render(&mut self, state: State, bright: bool) {
        const AMBER: (u8, u8, u8) = (255, 140, 0);
        const GREEN: (u8, u8, u8) = (0, 255, 0);
        const RED: (u8, u8, u8) = (255, 0, 0);

        if state == State::Off {
            return self.set(0, 0, 0);
        }

        let (r, g, b) = if state.busy() {
            AMBER
        } else if state == State::Error {
            RED
        } else {
            GREEN
        };

        if bright {
            self.set(r, g, b);
        } else {
            self.set(dim(r), dim(g), dim(b));
        }
    }
}
