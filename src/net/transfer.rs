use std::ffi::{c_char, c_void, CStr};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};

use esp_idf_svc::hal::cpu::Core;
use esp_idf_svc::hal::task::thread::ThreadSpawnConfiguration;
use esp_idf_svc::handle::RawHandle;
use esp_idf_svc::http::server::EspHttpServer;
use esp_idf_svc::sys::{
    esp_err_t, httpd_register_uri_handler, httpd_req_async_handler_begin,
    httpd_req_async_handler_complete, httpd_req_get_hdr_value_len, httpd_req_get_hdr_value_str,
    httpd_req_t, httpd_resp_send, httpd_resp_send_chunk, httpd_resp_set_hdr, httpd_resp_set_status,
    httpd_resp_set_type, httpd_uri_t, ESP_OK,
};

use crate::scan;
use crate::storage::SdCard;

use super::gz;
use super::http;

const STACK: usize = 6144;
const MAX_NAME: usize = 96;
const MAX_HDR: usize = 128;

pub struct Job {
    req: *mut httpd_req_t,
    name: heapless::String<MAX_NAME>,
    start: u64,
    ranged: bool,
    resumed: bool,
    gzip: bool,
    level: i32,
    _transfer: super::Transfer,
}

unsafe impl Send for Job {}

static SLOT: Mutex<Option<Job>> = Mutex::new(None);
static WAKE: Condvar = Condvar::new();
static STOP: AtomicBool = AtomicBool::new(false);

static BUSY: AtomicBool = AtomicBool::new(false);

pub fn spawn(card: Arc<SdCard>) -> anyhow::Result<std::thread::JoinHandle<()>> {
    STOP.store(false, Ordering::SeqCst);
    ThreadSpawnConfiguration {
        name: Some(c"transfer"),
        stack_size: STACK,
        priority: 4,
        pin_to_core: Some(Core::Core0),
        ..Default::default()
    }
    .set()?;
    let h = std::thread::Builder::new()
        .stack_size(STACK)
        .spawn(move || worker(card))?;
    ThreadSpawnConfiguration::default().set()?;
    Ok(h)
}

pub fn shutdown(handle: Option<std::thread::JoinHandle<()>>) {
    STOP.store(true, Ordering::SeqCst);
    WAKE.notify_all();
    if let Some(h) = handle {
        if h.join().is_err() {
            log::error!("transfer worker panicked on the way down");
        }
    }
}

pub fn busy() -> bool {
    BUSY.load(Ordering::SeqCst)
}

fn worker(card: Arc<SdCard>) {
    log::info!("transfer worker up");
    loop {
        let job = {
            let mut slot = SLOT.lock().unwrap_or_else(|e| e.into_inner());
            while slot.is_none() && !STOP.load(Ordering::SeqCst) {
                slot = WAKE.wait(slot).unwrap_or_else(|e| e.into_inner());
            }
            match slot.take() {
                Some(j) => j,
                None => break,
            }
        };

        let raw = job.req;
        if let Err(e) = run(&card, &job) {
            log::error!("{}: transfer failed: {e}", job.name);
        }
        unsafe { httpd_req_async_handler_complete(raw) };
        drop(job);
        BUSY.store(false, Ordering::SeqCst);
    }
    log::info!("transfer worker down");
}

fn enqueue(job: Job) -> Result<(), Job> {
    let mut slot = SLOT.lock().unwrap_or_else(|e| e.into_inner());
    if slot.is_some() || BUSY.swap(true, Ordering::SeqCst) {
        return Err(job);
    }
    *slot = Some(job);
    WAKE.notify_one();
    Ok(())
}

const S_200: &CStr = c"200 OK";
const S_206: &CStr = c"206 Partial Content";
const S_404: &CStr = c"404 Not Found";
const S_416: &CStr = c"416 Range Not Satisfiable";
const S_500: &CStr = c"500 Internal Server Error";
const S_503: &CStr = c"503 Service Unavailable";

const H_OCTET: &CStr = c"application/octet-stream";
const H_JSON: &CStr = c"application/json";

struct RawResponse(*mut httpd_req_t);

