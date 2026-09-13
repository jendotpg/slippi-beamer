use esp_idf_svc::hal::gpio::{AnyIOPin, Output, Pin, PinDriver};
use esp_idf_svc::hal::spi::{Dma, SpiDriver, SpiDriverConfig};
use esp_idf_svc::sys::{
    esp, esp_lcd_new_panel_io_spi, esp_lcd_panel_io_handle_t, esp_lcd_panel_io_spi_config_t,
    esp_lcd_panel_io_tx_param, spi_host_device_t_SPI2_HOST, EspError,
};

use crate::text::{self, ELLIPSIS};

use super::boot_animation;
use super::font;
use super::{Detail, LcdPins, Net, State, SPINNER_STEPS};

pub const W: u16 = 160;
pub const H: u16 = 80;
const X_GAP: u16 = 1;
const Y_GAP: u16 = 26;
const MADCTL: u8 = 0x68; // this setup is BGR, not RBG...
const MADCTL_FLIPPED: u8 = MADCTL ^ 0xC0;
const BACKLIGHT_ACTIVE_LOW: bool = true;
const PCLK_HZ: u32 = 10_000_000; // deliberately slow - we barely animate...

const SWRESET: u8 = 0x01;
const SLPOUT: u8 = 0x11;
const INVON: u8 = 0x21;
const DISPON: u8 = 0x29;
const CASET: u8 = 0x2A;
const RASET: u8 = 0x2B;
const RAMWR: u8 = 0x2C;
const COLMOD: u8 = 0x3A;
const MADCTL_CMD: u8 = 0x36;
const FRMCTR1: u8 = 0xB1;
const FRMCTR2: u8 = 0xB2;
const FRMCTR3: u8 = 0xB3;
const INVCTR: u8 = 0xB4;
const PWCTR1: u8 = 0xC0;
const PWCTR2: u8 = 0xC1;
const PWCTR3: u8 = 0xC2;
const PWCTR4: u8 = 0xC3;
const PWCTR5: u8 = 0xC4;
const VMCTR1: u8 = 0xC5;
const GMCTRP1: u8 = 0xE0;
const GMCTRN1: u8 = 0xE1;
const NORON: u8 = 0x13;

const fn rgb565(r: u8, g: u8, b: u8) -> u16 {
    ((r as u16 & 0xF8) << 8) | ((g as u16 & 0xFC) << 3) | (b as u16 >> 3)
}

const BLACK: u16 = 0x0000;
const WHITE: u16 = rgb565(255, 255, 255);
const GREY: u16 = rgb565(128, 128, 128);
const GREEN_RGB: (u8, u8, u8) = (0, 255, 0);
const GREEN: u16 = rgb565(GREEN_RGB.0, GREEN_RGB.1, GREEN_RGB.2);
const RED: u16 = rgb565(255, 0, 0);
const AMBER_RGB: (u8, u8, u8) = (255, 140, 0);
const AMBER: u16 = rgb565(AMBER_RGB.0, AMBER_RGB.1, AMBER_RGB.2);

// The spinner's fading tail.
const fn dim(level: u32) -> u16 {
    rgb565(
        (GREEN_RGB.0 as u32 * level / 255) as u8,
        (GREEN_RGB.1 as u32 * level / 255) as u8,
        (GREEN_RGB.2 as u32 * level / 255) as u8,
    )
}

const BAND_PX: usize = W as usize * 24;
const SCRATCH_PX: usize = if BAND_PX > boot_animation::PIXELS {
    BAND_PX
} else {
    boot_animation::PIXELS
};
const SCRATCH_BYTES: usize = SCRATCH_PX * 2;

#[repr(align(4))]
struct Scratch([u8; SCRATCH_BYTES]);
static mut SCRATCH: Scratch = Scratch([0; SCRATCH_BYTES]);

unsafe fn scratch() -> &'static mut [u8; SCRATCH_BYTES] {
    &mut *core::ptr::addr_of_mut!(SCRATCH.0)
}

struct Line {
    buf: [u8; 40],
    len: usize,
}

