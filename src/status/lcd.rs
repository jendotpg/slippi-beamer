use esp_idf_svc::hal::gpio::{AnyIOPin, Output, Pin, PinDriver};
use esp_idf_svc::hal::spi::{Dma, SpiDriver, SpiDriverConfig};
use esp_idf_svc::sys::{
    esp, esp_lcd_new_panel_io_spi, esp_lcd_panel_io_handle_t, esp_lcd_panel_io_spi_config_t,
    esp_lcd_panel_io_tx_param, spi_host_device_t_SPI2_HOST, EspError,
};

use crate::text::{self, ELLIPSIS};

use super::font;
use super::{Detail, LcdPins, Net, State, SPINNER_STEPS};

pub const W: u16 = 160;
pub const H: u16 = 80;
const X_GAP: u16 = 1;
const Y_GAP: u16 = 26;
const MADCTL: u8 = 0x60;
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
const GREEN: u16 = rgb565(0, 255, 0);
const RED: u16 = rgb565(255, 0, 0);
const AMBER_RGB: (u8, u8, u8) = (255, 140, 0);
const AMBER: u16 = rgb565(AMBER_RGB.0, AMBER_RGB.1, AMBER_RGB.2);

const fn dim(level: u32) -> u16 {
    rgb565(
        (AMBER_RGB.0 as u32 * level / 255) as u8,
        (AMBER_RGB.1 as u32 * level / 255) as u8,
        (AMBER_RGB.2 as u32 * level / 255) as u8,
    )
}

