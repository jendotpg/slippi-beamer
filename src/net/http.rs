use std::io::Read as _;
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

    let card = sd;

    let reset_card = card.clone();
    server.fn_handler::<anyhow::Error, _>("/reset-beamer", Method::Post, move |req| {
        if req.header("X-Beamer-Confirm").as_deref() != Some("reset") {
            return respond_json(req, 400, ERR_CONFIRM);
        }
        let Ok(_guard) = API_LOCK.try_lock() else {
            return respond_json(req, 409, ERR_BUSY);
        };
        if super::transfers_in_flight() > 0 {
            return respond_json(req, 409, ERR_SERVING);
        }
        match volume::wipe_replays(&reset_card) {
            Ok(n) => {
                scan::forget_all();
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

    let window = match ReadWindow::open(sd) {
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

    let mut resp = req.into_response(200, None, &[("Content-Type", "application/octet-stream")])?;
    let mut buf = SEND_BUF.lock().unwrap_or_else(|e| e.into_inner());
    let buf = &mut buf.0;

    loop {
        let n = match file.read(buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) => {
                log::error!("{path}: read failed after the header: {e}");
                break;
            }
        };
        resp.write_all(&buf[..n])?;
    }
    resp.flush()?;
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
const ERR_CONFIRM: &[u8] = br#"{"ok": false, "error": "POST /reset-beamer needs the header 'X-Beamer-Confirm: reset'. It erases every replay on this station."}"#;

fn error_body(msg: &str) -> String {
    let mut s = String::from("{\"ok\": false, \"error\": \"");
    crate::slp::escape_json_into(msg, &mut s);
    s.push_str("\"}");
    s
}

fn status_body() -> String {
    report::status_json(
        scan::uptime_s(),
        &scan::fast(),
        &super::check::health(),
        &errors::json_errors(),
        errors::session_has_errors(),
        &crate::warnings::labels(),
        match super::result() {
            super::NetResult::Ok => report::Verdict::Pass,
            super::NetResult::Pending => report::Verdict::Pending,
            super::NetResult::Offline => report::Verdict::Pass, // this means no ssid was set in config!
            super::NetResult::Fail => report::Verdict::Fail,
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
