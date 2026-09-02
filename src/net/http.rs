use std::io::Read as _;
use std::io::{Seek as _, SeekFrom};
use std::sync::{Arc, Mutex};

use esp_idf_svc::hal::cpu::Core;
use esp_idf_svc::http::server::{Configuration, EspHttpServer};
use esp_idf_svc::http::Method;
use esp_idf_svc::io::Write as _;

use crate::errors;
use crate::publish::is_replay_name;
use crate::report;
use crate::scan;
use crate::storage::fat::ReadWindow;
use crate::storage::{volume, SdCard};

static API_LOCK: Mutex<()> = Mutex::new(());

const CHUNK: usize = 8 * 1024;
#[repr(align(64))]
struct SendBuf([u8; CHUNK]);

static SEND_BUF: Mutex<SendBuf> = Mutex::new(SendBuf([0; CHUNK]));

#[derive(Debug, Clone, Copy, Default)]
struct TransferStats {
    bytes: u64,
    chunks: u32,
    read_us: u32,
    read_max_us: u32,
    write_us: u32,
    write_max_us: u32,
    lock_us: u32,
    mount_us: u32,
    sd_wait_us: u32,
    sd_wait_max_us: u32,
    total_us: u32,
}

static LAST: Mutex<Option<TransferStats>> = Mutex::new(None);

fn publish_stats(s: TransferStats) {
    *LAST.lock().unwrap_or_else(|e| e.into_inner()) = Some(s);
    log::info!(
        "transfer: {} B in {} us ({} chunks) read {} us (max {}) write {} us (max {}) \
         ro_lock {} us mount {} us sd_wait {} us (max {})",
        s.bytes,
        s.total_us,
        s.chunks,
        s.read_us,
        s.read_max_us,
        s.write_us,
        s.write_max_us,
        s.lock_us,
        s.mount_us,
        s.sd_wait_us,
        s.sd_wait_max_us
    );
}

fn now_us() -> i64 {
    unsafe { esp_idf_svc::sys::esp_timer_get_time() }
}

fn debug_enabled() -> bool {
    crate::journal::enabled() // DEBUG and the journal being enabled are the same :3
}

const SLIPPI_PREFIX_GLOB: &str = "/SLIPPI/*";
const SLIPPI_PREFIX: &str = "/SLIPPI/";

