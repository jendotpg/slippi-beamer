use std::sync::atomic::{AtomicBool, Ordering};

use esp_idf_svc::sys::{beamer_gz_begin, beamer_gz_end, beamer_gz_push, vTaskDelay};

use super::http::now_us;

pub const LEVEL_DEFAULT: i32 = 2;
const SLICE: usize = 1024;

pub struct Stream {
    have: usize,
    pub deflate_us: u32,
    pub deflate_max_us: u32,
}

static BUSY: AtomicBool = AtomicBool::new(false);

impl Stream {
    pub fn begin(level: i32) -> Option<Stream> {
        if BUSY.swap(true, Ordering::Acquire) {
            log::info!("gzip busy: serving this one uncompressed");
            return None;
        }

        if unsafe { beamer_gz_begin(level) } != 0 {
            log::error!("gzip would not start at level {level}: serving uncompressed");
            BUSY.store(false, Ordering::Release);
            return None;
        }

        Some(Stream {
            have: 0,
            deflate_us: 0,
            deflate_max_us: 0,
        })
    }

    pub fn push(
        &mut self,
        data: &[u8],
        out: &mut [u8],
        sink: &mut impl FnMut(&[u8]) -> anyhow::Result<()>,
    ) -> anyhow::Result<()> {
        for slice in data.chunks(SLICE) {
            self.pump(slice, false, out, sink)?;
            unsafe { vTaskDelay(0) };
        }
        Ok(())
    }

    pub fn finish(
        &mut self,
        out: &mut [u8],
        sink: &mut impl FnMut(&[u8]) -> anyhow::Result<()>,
    ) -> anyhow::Result<()> {
        self.pump(&[], true, out, sink)
    }

    fn pump(
        &mut self,
        mut data: &[u8],
        finish: bool,
        out: &mut [u8],
        sink: &mut impl FnMut(&[u8]) -> anyhow::Result<()>,
    ) -> anyhow::Result<()> {
        let mut idle = 0;

        loop {
            let mut used = 0usize;
            let mut made = 0usize;
            let room = out.len() - self.have;

            let t0 = now_us();
            let r = unsafe {
                beamer_gz_push(
                    data.as_ptr(),
                    data.len(),
                    &mut used,
                    out.as_mut_ptr().add(self.have),
                    room,
                    &mut made,
                    i32::from(finish),
                )
            };
            let took = (now_us() - t0) as u32;
            self.deflate_us += took;
            self.deflate_max_us = self.deflate_max_us.max(took);

            if r < 0 {
                anyhow::bail!("deflate failed");
            }

            self.have += made;
            data = &data[used..];

            if self.have == out.len() {
                sink(&out[..self.have])?;
                self.have = 0;
            }

            if r == 1 {
                break;
            }
            if !finish && data.is_empty() {
                break;
            }

            idle = if used == 0 && made == 0 { idle + 1 } else { 0 };
            if idle > 2 {
                anyhow::bail!("deflate stalled with {} B still to write", data.len());
            }
        }

        if finish && self.have > 0 {
            sink(&out[..self.have])?;
            self.have = 0;
        }
        Ok(())
    }
}

impl Drop for Stream {
    fn drop(&mut self) {
        unsafe { beamer_gz_end() }
        BUSY.store(false, Ordering::Release);
    }
}

pub fn accepted(header: Option<&str>) -> bool {
    let Some(raw) = header else {
        return false;
    };

    raw.split(',').any(|part| {
        let mut fields = part.split(';').map(str::trim);
        if !fields
            .next()
            .is_some_and(|t| t.eq_ignore_ascii_case("gzip"))
        {
            return false;
        }
        !fields.any(|f| {
            let Some((k, v)) = f.split_once('=') else {
                return false;
            };
            k.eq_ignore_ascii_case("q") && v.parse::<f32>().is_ok_and(|q| q <= 0.0)
        })
    })
}

pub fn level(header: Option<&str>) -> i32 {
    header
        .and_then(|v| v.trim().parse::<i32>().ok())
        .map_or(LEVEL_DEFAULT, |n| n.clamp(1, 9))
}