impl Line {
    fn new() -> Line {
        Line {
            buf: [0; 40],
            len: 0,
        }
    }

    fn push(&mut self, s: &str) -> &mut Line {
        for b in s.bytes() {
            if self.len == self.buf.len() {
                break;
            }
            self.buf[self.len] = b;
            self.len += 1;
        }
        self
    }

    fn push_num(&mut self, n: u32) -> &mut Line {
        let mut digits = [0u8; 10];
        let mut i = digits.len();
        let mut n = n;
        loop {
            i -= 1;
            digits[i] = b'0' + (n % 10) as u8;
            n /= 10;
            if n == 0 {
                break;
            }
        }
        self.push(core::str::from_utf8(&digits[i..]).unwrap_or("?"))
    }

    fn as_str(&self) -> &str {
        core::str::from_utf8(&self.buf[..self.len]).unwrap_or("?")
    }
}

fn drive_backlight(pin: &mut PinDriver<'_, Output>, on: bool) -> Result<(), EspError> {
    if on == BACKLIGHT_ACTIVE_LOW {
        pin.set_low()
    } else {
        pin.set_high()
    }
}

struct DarkOnDrop<'d>(Option<PinDriver<'d, Output>>);

impl<'d> DarkOnDrop<'d> {
    fn take(&mut self) -> PinDriver<'d, Output> {
        self.0.take().expect("taken once")
    }
}

impl Drop for DarkOnDrop<'_> {
    fn drop(&mut self) {
        if let Some(pin) = self.0.take() {
            core::mem::forget(pin);
        }
    }
}

pub struct Lcd<'d> {
    _bus: SpiDriver<'d>,
    io: esp_lcd_panel_io_handle_t,
    _rst: PinDriver<'d, Output>,
    backlight: PinDriver<'d, Output>,
    lit: Option<bool>,
}

impl<'d> Lcd<'d> {
    pub fn new(pins: LcdPins) -> anyhow::Result<Lcd<'d>> {
        let mut backlight = PinDriver::output(pins.bl.degrade_output())?;
        drive_backlight(&mut backlight, false)?;
        let mut dark = DarkOnDrop(Some(backlight));

        let bus = SpiDriver::new(
            pins.spi,
            pins.sclk,
            pins.mosi,
            Option::<AnyIOPin>::None,
            &SpiDriverConfig::new().dma(Dma::Auto(SCRATCH_BYTES)),
        )?;

        let mut io: esp_lcd_panel_io_handle_t = core::ptr::null_mut();
        let config = esp_lcd_panel_io_spi_config_t {
            cs_gpio_num: pins.cs.pin() as i32,
            dc_gpio_num: pins.dc.pin() as i32,
            spi_mode: 0,
            pclk_hz: PCLK_HZ,
            trans_queue_depth: 2,
            lcd_cmd_bits: 8,
            lcd_param_bits: 8,
            ..Default::default()
        };
        esp!(unsafe {
            esp_lcd_new_panel_io_spi(spi_host_device_t_SPI2_HOST as _, &config, &mut io)
        })?;

        let mut rst = PinDriver::output(pins.rst.degrade_output())?;
        let backlight = dark.take();

        rst.set_high()?;
        wait(10);
        rst.set_low()?;
        wait(10);
        rst.set_high()?;
        wait(120);