pub fn serve(sd: Arc<SdCard>) -> anyhow::Result<EspHttpServer<'static>> {
    let mut server = EspHttpServer::new(&Configuration {
        http_port: 80,
        core: Some(Core::Core0),
        stack_size: 8192, // determined experimentally - lower panics...
        uri_match_wildcard: true,
        ..Default::default()
    })?;

    server.fn_handler::<anyhow::Error, _>("/", Method::Get, |req| {
        req.into_status_response(403)?.write_all(b"forbidden\n")?;
        Ok(())
    })?;

    server.fn_handler::<anyhow::Error, _>("/status", Method::Get, |req| {
        respond_json(req, 200, status_body().as_bytes())
    })?;

    server.fn_handler::<anyhow::Error, _>("/status", Method::Post, |req| {
        let Ok(_guard) = API_LOCK.try_lock() else {
            return respond_json(req, 409, ERR_BUSY);
        };
        scan::refresh();
        respond_json(req, 200, status_body().as_bytes())
    })?;

    server.fn_handler::<anyhow::Error, _>(SLIPPI_PREFIX, Method::Get, |req| {
        respond_json(req, 200, &scan::index_json())
    })?;

    if debug_enabled() {
        server.fn_handler::<anyhow::Error, _>("/debug/transfer", Method::Get, |req| {
            respond_json(req, 200, last_transfer_json().as_bytes())
        })?;

        server
            .fn_handler::<anyhow::Error, _>("/debug/zeros", Method::Get, |req| send_zeros(req))?;

        log::info!("debug endpoints on: /debug/transfer /debug/zeros");
    }

    let card = sd;

    let reset_card = card.clone();
    server.fn_handler::<anyhow::Error, _>("/reset-beamer", Method::Post, move |req| {
        if req.header("X-Beamer-Confirm") != Some("reset") {
            return respond_json(req, 400, ERR_CONFIRM);
        }
        let Ok(_guard) = API_LOCK.try_lock() else {
            return respond_json(req, 409, ERR_BUSY);
        };
        if super::transfers_in_flight() > 0 {
            return respond_json(req, 409, ERR_SERVING);
        }
        if scan::game_live() {
            return respond_json(req, 409, ERR_GAME_LIVE);
        }
        match volume::wipe_replays(&reset_card) {
            Ok(n) => {
                scan::forget_all();
                scan::refresh();
                log::warn!("reset: {n} replay(s) erased");
                respond_json(req, 200, br#"{"ok": true, "message": "reset OK"}"#)
            }
            Err(e) => {
                log::error!("reset failed: {e}");
                let body = error_body(&format!("the replay drive could not be wiped: {e}"));
                respond_json(req, 500, body.as_bytes())
            }
        }
    })?;

    server.fn_handler::<anyhow::Error, _>(SLIPPI_PREFIX_GLOB, Method::Get, move |req| {
        let Some(name) = replay_name(req.uri()) else {
            log::warn!("refused {:?}: not a replay name", req.uri());
            return respond(req, 404, b"not found\n");
        };

        if !scan::is_published(name) {
            log::info!("refused {name}: not published");
            return respond(req, 404, b"not found\n");
        }

        let name = name.to_owned();
        send_replay(req, &card, &name)
    })?;

    log::info!("http listening on :80");
    Ok(server)
}

fn replay_name(uri: &str) -> Option<&str> {
    let path = uri.split('?').next().unwrap_or(uri);
    let name = path.strip_prefix(SLIPPI_PREFIX)?;
    is_replay_name(name).then_some(name)
}

enum RangeReq {
    None,
    From(u64),
    Bad,
}

fn parse_range(header: Option<&str>) -> RangeReq {
    let Some(raw) = header else {
        return RangeReq::None;
    };
    let Some(spec) = raw.trim().strip_prefix("bytes=") else {
        return RangeReq::Bad;
    };
    let Some(start) = spec.trim().strip_suffix('-') else {
        return RangeReq::Bad;
    };
    match start.trim().parse::<u64>() {
        Ok(n) => RangeReq::From(n),
        Err(_) => RangeReq::Bad,
    }
}