fn check(rc: esp_err_t) -> anyhow::Result<()> {
    if rc == ESP_OK {
        Ok(())
    } else {
        anyhow::bail!("esp_err {rc}")
    }
}

impl RawResponse {
    fn status(&self, s: &'static CStr) {
        unsafe { httpd_resp_set_status(self.0, s.as_ptr()) };
    }

    fn ctype(&self, t: &'static CStr) {
        unsafe { httpd_resp_set_type(self.0, t.as_ptr()) };
    }

    fn hdr(&self, k: &'static CStr, v: &CStr) {
        unsafe { httpd_resp_set_hdr(self.0, k.as_ptr(), v.as_ptr()) };
    }

    fn chunk(&self, b: &[u8]) -> anyhow::Result<()> {
        check(unsafe {
            httpd_resp_send_chunk(self.0, b.as_ptr() as *const c_char, b.len() as isize)
        })
    }

    fn finish(&self) -> anyhow::Result<()> {
        self.chunk(&[])
    }

    fn send(&self, status: &'static CStr, ctype: &'static CStr, body: &[u8]) -> esp_err_t {
        self.status(status);
        self.ctype(ctype);
        unsafe { httpd_resp_send(self.0, body.as_ptr() as *const c_char, body.len() as isize) }
    }
}

fn run(card: &SdCard, job: &Job) -> anyhow::Result<()> {
    use std::io::{Read as _, Seek as _, SeekFrom};

    let resp = RawResponse(job.req);
    let t_start = http::now_us();

    let opened = match crate::storage::fat::ReadWindow::try_open_measured(card) {
        Ok(Some(w)) => w,
        Ok(None) => {
            log::warn!("{}: the RO lock is held; refusing", job.name);
            resp.hdr(c"Retry-After", http::RETRY_AFTER_SECONDS);
            resp.send(S_503, H_JSON, http::ERR_VOLUME);
            return Ok(());
        }
        Err(e) => {
            log::error!("{}: could not mount read-only: {e}", job.name);
            resp.hdr(c"Retry-After", http::RETRY_AFTER_SECONDS);
            resp.send(S_503, H_JSON, http::ERR_VOLUME);
            return Ok(());
        }
    };
    let (window, open) = opened;

    let path = window.path(&format!("SLIPPI/{}", job.name));
    let mut file = match std::fs::File::open(&path) {
        Ok(f) => f,
        Err(e) => {
            log::warn!("{path}: {e}");
            resp.send(S_404, H_JSON, http::ERR_NOT_FOUND);
            return Ok(());
        }
    };

    let len = match file.metadata() {
        Ok(m) => m.len(),
        Err(e) => {
            log::error!("{path}: could not stat: {e}");
            resp.send(S_500, H_JSON, http::ERR_STAT);
            return Ok(());
        }
    };
    if job.start >= len {
        let cr = std::ffi::CString::new(format!("bytes */{len}"))?;
        resp.status(S_416);
        resp.ctype(H_JSON);
        resp.hdr(c"Content-Range", &cr);
        unsafe {
            httpd_resp_send(
                resp.0,
                http::ERR_RANGE.as_ptr() as *const c_char,
                http::ERR_RANGE.len() as isize,
            )
        };
        return Ok(());
    }

    let mut stream = if job.gzip {
        gz::Stream::begin(job.level)
    } else {
        None
    };
    let gzip = stream.is_some();

    if job.start > 0 {
        file.seek(SeekFrom::Start(job.start))?;
    }

    let content_range = std::ffi::CString::new(format!("bytes {}-{}/{len}", job.start, len - 1))?;
    let from_echo = std::ffi::CString::new(job.start.to_string())?;

    resp.status(if job.ranged { S_206 } else { S_200 });
    resp.ctype(H_OCTET);
    if job.ranged {
        resp.hdr(c"Accept-Ranges", c"bytes");
        resp.hdr(c"Content-Range", &content_range);
    } else if gzip {
        resp.hdr(c"Content-Encoding", c"gzip");
        resp.hdr(c"Vary", c"Accept-Encoding, X-Replay-From");
    } else {
        resp.hdr(c"Accept-Ranges", c"bytes");
    }
    if job.resumed && !job.ranged {
        resp.hdr(c"X-Replay-From", &from_echo);
    }

    let mut scratch = http::SCRATCH.lock().unwrap_or_else(|e| e.into_inner());
    let http::Scratch { read: buf, out } = &mut *scratch;

    crate::storage::msc::read_wait_reset();

    let mut sent = 0u64;
    let mut write_us = 0u32;
    let mut write_max_us = 0u32;
    let mut bytes = 0u64;
    let mut chunks = 0u32;
    let mut read_us = 0u32;
    let mut read_max_us = 0u32;

    {
        let mut sink = |block: &[u8]| -> anyhow::Result<()> {
            let t = http::now_us();
            resp.chunk(block)?;
            let took = (http::now_us() - t) as u32;
            write_us += took;
            write_max_us = write_max_us.max(took);
            sent += block.len() as u64;
            Ok(())
        };

        loop {
            let t0 = http::now_us();
            let n = match file.read(buf) {
                Ok(0) => break,
                Ok(n) => n,
                Err(e) => {
                    log::error!("{path}: read failed after the header: {e}");
                    break;
                }
            };
            let took = (http::now_us() - t0) as u32;
            read_us += took;
            read_max_us = read_max_us.max(took);

            match stream.as_mut() {
                Some(gz) => gz.push(&buf[..n], out, &mut sink)?,
                None => sink(&buf[..n])?,
            }

            chunks += 1;
            bytes += n as u64;
        }

        if let Some(gz) = stream.as_mut() {
            gz.finish(out, &mut sink)?;
        }
    }

    let (deflate_us, deflate_max_us) = stream
        .as_ref()
        .map_or((0, 0), |gz| (gz.deflate_us, gz.deflate_max_us));

    let mut stats = http::TransferStats {
        bytes,
        sent_bytes: sent,
        chunks,
        gzip,
        level: if gzip { job.level } else { 0 },
        read_us,
        read_max_us,
        write_us,
        write_max_us,
        deflate_us,
        deflate_max_us,
        lock_us: open.lock_us,
        mount_us: open.mount_us,
        stack_left: unsafe { esp_idf_svc::sys::uxTaskGetStackHighWaterMark(std::ptr::null_mut()) },
        ..http::TransferStats::default()
    };

    resp.finish()?;

    (stats.sd_wait_us, stats.sd_wait_max_us) = crate::storage::msc::read_wait();
    stats.total_us = (http::now_us() - t_start) as u32;
    http::publish_stats(stats);
    crate::journal::heap_checkin();
    Ok(())
}