        let mut lcd = Lcd {
            _bus: bus,
            io,
            _rst: rst,
            backlight,
            lit: Some(false),
        };
        lcd.init();
        lcd.fill(0, 0, W, H, BLACK);
        lcd.backlight(true);
        Ok(lcd)
    }

    /// The ST7735S sequence for the 0.96" 80x160 IPS module.
    fn init(&mut self) {
        self.cmd(SWRESET);
        wait(150);
        self.cmd(SLPOUT);
        wait(120);

        self.cmd_data(FRMCTR1, &[0x05, 0x3C, 0x3C]);
        self.cmd_data(FRMCTR2, &[0x05, 0x3C, 0x3C]);
        self.cmd_data(FRMCTR3, &[0x05, 0x3C, 0x3C, 0x05, 0x3C, 0x3C]);
        self.cmd_data(INVCTR, &[0x03]);
        self.cmd_data(PWCTR1, &[0x62, 0x02, 0x04]);
        self.cmd_data(PWCTR2, &[0xC0]);
        self.cmd_data(PWCTR3, &[0x0D, 0x00]);
        self.cmd_data(PWCTR4, &[0x8D, 0x6A]);
        self.cmd_data(PWCTR5, &[0x8D, 0xEE]);
        self.cmd_data(VMCTR1, &[0x0E]);
        self.cmd_data(
            GMCTRP1,
            &[
                0x10, 0x0E, 0x02, 0x03, 0x0E, 0x07, 0x02, 0x07, 0x0A, 0x12, 0x27, 0x37, 0x00, 0x0D,
                0x0E, 0x10,
            ],
        );
        self.cmd_data(
            GMCTRN1,
            &[
                0x10, 0x0E, 0x03, 0x03, 0x0F, 0x06, 0x02, 0x08, 0x0A, 0x13, 0x26, 0x36, 0x00, 0x0D,
                0x0E, 0x10,
            ],
        );
        // The IPS variant of this panel is wired inverted.
        self.cmd(INVON);
        self.cmd_data(COLMOD, &[0x05]); // 16-bit, RGB565
        self.cmd_data(MADCTL_CMD, &[MADCTL]);
        self.cmd(NORON);
        wait(10);
        self.cmd(DISPON);
        wait(100);
    }

    fn cmd(&mut self, cmd: u8) {
        self.tx(cmd, &[]);
    }

    fn cmd_data(&mut self, cmd: u8, data: &[u8]) {
        self.tx(cmd, data);
    }

    fn tx(&mut self, cmd: u8, data: &[u8]) {
        let (ptr, len) = if data.is_empty() {
            (core::ptr::null(), 0)
        } else {
            (data.as_ptr() as *const core::ffi::c_void, data.len())
        };
        let err = unsafe { esp_lcd_panel_io_tx_param(self.io, cmd as i32, ptr, len) };
        if err != esp_idf_svc::sys::ESP_OK {
            log::warn!("panel command {cmd:#04x} failed: {err}");
        }
    }

    fn window(&mut self, x: u16, y: u16, w: u16, h: u16) {
        let (x0, x1) = (x + X_GAP, x + X_GAP + w - 1);
        let (y0, y1) = (y + Y_GAP, y + Y_GAP + h - 1);
        self.cmd_data(
            CASET,
            &[(x0 >> 8) as u8, x0 as u8, (x1 >> 8) as u8, x1 as u8],
        );
        self.cmd_data(
            RASET,
            &[(y0 >> 8) as u8, y0 as u8, (y1 >> 8) as u8, y1 as u8],
        );
    }

    fn blit(&mut self, x: u16, y: u16, w: u16, h: u16) {
        if w == 0 || h == 0 {
            return;
        }
        let bytes = w as usize * h as usize * 2;
        debug_assert!(bytes <= SCRATCH_BYTES);
        self.window(x, y, w, h);
        let buf = unsafe { scratch() };
        self.tx(RAMWR, &buf[..bytes.min(SCRATCH_BYTES)]);
    }

    fn fill(&mut self, x: u16, y: u16, w: u16, h: u16, color: u16) {
        if w == 0 || h == 0 {
            return;
        }
        let rows = (SCRATCH_PX / w as usize).max(1).min(h as usize) as u16;
        paint(unsafe { scratch() }, w as usize * rows as usize, color);

        let mut done = 0;
        while done < h {
            let n = rows.min(h - done);
            self.blit(x, y + done, w, n);
            done += n;
        }
    }

    fn text(&mut self, x: u16, y: u16, w: u16, scale: u16, s: &str, fg: u16) {
        let h = font::H as u16 * scale;
        if y >= H || h == 0 || w == 0 {
            return;
        }
        let h = h.min(H - y);
        let width = w as usize;

        let buf = unsafe { scratch() };
        paint(buf, width * h as usize, BLACK);

        let chars = s.chars().count();
        let text_w = chars * font::ADVANCE * scale as usize;
        let text_w = text_w.saturating_sub(scale as usize);
        let x0 = (width.saturating_sub(text_w)) / 2;

        for (i, c) in s.chars().enumerate() {
            let gx = x0 + i * font::ADVANCE * scale as usize;
            draw_glyph(buf, width, h as usize, gx, c, scale as usize, fg);
        }

        self.blit(x, y, w, h);
    }

    pub fn paint(&mut self, state: State, d: &Detail) {
        if state == State::Off {
            self.fill(0, 0, W, H, BLACK);
            self.backlight(false);
            return;
        }

        self.backlight(true);
        self.fill(0, 0, W, H, BLACK);

        match state {
            State::Booting => {}
            State::ErrorIdle | State::ErrorBusy => self.error(d),
            State::HealthyIdle | State::WarningIdle | State::HealthyBusy | State::WarningBusy => {
                self.healthy(state, d)
            }
            State::Off => unreachable!(),
        }
    }

    const NAME_W: u16 = 132;
    const LOWER_X: u16 = 32;
    const LOWER_W: u16 = W - 2 * Self::LOWER_X;
    const LOWER_Y: u16 = 56;
    const ICON_BOX: u16 = 28;
    const ICON_Y: u16 = 46;
    fn healthy(&mut self, state: State, d: &Detail) {
        let name = if d.name.is_empty() {
            "BEAMER"
        } else {
            d.name.as_str()
        };

        self.text_upper(name, GREEN);
        self.wifi(d.net, d.weak_signal);
        if matches!(state, State::WarningIdle | State::WarningBusy) {
            self.warning_icon();
        }
        self.text_inner_lower(state, d);
    }

    /// upper text longer than 22 characters is elided
    fn text_upper(&mut self, s: &str, fg: u16) {
        let mut rows = ["", ""];
        let fit = text::fit(s, font::cols(Self::NAME_W as usize, 3), &mut rows[..1]);
        let (scale, fit) = if !fit.truncated && fit.used == 1 {
            (3u16, fit)
        } else {
            (
                2u16,
                text::fit(s, font::cols(Self::NAME_W as usize, 2), &mut rows),
            )
        };

        let step = font::H as u16 * scale + 2;
        let block = step.saturating_mul(fit.used as u16).saturating_sub(2);
        let mut y = (H / 2).saturating_sub(block) / 2;
        for (i, row) in rows[..fit.used].iter().enumerate() {
            let last = i + 1 == fit.used;
            if last && fit.truncated {
                let mut line = Line::new();
                line.push(row).push(ELLIPSIS);
                self.text(0, y, Self::NAME_W, scale, line.as_str(), fg);
            } else {
                self.text(0, y, Self::NAME_W, scale, row, fg);
            }
            y += step;
        }
    }

    /// lines longer than 16 characters are elided
    fn text_inner_lower(&mut self, state: State, d: &Detail) {
        let cols = font::cols(Self::LOWER_W as usize, 1);
        let fill;
        let (s, fg) = if matches!(state, State::HealthyBusy | State::WarningBusy) {
            ("DO NOT UNPLUG", WHITE)
        } else if let Some(warn) = d.warn {
            (warn.as_str(), AMBER)
        } else if let Some((files, cap)) = d.files {
            let pct = files
                .saturating_mul(100)
                .checked_div(cap)
                .unwrap_or(0)
                .min(100);
            let mut line = Line::new();
            line.push_num(pct).push("% full");
            fill = line;
            (fill.as_str(), GREY)
        } else {
            return;
        };

        let mut rows = [""];
        let fit = text::fit(s, cols, &mut rows);
        if fit.truncated {
            let mut line = Line::new();
            line.push(rows[0]).push(ELLIPSIS);
            self.text(
                Self::LOWER_X,
                Self::LOWER_Y,
                Self::LOWER_W,
                1,
                line.as_str(),
                fg,
            );
        } else {
            self.text(Self::LOWER_X, Self::LOWER_Y, Self::LOWER_W, 1, rows[0], fg);
        }
    }

    fn wifi(&mut self, net: Net, weak: bool) {
        const BOX_W: u16 = 28;
        const BOX_H: u16 = 16;
        let w = BOX_W as usize;
        let h = BOX_H as usize;

        const BARS: [(usize, usize); 3] = [(6, 5), (14, 10), (22, 15)]; // (left x, height)
        const BAR_W: usize = 4;
        let up = matches!(net, Net::Up(_));
        let lit = match (up, weak) {
            (false, _) => 0,
            (true, true) => 1,
            (true, false) => BARS.len(),
        };

        let buf = unsafe { scratch() };
        paint(buf, w * h, BLACK);
        for (i, &(x0, height)) in BARS.iter().enumerate() {
            let color = if i < lit { GREEN } else { GREY };
            for x in x0..x0 + BAR_W {
                for y in h - height..h {
                    put(buf, w, h, x, y, color);
                }
            }
        }
        if !up {
            for dx in 0..2 {
                line(buf, w, h, (6 + dx, 0), (12 + dx, 6), RED);
                line(buf, w, h, (12 + dx, 0), (6 + dx, 6), RED);
            }
        }

        self.blit(W - BOX_W, (H / 2 - BOX_H) / 2, BOX_W, BOX_H);
    }

    fn warning_icon(&mut self) {
        const TOP: usize = 4;
        const ROWS: usize = 19;
        const HALF_BASE: usize = 11;
        let box_px = Self::ICON_BOX as usize;
        let cx = box_px / 2;

        let buf = unsafe { scratch() };
        paint(buf, box_px * box_px, BLACK);
        for row in 0..ROWS {
            let half = row * HALF_BASE / (ROWS - 1);
            for x in cx - half..=cx + half {
                put(buf, box_px, box_px, x, TOP + row, AMBER);
            }
        }
        for y in TOP + 7..TOP + 13 {
            put(buf, box_px, box_px, cx - 1, y, BLACK);
            put(buf, box_px, box_px, cx, y, BLACK);
        }
        for y in TOP + 15..TOP + 17 {
            put(buf, box_px, box_px, cx - 1, y, BLACK);
            put(buf, box_px, box_px, cx, y, BLACK);
        }

        self.blit(2, Self::ICON_Y, Self::ICON_BOX, Self::ICON_BOX);
    }

    fn error(&mut self, d: &Detail) {
        self.text_upper(d.label.map_or("ERROR", |l| l.as_str()), RED);
        self.wifi(d.net, d.weak_signal);
        self.text_full_lower(d);
    }

    /// lines longer than 26 characters are wrapped, over at most three rows
    fn text_full_lower(&mut self, d: &Detail) {
        const STEP: u16 = font::H as u16 + 3;

        let mut rows = ["", "", ""];
        let fit = text::fit(&d.error_head, font::cols(W as usize, 1), &mut rows);
        let used = fit.used as u16 + u16::from(d.more > 0);

        let block = STEP.saturating_mul(used).saturating_sub(3);
        let mut y = H / 2 + (H / 2).saturating_sub(block) / 2;
        for (i, row) in rows[..fit.used].iter().enumerate() {
            if i + 1 == fit.used && fit.truncated {
                let mut line = Line::new();
                line.push(row).push(ELLIPSIS);
                self.text(0, y, W, 1, line.as_str(), WHITE);
            } else {
                self.text(0, y, W, 1, row, WHITE);
            }
            y += STEP;
        }

        if d.more > 0 {
            let mut line = Line::new();
            line.push("+").push_num(d.more).push(" more");
            self.text(0, y, W, 1, line.as_str(), GREY);
        }
    }

    fn spinner(&mut self, x0: u16, y0: u16, box_px: u16, r: i32, dot: i32, frame: u64) {
        let w = box_px as usize;
        let c = box_px as i32 / 2;

        let buf = unsafe { scratch() };
        paint(buf, w * w, BLACK);

        for i in 0..SPINNER_STEPS {
            let behind = (SPINNER_STEPS + frame - i) % SPINNER_STEPS;
            let color = match behind {
                0 => GREEN,
                1 => dim(170),
                2 => dim(110),
                3 => dim(60),
                _ => dim(20),
            };
            let cx = c + COS[i as usize] as i32 * r / 64;
            let cy = c + SIN[i as usize] as i32 * r / 64;
            disc(buf, w, w, cx, cy, dot, color);
        }

        self.blit(x0, y0, box_px, box_px);
    }

    pub fn boot_animation(&mut self, frame: u64) {
        let buf = unsafe { scratch() };
        buf[..boot_animation::FRAME_BYTES].copy_from_slice(boot_animation::frame(frame as usize));
        self.blit(
            (W - boot_animation::WIDTH) / 2,
            (H - boot_animation::HEIGHT) / 2,
            boot_animation::WIDTH,
            boot_animation::HEIGHT,
        );
    }

    pub fn busy_spinner(&mut self, frame: u64) {
        let box_px = Self::ICON_BOX;
        self.spinner(W - box_px - 2, Self::ICON_Y, box_px, 10, 2, frame);
    }

    pub fn set_flipped(&mut self, flipped: bool) {
        self.cmd_data(MADCTL_CMD, &[if flipped { MADCTL_FLIPPED } else { MADCTL }]);
    }

    fn backlight(&mut self, on: bool) {
        if self.lit == Some(on) {
            return;
        }
        match drive_backlight(&mut self.backlight, on) {
            Ok(()) => {
                self.lit = Some(on);
                log::info!("backlight {}", if on { "on" } else { "off" });
            }
            Err(e) => {
                self.lit = None;
                log::warn!("backlight: {e}");
            }
        }
    }
}