fn send_replay<C>(
    req: esp_idf_svc::http::server::Request<C>,
    sd: &SdCard,
    name: &str,
) -> anyhow::Result<()>
where
    C: esp_idf_svc::http::server::Connection,
    C::Error: std::error::Error + Send + Sync + 'static,
{
    let _transfer = super::Transfer::begin();
    let t_start = now_us();

    let (window, open) = match ReadWindow::open_measured(sd) {
        Ok(w) => w,
        Err(e) => {
            log::error!(
                "{name}: could not mount the volume read-only: {e} ({})",
                crate::journal::heap_note()
            ); //not an error - something else could currently hold the RO lock
            return respond(req, 503, b"volume unavailable\n");
        }
    };

    let path = window.path(&format!("SLIPPI/{name}"));
    let mut file = match std::fs::File::open(&path) {
        Ok(f) => f,
        Err(e) => {
            log::warn!("{path}: {e}");
            return respond(req, 404, b"not found\n");
        }
    };

    let len = match file.metadata() {
        Ok(m) => m.len(),
        Err(e) => {
            log::error!("{path}: could not stat: {e}");
            return respond(req, 500, b"could not stat that replay\n");
        }
    };

    let range = parse_range(req.header("Range"));
    let ranged = matches!(range, RangeReq::From(_));
    let start = match range {
        RangeReq::None => 0,
        RangeReq::From(n) if n < len => n,
        _ => {
            let content_range = format!("bytes */{len}");
            let mut resp = req.into_response(
                416,
                None,
                &[
                    ("Content-Range", content_range.as_str()),
                    ("Accept-Ranges", "bytes"),
                ],
            )?;
            resp.write_all(b"range not satisfiable\n")?;
            resp.flush()?;
            return Ok(());
        }
    };

    if start > 0 {
        if let Err(e) = file.seek(SeekFrom::Start(start)) {
            log::error!("{path}: seek to {start} failed: {e}");
            return respond(req, 500, b"could not seek that replay\n");
        }
    }

    let mut resp = if ranged {
        let content_range = format!("bytes {start}-{}/{len}", len - 1);
        req.into_response(
            206,
            None,
            &[
                ("Content-Type", "application/octet-stream"),
                ("Accept-Ranges", "bytes"),
                ("Content-Range", content_range.as_str()),
            ],
        )?
    } else {
        req.into_response(
            200,
            None,
            &[
                ("Content-Type", "application/octet-stream"),
                ("Accept-Ranges", "bytes"),
            ],
        )?
    };
    let mut buf = SEND_BUF.lock().unwrap_or_else(|e| e.into_inner());
    let buf = &mut buf.0;

    let mut stats = TransferStats {
        lock_us: open.lock_us,
        mount_us: open.mount_us,
        ..TransferStats::default()
    };
    crate::storage::msc::read_wait_reset();

    loop {
        let t0 = now_us();
        let n = match file.read(buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) => {
                log::error!("{path}: read failed after the header: {e}");
                break;
            }
        };
        let t1 = now_us();
        resp.write_all(&buf[..n])?;
        let t2 = now_us();

        let read_us = (t1 - t0) as u32;
        let write_us = (t2 - t1) as u32;
        stats.read_us += read_us;
        stats.write_us += write_us;
        stats.read_max_us = stats.read_max_us.max(read_us);
        stats.write_max_us = stats.write_max_us.max(write_us);
        stats.chunks += 1;
        stats.bytes += n as u64;
    }
    resp.flush()?;

    (stats.sd_wait_us, stats.sd_wait_max_us) = crate::storage::msc::read_wait();
    stats.total_us = (now_us() - t_start) as u32;
    publish_stats(stats);
    Ok(())
}

fn query_num(uri: &str, key: &str, default: usize, max: usize) -> usize {
    let Some(query) = uri.split('?').nth(1) else {
        return default;
    };
    for pair in query.split('&') {
        let mut kv = pair.splitn(2, '=');
        if kv.next() == Some(key) {
            return match kv.next().and_then(|v| v.parse::<usize>().ok()) {
                Some(n) => n.clamp(1, max),
                None => default,
            };
        }
    }
    default
}