fn header(r: *mut httpd_req_t, key: &CStr) -> Option<heapless::String<MAX_HDR>> {
    let len = unsafe { httpd_req_get_hdr_value_len(r, key.as_ptr()) };
    if len == 0 || len >= MAX_HDR {
        return None;
    }
    let mut buf = [0u8; MAX_HDR];
    let rc = unsafe {
        httpd_req_get_hdr_value_str(r, key.as_ptr(), buf.as_mut_ptr() as *mut c_char, MAX_HDR)
    };
    if rc != ESP_OK {
        return None;
    }
    let s = CStr::from_bytes_until_nul(&buf).ok()?.to_str().ok()?;
    heapless::String::try_from(s).ok()
}

unsafe extern "C" fn handle(r: *mut httpd_req_t) -> esp_err_t {
    let resp = RawResponse(r);

    let Ok(uri) = CStr::from_bytes_until_nul(&(*r).uri)
        .map_err(|_| ())
        .and_then(|c| c.to_str().map_err(|_| ()))
    else {
        return resp.send(S_404, H_JSON, http::ERR_NOT_FOUND);
    };
    let Some(name) = http::replay_name(uri) else {
        log::warn!("refused {uri:?}: not a replay name");
        return resp.send(S_404, H_JSON, http::ERR_NOT_FOUND);
    };
    let Ok(name) = heapless::String::<MAX_NAME>::try_from(name) else {
        return resp.send(S_404, H_JSON, http::ERR_NOT_FOUND);
    };
    let Some(indexed_len) = scan::published_size(&name) else {
        log::info!("refused {name}: not published");
        return resp.send(S_404, H_JSON, http::ERR_NOT_FOUND);
    };

    if let Some(short) = super::heap_too_low() {
        match short {
            super::HeapShort::Free(free) => log::error!(
                "refusing {name}: {free} B free is under the {} B floor",
                super::HEAP_FLOOR
            ),
            super::HeapShort::Fragmented(block) => log::error!(
                "refusing {name}: largest free block {block} B is under the {} B floor",
                super::BLOCK_FLOOR
            ),
        }
        resp.hdr(c"Retry-After", http::RETRY_AFTER_SECONDS);
        return resp.send(S_503, H_JSON, http::ERR_LOW_MEMORY);
    }

    if super::transfers_in_flight() > 0 || busy() {
        log::info!("refusing {name}: already serving a replay");
        resp.hdr(c"Retry-After", http::RETRY_AFTER_SECONDS);
        return resp.send(S_503, H_JSON, http::ERR_SERVING);
    }

    let range = http::parse_range(header(r, c"Range").as_deref());
    let ranged = matches!(range, http::RangeReq::From(_));
    let resume = match range {
        http::RangeReq::None => http::parse_from(header(r, c"X-Replay-From").as_deref()),
        _ => http::Resume::None,
    };
    let resumed = matches!(resume, http::Resume::At(_));

    let want = match (&range, &resume) {
        (http::RangeReq::Bad, _) | (_, http::Resume::Bad) => None,
        (http::RangeReq::From(n), _) | (http::RangeReq::None, http::Resume::At(n)) => {
            (*n < indexed_len).then_some(*n)
        }
        (http::RangeReq::None, http::Resume::None) => Some(0),
    };
    let Some(start) = want else {
        let Ok(cr) = std::ffi::CString::new(format!("bytes */{indexed_len}")) else {
            return resp.send(S_500, H_JSON, http::ERR_STAT);
        };
        resp.status(S_416);
        resp.ctype(H_JSON);
        resp.hdr(c"Accept-Ranges", c"bytes");
        resp.hdr(c"Content-Range", &cr);
        return httpd_resp_send(
            r,
            http::ERR_RANGE.as_ptr() as *const c_char,
            http::ERR_RANGE.len() as isize,
        );
    };

    let level = if http::debug_enabled() {
        gz::level(header(r, c"X-Beamer-Gz-Level").as_deref())
    } else {
        gz::LEVEL_DEFAULT
    };
    let gzip = !ranged && gz::accepted(header(r, c"Accept-Encoding").as_deref());

    let mut async_req: *mut httpd_req_t = std::ptr::null_mut();
    if httpd_req_async_handler_begin(r, &mut async_req) != ESP_OK || async_req.is_null() {
        log::error!("refusing {name}: could not take the request async");
        resp.hdr(c"Retry-After", http::RETRY_AFTER_SECONDS);
        return resp.send(S_503, H_JSON, http::ERR_LOW_MEMORY);
    }

    let job = Job {
        req: async_req,
        name,
        start,
        ranged,
        resumed,
        gzip,
        level,
        _transfer: super::Transfer::begin(),
    };

    if let Err(job) = enqueue(job) {
        let async_req = job.req;
        drop(job);
        let late = RawResponse(async_req);
        late.hdr(c"Retry-After", http::RETRY_AFTER_SECONDS);
        late.send(S_503, H_JSON, http::ERR_SERVING);
        httpd_req_async_handler_complete(async_req);
    }
    ESP_OK
}

pub fn register(server: &EspHttpServer<'static>) -> anyhow::Result<()> {
    let uri = c"/SLIPPI/*";
    let cfg = httpd_uri_t {
        uri: uri.as_ptr(),
        method: esp_idf_svc::sys::http_method_HTTP_GET,
        handler: Some(handle),
        user_ctx: std::ptr::null_mut::<c_void>(),
    };
    check(unsafe { httpd_register_uri_handler(server.handle(), &cfg) })
}