impl Lcd<'_> {
    #[allow(dead_code)]
    pub fn off(&mut self) {
        self.fill(0, 0, W, H, BLACK);
        self.backlight(false);
    }
}

fn paint(buf: &mut [u8], px: usize, color: u16) {
    let (hi, lo) = ((color >> 8) as u8, color as u8);
    let end = (px * 2).min(buf.len());
    for p in buf[..end].chunks_exact_mut(2) {
        p[0] = hi;
        p[1] = lo;
    }
}

fn put(buf: &mut [u8], w: usize, h: usize, x: usize, y: usize, color: u16) {
    if x >= w || y >= h {
        return;
    }
    let i = (y * w + x) * 2;
    if i + 1 < buf.len() {
        buf[i] = (color >> 8) as u8;
        buf[i + 1] = color as u8;
    }
}

fn draw_glyph(buf: &mut [u8], w: usize, h: usize, x: usize, c: char, scale: usize, color: u16) {
    let g = font::glyph(c);
    for (col, bits) in g.iter().enumerate() {
        for row in 0..font::H {
            if bits >> row & 1 == 0 {
                continue;
            }
            for dy in 0..scale {
                for dx in 0..scale {
                    put(buf, w, h, x + col * scale + dx, row * scale + dy, color);
                }
            }
        }
    }
}