fn last_transfer_json() -> String {
    let guard = LAST.lock().unwrap_or_else(|e| e.into_inner());
    let Some(s) = *guard else {
        return String::from(r#"{"ok": true, "transfer": null}"#);
    };
    format!(
        r#"{{"ok": true, "transfer": {{"bytes": {}, "chunks": {}, "total_us": {}, "read_us": {}, "read_max_us": {}, "write_us": {}, "write_max_us": {}, "ro_lock_us": {}, "mount_us": {}, "sd_wait_us": {}, "sd_wait_max_us": {}}}}}"#,
        s.bytes,
        s.chunks,
        s.total_us,
        s.read_us,
        s.read_max_us,
        s.write_us,
        s.write_max_us,
        s.lock_us,
        s.mount_us,
        s.sd_wait_us,
        s.sd_wait_max_us
    )
}

fn send_zeros<C>(req: esp_idf_svc::http::server::Request<C>) -> anyhow::Result<()>
where
    C: esp_idf_svc::http::server::Connection,
    C::Error: std::error::Error + Send + Sync + 'static,
{
    const MAX_N: usize = 64 * 1024 * 1024;
    const MAX_CHUNK: usize = 64 * 1024;

    let n = query_num(req.uri(), "n", 4 * 1024 * 1024, MAX_N);
    let want = query_num(req.uri(), "chunk", CHUNK, MAX_CHUNK);

    let mut resp = req.into_response(
        200,
        None,
        &[
            ("Content-Type", "application/octet-stream"),
            ("Cache-Control", "no-store"),
        ],
    )?;

    let t0 = now_us();
    let mut sent = 0usize;
    let mut chunks = 0u32;

    let mut scratch: Vec<u8> = Vec::new();
    let big = want > CHUNK && scratch.try_reserve_exact(want).is_ok();
    if big {
        scratch.resize(want, 0);
        while sent < n {
            let k = want.min(n - sent);
            resp.write_all(&scratch[..k])?;
            sent += k;
            chunks += 1;
        }
    } else {
        let mut guard = SEND_BUF.lock().unwrap_or_else(|e| e.into_inner());
        let buf = &mut guard.0;
        buf.fill(0);
        let step = want.min(CHUNK);
        while sent < n {
            let k = step.min(n - sent);
            resp.write_all(&buf[..k])?;
            sent += k;
            chunks += 1;
        }
    }
    resp.flush()?;

    let took = (now_us() - t0) as u32;
    log::info!(
        "zeros: {sent} B in {took} us, {chunks} chunk(s) of {}",
        if big { want } else { want.min(CHUNK) }
    );
    Ok(())
}

fn respond<C>(
    req: esp_idf_svc::http::server::Request<C>,
    status: u16,
    body: &[u8],
) -> anyhow::Result<()>
where
    C: esp_idf_svc::http::server::Connection,
    C::Error: std::error::Error + Send + Sync + 'static,
{
    req.into_status_response(status)?.write_all(body)?;
    Ok(())
}

const ERR_BUSY: &[u8] =
    br#"{"ok": false, "error": "another request is already running on this station"}"#;
const ERR_SERVING: &[u8] =
    br#"{"ok": false, "error": "a replay is being served right now; retry once it finishes"}"#;
const ERR_GAME_LIVE: &[u8] =
    br#"{"ok": false, "error": "a game is being recorded right now; retry once it finishes"}"#;
const ERR_CONFIRM: &[u8] = br#"{"ok": false, "error": "POST /reset-beamer needs the header 'X-Beamer-Confirm: reset'. It erases every replay on this station."}"#;

fn error_body(msg: &str) -> String {
    let mut s = String::from("{\"ok\": false, \"error\": \"");
    crate::slp::escape_json_into(msg, &mut s);
    s.push_str("\"}");
    s
}

fn status_body() -> String {
    let id = super::check::identity();
    let link = super::wifi::link().map(|l| report::LinkInfo {
        rssi: l.rssi,
        phy: l.phy,
        channel: l.channel,
    });
    report::status_json(
        &id.station,
        &id.station_name,
        id.ssid.as_deref(),
        link,
        &scan::fast(),
        scan::replay_cap(),
        scan::uptime_s(),
        errors::session_has_errors(),
        &crate::warnings::labels(),
        match super::result() {
            super::NetResult::Ok => report::Health::Ok,
            super::NetResult::Pending => report::Health::Starting,
            super::NetResult::Offline => report::Health::Ok, // this means no ssid was set in config!
            super::NetResult::Fail => report::Health::Error,
        },
    )
}

fn respond_json<C>(
    req: esp_idf_svc::http::server::Request<C>,
    status: u16,
    body: &[u8],
) -> anyhow::Result<()>
where
    C: esp_idf_svc::http::server::Connection,
    C::Error: std::error::Error + Send + Sync + 'static,
{
    let mut resp = req.into_response(
        status,
        None,
        &[
            ("Content-Type", "application/json"),
            ("Cache-Control", "no-store"),
        ],
    )?;
    resp.write_all(body)?;
    resp.flush()?;
    Ok(())
}