const SCRATCH_PX: usize = W as usize * 24;
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

    fn text(&mut self, y: u16, scale: u16, s: &str, fg: u16, bg: u16) {
        let h = font::H as u16 * scale;
        if y >= H || h == 0 {
            return;
        }
        let h = h.min(H - y);
        let w = W as usize;

        let buf = unsafe { scratch() };
        paint(buf, w * h as usize, bg);

        let chars = s.chars().count();
        let text_w = chars * font::ADVANCE * scale as usize;
        let text_w = text_w.saturating_sub(scale as usize);
        let x0 = (w.saturating_sub(text_w)) / 2;

        for (i, c) in s.chars().enumerate() {
            let gx = x0 + i * font::ADVANCE * scale as usize;
            draw_glyph(buf, w, h as usize, gx, c, scale as usize, fg);
        }

        self.blit(0, y, W, h);
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
            State::Idle => self.idle(d),
            State::Error => self.error(d),
            State::Busy => self.busy(d),
            State::Off => unreachable!(),
        }
    }

    const BUSY_Y: u16 = 8;
    const BUSY_Y2: u16 = 26;

    fn busy(&mut self, d: &Detail) {
        self.dots(d, 1);

        self.text(46, 1, "DO NOT UNPLUG", WHITE, BLACK);

        if !d.name.is_empty() {
            let mut rows = [""];
            let fit = text::fit(&d.name, font::cols(W as usize, 1), &mut rows);
            if fit.truncated {
                let mut line = Line::new();
                line.push(rows[0]).push(ELLIPSIS);
                self.text(64, 1, line.as_str(), GREY, BLACK);
            } else {
                self.text(64, 1, rows[0], GREY, BLACK);
            }
        }
    }

    pub fn dots(&mut self, d: &Detail, frame: u64) {
        let mut used = 0usize;
        for (active, verb) in [(d.writing, "WRITING"), (d.sending, "SENDING")] {
            if !active {
                continue;
            }
            let mut line = Line::new();
            line.push(verb);
            for i in 0..super::DOTS_MAX {
                line.push(if i < frame { "." } else { " " });
            }
            let y = if used == 0 {
                Self::BUSY_Y
            } else {
                Self::BUSY_Y2
            };
            self.text(y, 2, line.as_str(), AMBER, BLACK);
            used += 1;
        }

        if used == 0 {
            self.text(Self::BUSY_Y, 2, " ", AMBER, BLACK);
        }
        if used <= 1 {
            self.text(Self::BUSY_Y2, 2, " ", AMBER, BLACK);
        }
    }

    fn idle(&mut self, d: &Detail) {
        let name = if d.name.is_empty() {
            "BEAMER"
        } else {
            d.name.as_str()
        };

        let mut rows = ["", ""];
        let big = font::cols(W as usize, 3);
        let fit = text::fit(name, big, &mut rows[..1]);
        let (scale, fit) = if !fit.truncated && fit.used == 1 {
            (3u16, fit)
        } else {
            (2u16, text::fit(name, font::cols(W as usize, 2), &mut rows))
        };

        let mut y = 8;
        for (i, row) in rows[..fit.used].iter().enumerate() {
            let last = i + 1 == fit.used;
            if last && fit.truncated {
                let mut line = Line::new();
                line.push(row).push(ELLIPSIS);
                self.text(y, scale, line.as_str(), GREEN, BLACK);
            } else {
                self.text(y, scale, row, GREEN, BLACK);
            }
            y += font::H as u16 * scale + 2;
        }

        // `Net::Offline` and an unknown replay count render nothing at all.
        // A station that has not looked yet must not claim it has no network.
        match d.net {
            Net::Offline => {}
            Net::NotSet => self.text(52, 1, "no network", GREY, BLACK),
            Net::Up(ip) => {
                let mut line = Line::new();
                let o = ip.octets();
                line.push_num(o[0] as u32);
                for b in &o[1..] {
                    line.push(".").push_num(*b as u32);
                }
                self.text(52, 1, line.as_str(), GREY, BLACK);
            }
        }

        if let Some(n) = d.replays {
            let mut line = Line::new();
            line.push_num(n)
                .push(if n == 1 { " replay" } else { " replays" });
            self.text(64, 1, line.as_str(), GREY, BLACK);
        }
    }

    fn error(&mut self, d: &Detail) {
        let label = d.label.map_or("ERROR", |l| l.as_str());
        self.text(6, 2, label, RED, BLACK);

        let cols = font::cols(W as usize, 1);
        let mut rows = ["", ""];
        let fit = text::fit(&d.head, cols, &mut rows);
        let mut y = 26;
        for (i, row) in rows[..fit.used].iter().enumerate() {
            if i + 1 == fit.used && fit.truncated {
                let mut line = Line::new();
                line.push(row).push(ELLIPSIS);
                self.text(y, 1, line.as_str(), WHITE, BLACK);
            } else {
                self.text(y, 1, row, WHITE, BLACK);
            }
            y += font::H as u16 + 3;
        }

        if d.more > 0 {
            let mut line = Line::new();
            line.push("+").push_num(d.more).push(" more");
            self.text(54, 1, line.as_str(), GREY, BLACK);
        }
        self.text(66, 1, "see LOGS/error.txt", GREY, BLACK);
    }

    pub fn spinner(&mut self, frame: u64) {
        const BOX: u16 = 56;
        const R: i32 = 22;
        const DOT: i32 = 3;
        let x0 = (W - BOX) / 2;
        let y0 = (H - BOX) / 2;
        let c = BOX as i32 / 2;

        let buf = unsafe { scratch() };
        paint(buf, BOX as usize * BOX as usize, BLACK);

        for i in 0..SPINNER_STEPS {
            let behind = (SPINNER_STEPS + frame - i) % SPINNER_STEPS;
            let color = match behind {
                0 => AMBER,
                1 => dim(170),
                2 => dim(110),
                3 => dim(60),
                _ => dim(20),
            };
            let cx = c + COS[i as usize] as i32 * R / 64;
            let cy = c + SIN[i as usize] as i32 * R / 64;
            disc(buf, BOX as usize, BOX as usize, cx, cy, DOT, color);
        }

        self.blit(x0, y0, BOX, BOX);
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