fn line(buf: &mut [u8], w: usize, h: usize, from: (i32, i32), to: (i32, i32), color: u16) {
    let ((x0, y0), (x1, y1)) = (from, to);
    let steps = (x1 - x0).abs().max((y1 - y0).abs()).max(1);
    for i in 0..=steps {
        let x = x0 + (x1 - x0) * i / steps;
        let y = y0 + (y1 - y0) * i / steps;
        if x >= 0 && y >= 0 {
            put(buf, w, h, x as usize, y as usize, color);
        }
    }
}

fn disc(buf: &mut [u8], w: usize, h: usize, cx: i32, cy: i32, r: i32, color: u16) {
    for y in -r..=r {
        for x in -r..=r {
            if x * x + y * y <= r * r && cx + x >= 0 && cy + y >= 0 {
                put(buf, w, h, (cx + x) as usize, (cy + y) as usize, color);
            }
        }
    }
}

//throwback,,,
const COS: [i16; 12] = [64, 55, 32, 0, -32, -55, -64, -55, -32, 0, 32, 55];
const SIN: [i16; 12] = [0, 32, 55, 64, 55, 32, 0, -32, -55, -64, -55, -32];

fn wait(ms: u32) {
    std::thread::sleep(std::time::Duration::from_millis(ms as u64));
}
