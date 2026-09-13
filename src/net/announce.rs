use core::fmt::Write as _;
use std::net::{Ipv4Addr, SocketAddrV4, UdpSocket};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Mutex;

use crate::report;
use crate::slp::{self, escape_json_into};

const GROUP: Ipv4Addr = Ipv4Addr::new(239, 255, 42, 1);
const PORT: u16 = 34700;
const TTL: u32 = 1; // one hop: the datagram never leaves the LAN
const MAX: usize = 1472;

struct Announcer {
    socket: UdpSocket,
    station_id: String,
    station_name: String,
}

static STATE: Mutex<Option<Announcer>> = Mutex::new(None);
static SEQ: AtomicU32 = AtomicU32::new(0);

fn lock() -> std::sync::MutexGuard<'static, Option<Announcer>> {
    STATE.lock().unwrap_or_else(|e| e.into_inner())
}

pub fn open(station_id: &str, station_name: &str) {
    let socket = match UdpSocket::bind("0.0.0.0:0").and_then(|s| {
        s.set_multicast_ttl_v4(TTL)?;
        Ok(s)
    }) {
        Ok(s) => s,
        Err(e) => {
            log::warn!("announce: no socket, game events will not be sent: {e}");
            return;
        }
    };
    *lock() = Some(Announcer {
        socket,
        station_id: station_id.to_owned(),
        station_name: station_name.to_owned(),
    });
    log::info!("announce: game events to {GROUP}:{PORT}");
}

pub fn set_name(station_name: &str) {
    if let Some(a) = lock().as_mut() {
        a.station_name = station_name.to_owned();
    }
}

pub fn close() {
    *lock() = None;
}

pub fn game_started(game: &slp::Game, name: &str, size: u64) {
    send("game_started", game, name, size);
}

pub fn game_finished(game: &slp::Game, name: &str, size: u64) {
    send("game_finished", game, name, size);
}

fn send(event: &str, game: &slp::Game, name: &str, size: u64) {
    let guard = lock();
    let Some(a) = guard.as_ref() else { return };

    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    let mut buf: heapless::String<MAX> = heapless::String::new();
    if build(&mut buf, a, event, seq, game, name, size).is_err() {
        log::warn!("announce: {event} for {name} did not fit; skipping");
        return;
    }

    let _ = a
        .socket
        .send_to(buf.as_bytes(), SocketAddrV4::new(GROUP, PORT));
}

fn build(
    buf: &mut heapless::String<MAX>,
    a: &Announcer,
    event: &str,
    seq: u32,
    game: &slp::Game,
    name: &str,
    size: u64,
) -> core::fmt::Result {
    write!(
        buf,
        "{{\"schema\":{},\"event\":\"{event}\",",
        report::SCHEMA
    )?;
    buf.write_str("\"station_id\":\"")?;
    escape_json_into(&a.station_id, buf);
    buf.write_str("\",\"station_name\":\"")?;
    escape_json_into(&a.station_name, buf);
    write!(buf, "\",\"seq\":{seq},\"replay\":{{\"name\":\"")?;
    escape_json_into(name, buf);
    write!(buf, "\",\"size\":{size},\"url\":\"/SLIPPI/")?;
    escape_json_into(name, buf);
    buf.write_str("\"},\"game\":")?;

    let before = buf.len();
    let mut game_json = report::GameJson::new();
    game.to_json_into(&mut game_json);
    if buf.push_str(game_json.as_str()).is_err() {
        buf.truncate(before);
        buf.write_str("null")?; // if the game is too big, just dont bother
    }

    buf.write_str("}")
}
