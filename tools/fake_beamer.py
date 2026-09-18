#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.10"
# dependencies = ["click"]
# ///

import json
import os
import re
import shutil
import signal
import socket
import subprocess
import sys
import threading
import time
import uuid
import zlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from types import SimpleNamespace

import click

sys.dont_write_bytecode = True

import beamer_lib as beamer

SCHEMA = 1
DEFAULT_SERVED = 10
DEFAULT_CAP = 512
CHUNK = 8 * 1024
GZ_WINDOW_BITS = 10
GZ_MEM_LEVEL = 3
GZ_LEVEL = 2  # the firmware's default

ANNOUNCE_GROUP = "239.255.42.1"
ANNOUNCE_PORT = 34700

FAKE_RSSI = 0
FAKE_PHY_MODE = "fake"
FAKE_CHANNEL = 0

ERR_NOT_FOUND = "no such replay on this station"
ERR_STAT = "that replay could not be read"
ERR_RANGE = "range not satisfiable"
ERR_SERVING = "a replay is being served right now; retry once it finishes"
ERR_CONFIRM = ("POST /reset-beamer needs the header 'X-Beamer-Confirm: reset'. "
               "It erases every replay on this station.")
ERR_GAME_LIVE = "a game is being recorded right now; retry once it finishes"
RETRY_AFTER_SERVING = "2"
RETRY_AFTER_GAME_LIVE = "15"


# A port of beamer::slp (src/slp.rs)
PEEK_BYTES = 1024
MAGIC = b"{U\x03raw[$U#l"
GS_DEEPEST = 0x1A1
CHARS = [
    ("Falcon", [None, "black", "red", "white", "green", "blue"]),
    ("DK", [None, "black", "red", "blue", "green"]),
    ("Fox", [None, "red", "blue", "green"]),
    ("GW", [None, "red", "blue", "green"]),
    ("Kirby", [None, "yellow", "blue", "red", "green", "white"]),
    ("Bowser", [None, "red", "blue", "black"]),
    ("Link", [None, "red", "blue", "black", "white"]),
    ("Luigi", [None, "white", "blue", "red"]),
    ("Mario", [None, "yellow", "black", "blue", "green"]),
    ("Marth", [None, "red", "green", "black", "white"]),
    ("Mewtwo", [None, "red", "blue", "green"]),
    ("Ness", [None, "gold", "blue", "green"]),
    ("Peach", [None, "gold", "white", "blue", "green"]),
    ("Pikachu", [None, "red", "blue", "green"]),
    ("ICs", [None, "green", "yellow", "red"]),
    ("Puff", [None, "red", "blue", "green", "gold"]),
    ("Samus", [None, "pink", "dark", "green", "blue"]),
    ("Yoshi", [None, "red", "blue", "yellow", "pink", "cyan"]),
    ("Zelda", [None, "red", "blue", "green", "white"]),
    ("Sheik", [None, "red", "blue", "green", "white"]),
    ("Falco", [None, "red", "blue", "green"]),
    ("YL", [None, "red", "blue", "white", "black"]),
    ("Doc", [None, "red", "blue", "green", "black"]),
    ("Roy", [None, "red", "blue", "green", "gold"]),
    ("Pichu", [None, "red", "blue", "green"]),
    ("Ganon", [None, "red", "blue", "green", "purple"]),
]


class PeekError(Exception):
    """Any failure here means do not publish."""


REPLACEMENT = "\ufffd"

SJIS_81 = [
    0x3000,
    0x3001,
    0x3002,
    0xFF0C,
    0xFF0E,
    0x30FB,
    0xFF1A,
    0xFF1B,
    0xFF1F,
    0xFF01,
    0x309B,
    0x309C,
    0x00B4,
    0xFF40,
    0x00A8,
    0xFF3E,
    0xFFE3,
    0xFF3F,
    0x30FD,
    0x30FE,
    0x309D,
    0x309E,
    0x3003,
    0x4EDD,
    0x3005,
    0x3006,
    0x3007,
    0x30FC,
    0x2015,
    0x2010,
    0xFF0F,
    0xFF3C,
    0x301C,
    0x2016,
    0xFF5C,
    0x2026,
    0x2025,
    0x2018,
    0x2019,
    0x201C,
    0x201D,
    0xFF08,
    0xFF09,
    0x3014,
    0x3015,
    0xFF3B,
    0xFF3D,
    0xFF5B,
    0xFF5D,
    0x3008,
    0x3009,
    0x300A,
    0x300B,
    0x300C,
    0x300D,
    0x300E,
    0x300F,
    0x3010,
    0x3011,
    0xFF0B,
    0x2212,
    0x00B1,
    0x00D7,
    0x00F7,
    0xFF1D,
    0x2260,
    0xFF1C,
    0xFF1E,
    0x2266,
    0x2267,
    0x221E,
    0x2234,
    0x2642,
    0x2640,
    0x00B0,
    0x2032,
    0x2033,
    0x2103,
    0xFFE5,
    0xFF04,
    0x00A2,
    0x00A3,
    0xFF05,
    0xFF03,
    0xFF06,
    0xFF0A,
    0xFF20,
    0x00A7,
    0x2606,
    0x2605,
    0x25CB,
    0x25CF,
    0x25CE,
    0x25C7,
    0x25C6,
    0x25A1,
    0x25A0,
    0x25B3,
    0x25B2,
    0x25BD,
    0x25BC,
    0x203B,
    0x3012,
    0x2192,
    0x2190,
    0x2191,
    0x2193,
    0x3013,
    0x0000,
    0x0000,
    0x0000,
    0x0000,
    0x0000,
    0x0000,
    0x0000,
    0x0000,
    0x0000,
    0x0000,
    0x0000,
    0x2208,
    0x220B,
    0x2286,
    0x2287,
    0x2282,
    0x2283,
    0x222A,
    0x2229,
    0x0000,
    0x0000,
    0x0000,
    0x0000,
    0x0000,
    0x0000,
    0x0000,
    0x0000,
    0x2227,
    0x2228,
    0x00AC,
    0x21D2,
    0x21D4,
    0x2200,
    0x2203,
    0x0000,
    0x0000,
    0x0000,
    0x0000,
    0x0000,
    0x0000,
    0x0000,
    0x0000,
    0x0000,
    0x0000,
    0x0000,
    0x2220,
    0x22A5,
    0x2312,
    0x2202,
    0x2207,
    0x2261,
    0x2252,
    0x226A,
    0x226B,
    0x221A,
    0x223D,
    0x221D,
    0x2235,
    0x222B,
    0x222C,
    0x0000,
    0x0000,
    0x0000,
    0x0000,
    0x0000,
    0x0000,
    0x0000,
    0x212B,
    0x2030,
    0x266F,
    0x266D,
    0x266A,
    0x2020,
    0x2021,
    0x00B6,
    0x0000,
    0x0000,
    0x0000,
    0x0000,
    0x25EF,
]


def trail_index(lo):
    """Position of a trail byte within its lead byte's row. 0x7F is not a valid
    trail byte, so the range skips it."""
    return (lo - 0x40) - (1 if lo > 0x7F else 0)


def sjis_next(p):
    """Decode one character, returning it and how many bytes it consumed."""
    hi = p[0]

    if 0x20 <= hi <= 0x7E:
        return chr(hi), 1
    # Half-width katakana.
    if 0xA1 <= hi <= 0xDF:
        return chr(0xFF61 + (hi - 0xA1)), 1

    if len(p) < 2 or not (0x81 <= hi <= 0x83):
        return REPLACEMENT, 1

    lo = p[1]
    if not (0x40 <= lo <= 0xFC) or lo == 0x7F:
        return REPLACEMENT, 1

    if hi == 0x81:
        cp = SJIS_81[trail_index(lo)] if trail_index(lo) < len(SJIS_81) else 0
        if cp == 0:
            return REPLACEMENT, 2
    elif hi == 0x82:
        if 0x4F <= lo <= 0x58:
            cp = 0xFF10 + (lo - 0x4F)  # full-width digits
        elif 0x60 <= lo <= 0x79:
            cp = 0xFF21 + (lo - 0x60)  # full-width A-Z
        elif 0x81 <= lo <= 0x9A:
            cp = 0xFF41 + (lo - 0x81)  # full-width a-z
        elif 0x9F <= lo <= 0xF1:
            cp = 0x3041 + (lo - 0x9F)  # hiragana
        else:
            return REPLACEMENT, 2
    elif lo <= 0x96:
        cp = 0x30A1 + trail_index(lo)  # katakana
    else:
        return REPLACEMENT, 2

    return chr(cp), 2


def decode_nametag(tag):
    out = []
    i = 0
    while i < len(tag):
        if tag[i] == 0:
            break
        c, used = sjis_next(tag[i:])
        out.append(c)
        i += used
    return "".join(out) or None


def peek(buf):
    n = len(buf)

    if n < 17 or not buf.startswith(MAGIC):
        raise PeekError("not an .slp file")

    live = int.from_bytes(buf[11:15], "big") == 0

    if buf[15] != 0x35:
        raise PeekError("no event payloads command")

    psz = buf[16]
    if psz < 4 or (psz - 1) % 3 != 0:
        raise PeekError("bad event payloads size")

    nent = (psz - 1) // 3
    if 17 + 3 * nent > n:
        raise PeekError("truncated event payloads")

    gs_size = 0
    for i in range(nent):
        if buf[17 + 3 * i] == 0x36:
            gs_size = int.from_bytes(buf[18 + 3 * i : 20 + 3 * i], "big")
            break

    gs = 15 + 1 + psz

    if gs + 0xD4 >= n or buf[gs] != 0x36:
        raise PeekError("truncated or missing game start")

    has_nametags = gs_size + 1 >= GS_DEEPEST
    if has_nametags and gs + GS_DEEPEST > n:
        raise PeekError("truncated game start")

    ports = []
    for i in range(4):
        pb = gs + 0x65 + 0x24 * i
        cid = buf[pb]
        player_type = buf[pb + 1]
        costume = buf[pb + 3]

        # 0 is human, 1 is CPU. Anything else (2 = demo, 3 = empty) is not a
        # player and is left out entirely.
        if player_type not in (0, 1):
            continue

        entry = CHARS[cid] if cid < len(CHARS) else None
        color = None
        if entry is not None and costume < len(entry[1]):
            color = entry[1][costume]

        ports.append(
            {
                "port": i + 1,
                "char": entry[0] if entry else None,
                "char_id": cid if entry else None,
                "color": color,
                "costume": costume,
                "nametag": (
                    decode_nametag(buf[gs + 0x161 + 0x10 * i :][:16])
                    if has_nametags
                    else None
                ),
            }
        )

    return {"live": live, "ports": ports}


def secs_since(at, now):
    return None if at is None else int(now - at)


def port_sig(game):
    return tuple(p["port"] for p in game["ports"]) if game else None


def character_sig(game):
    return tuple((p["port"], p["char_id"]) for p in game["ports"]) if game else None


class Station:
    def __init__(self, args):
        self.args = args
        self.station_id = args.station or str(uuid.uuid4())
        self.station_name = args.station_name or self.station_id
        self.lock = threading.Lock() # safe "set_game" call
        self.transfer_lock = threading.Lock()
        self.replay_requests = 0
        self.port_sig = None
        self.character_sig = None
        self.port_change_at = None
        self.character_change_at = None
        self.game_start_at = None
        self.game = None
        self.deferred_name = None
        if args.ended_delay > 0:
            served = self._all_replay_names()
            if served:
                self.deferred_name = sorted(served)[0]
        self.set_game(self.read_game())

    def live(self):
        return self.game is not None and self.game["live"]

    def set_game(self, game):
        with self.lock:
            was_live = self.game is not None and self.game["live"]
            self.game = game
            if game is None:
                return
            now = time.monotonic()
            if game["live"] and not was_live:
                self.game_start_at = now
            ports, chars = port_sig(game), character_sig(game)
            if self.port_sig != ports:
                self.port_sig = ports
                self.port_change_at = now
            if self.character_sig != chars:
                self.character_sig = chars
                self.character_change_at = now

    def read_game(self):
        if not self.args.game:
            return None
        try:
            with open(self.args.game, "rb") as f:
                buf = f.read(PEEK_BYTES)
        except OSError as e:
            print(f"fake_beamer: cannot read {self.args.game}: {e}", file=sys.stderr)
            return None
        try:
            return peek(buf)
        except PeekError as e:
            print(f"fake_beamer: {self.args.game}: {e}", file=sys.stderr)
            return None

    def _all_replay_names(self):
        if not self.args.replays:
            return []
        try:
            names = [
                name
                for name in os.listdir(self.args.replays)
                if name.endswith(".slp") and not name.startswith(".")
            ]
        except OSError:
            return []
        names.sort(reverse=True)
        return names[: self.args.served]

    def replays(self):
        names = self._all_replay_names()
        if self.deferred_name in names:
            names.remove(self.deferred_name)
        return names

    def release_deferred(self):
        name = self.deferred_name
        self.deferred_name = None
        return name

    def status(self):
        with self.lock:
            game = self.game
            now = time.monotonic()
            since_ports = secs_since(self.port_change_at, now)
            since_chars = secs_since(self.character_change_at, now)
            since_game = secs_since(self.game_start_at, now)
        return {
            "schema": SCHEMA,
            "arch": "fake",
            "firmware_version": "fake",
            "station_id": self.station_id,
            "station_name": self.station_name,
            "ssid": self.args.wifi,
            "rssi": self.args.rssi,
            "phy_mode": self.args.phy_mode,
            "channel": self.args.channel,
            "replay_count": len(self.replays()),
            "replay_cap": self.args.cap,
            "serving": 1 if self.transfer_lock.locked() else 0,
            "ssh": False,
            "game": game,
            "secs_since_port_change": since_ports,
            "secs_since_character_change": since_chars,
            "secs_since_game_start": since_game,
            "health": self.health(),
            "warnings": self.warnings(),
        }

    def announce_payload(self, event, seq, replay_name=None):
        replay = None
        name = replay_name
        if name is None:
            names = self.replays()
            if names:
                name = names[0]
        if name:
            try:
                size = os.path.getsize(os.path.join(self.args.replays, name))
                replay = {"name": name, "size": size, "url": f"/SLIPPI/{name}"}
            except OSError:
                replay = None
        return {
            "schema": SCHEMA,
            "event": event,
            "station_id": self.station_id,
            "station_name": self.station_name,
            "seq": seq,
            "replay": replay,
            "game": self.game,
        }

    def warnings(self):
        return [w.strip().upper() for w in self.args.warn.split(",") if w.strip()]

    def health(self):
        if self.args.unhealthy:
            return "error"
        return "warn" if self.warnings() else "ok"

    def index(self):
        names = self.replays()
        files = []
        for name in names:
            try:
                size = os.path.getsize(os.path.join(self.args.replays, name))
            except OSError:
                continue
            files.append(
                {
                    "size": size,
                    "url": f"/SLIPPI/{name}",
                }
            )
        return {
            "schema": SCHEMA,
            "station_id": self.station_id,
            "served_replay_count": len(files),
            "files": files,
        }


SAFE_NAME = re.compile(r"^[A-Za-z0-9._-]+\.slp$")

class Handler(BaseHTTPRequestHandler):
    station: Station = None

    protocol_version = "HTTP/1.1"

    def log_message(self, fmt, *args):
        print(f"fake_beamer[{self.station.args.port}] {fmt % args}", file=sys.stderr)

    def send_json(self, code, payload, extra=None):
        body = json.dumps(payload).encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Cache-Control", "no-store")
        for key, value in (extra or {}).items():
            self.send_header(key, value)
        self.end_headers()
        self.wfile.write(body)

    def send_error_json(self, code, message, extra=None):
        self.send_json(code, {"ok": False, "error": message}, extra)

    def send_text(self, code, text):
        body = text.encode()
        self.send_response(code)
        self.send_header("Content-Type", "text/plain")
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Cache-Control", "no-store")
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        path = self.path.split("?", 1)[0].rstrip("/") or "/"

        if path == "/status":
            if self.station.args.unreported:
                self.send_error_json(503, "no status check has run yet on this station")
                return
            self.send_json(200, self.station.status())
            return

        if path == "/SLIPPI":
            self.send_json(200, self.station.index())
            return

        if path.startswith("/SLIPPI/"):
            self.serve_replay(path[len("/SLIPPI/") :])
            return

        # GET / is a plain-text 403 on a real station (net/http.rs).
        if path == "/":
            self.send_text(403, "forbidden\n")
            return
        self.send_error_json(404, "no such endpoint")

    def parse_range(self, total):
        raw = self.headers.get("Range")
        if raw is None:
            return None
        spec = raw.strip()
        if not spec.startswith("bytes="):
            return "bad"
        start = spec[len("bytes=") :].strip()
        if not start.endswith("-"):
            return "bad"
        try:
            n = int(start[:-1])
        except ValueError:
            return "bad"
        return n if 0 <= n < total else "bad"

    def parse_from(self, total):
        raw = self.headers.get("X-Replay-From")
        if raw is None:
            return None
        try:
            n = int(raw.strip())
        except ValueError:
            return "bad"
        return n if 0 <= n < total else "bad"

    def gzip_wanted(self):
        raw = self.headers.get("Accept-Encoding")
        if not raw:
            return False
        for part in raw.split(","):
            fields = [f.strip() for f in part.split(";")]
            if fields[0].lower() != "gzip":
                continue
            for f in fields[1:]:
                k, _, v = f.partition("=")
                if k.lower() == "q":
                    try:
                        if float(v) <= 0:
                            return False
                    except ValueError:
                        pass
            return True
        return False

    def gzip_level(self):
        try:
            n = int(self.headers.get("X-Beamer-Gz-Level", GZ_LEVEL))
        except ValueError:
            return GZ_LEVEL
        return max(1, min(9, n))

    def serve_replay(self, name):
        if not SAFE_NAME.match(name) or not self.station.args.replays:
            self.send_error_json(404, ERR_NOT_FOUND)
            return
        full = os.path.join(self.station.args.replays, name)
        if not os.path.isfile(full):
            self.send_error_json(404, ERR_NOT_FOUND)
            return
        if name == self.station.deferred_name:
            self.send_error_json(404, ERR_NOT_FOUND)
            return

        if not self.station.transfer_lock.acquire(blocking=False): # one at a time, just like firmware
            self.send_error_json(503, ERR_SERVING, {"Retry-After": RETRY_AFTER_SERVING})
            return
        try:
            self._stream_replay(full)
        finally:
            self.station.transfer_lock.release()

    def _stream_replay(self, full):
        try:
            with open(full, "rb") as f:
                body = f.read()
        except OSError:
            self.send_error_json(500, ERR_STAT)
            return

        total = len(body)
        start = self.parse_range(total)
        resume = None if start is not None else self.parse_from(total)
        if start == "bad" or resume == "bad":
            self.send_error_json(416, ERR_RANGE, {
                "Content-Range": f"bytes */{total}",
                "Accept-Ranges": "bytes",
            })
            return

        ranged = start is not None
        resumed = resume is not None
        if not ranged:
            start = resume or 0
        body = body[start:]

        level = self.gzip_level()
        gzip = not ranged and self.gzip_wanted()
        if gzip:
            c = zlib.compressobj(level, zlib.DEFLATED, 16 + GZ_WINDOW_BITS, GZ_MEM_LEVEL)
            body = c.compress(body) + c.flush()

        self.station.replay_requests += 1
        n = self.station.replay_requests
        truncate = self.station.args.truncate_every
        stall = self.station.args.stall_every

        self.send_response(206 if ranged else 200)
        self.send_header("Content-Type", "application/octet-stream")
        self.send_header("Transfer-Encoding", "chunked") # always chunked - real firmware is memory starved
        if gzip:
            self.send_header("Content-Encoding", "gzip")
            self.send_header("Vary", "Accept-Encoding, X-Replay-From")
        else:
            self.send_header("Accept-Ranges", "bytes")
        if ranged:
            self.send_header("Content-Range", f"bytes {start}-{total - 1}/{total}")
        if resumed:
            self.send_header("X-Replay-From", str(start))
        self.end_headers()

        if stall and n % stall == 0:
            self.write_chunk(body[:CHUNK])
            time.sleep(self.station.args.stall_seconds)
            return
        if truncate and n % truncate == 0:
            self.write_body(body[: len(body) // 2], last=False)
            return
        self.write_body(body)

    def write_body(self, body, last=True):
        rate = self.station.args.rate
        per_chunk = CHUNK / (rate * 1024) if rate else 0
        for i in range(0, len(body), CHUNK):
            self.write_chunk(body[i : i + CHUNK])
            if per_chunk:
                time.sleep(per_chunk)
        if last:
            self.wfile.write(b"0\r\n\r\n")
            self.wfile.flush()

    def write_chunk(self, piece):
        self.wfile.write(f"{len(piece):x}\r\n".encode())
        self.wfile.write(piece)
        self.wfile.write(b"\r\n")
        self.wfile.flush()

    def do_POST(self):
        path = self.path.split("?", 1)[0].rstrip("/") or "/"
        length = self.headers.get("Content-Length")
        if length is None:
            self.send_error_json(411, "Length Required")
            return
        self.rfile.read(int(length))

        if path == "/reset-beamer":
            if self.headers.get("X-Beamer-Confirm") != "reset":
                self.send_error_json(400, ERR_CONFIRM)
                return
            if self.station.live():
                self.send_error_json(409, ERR_GAME_LIVE,
                                     {"Retry-After": RETRY_AFTER_GAME_LIVE})
                return
            if self.station.transfer_lock.locked():
                self.send_error_json(409, ERR_SERVING,
                                     {"Retry-After": RETRY_AFTER_SERVING})
                return
            self.send_json(200, {"ok": True, "message": "reset OK"})
            return

        self.send_error_json(404, "no such endpoint")


class Announcer:
    def __init__(self, station, enabled):
        self.station = station
        self.enabled = enabled
        self.seq = 0
        self.sock = None
        if enabled:
            self.sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
            self.sock.setsockopt(socket.IPPROTO_IP, socket.IP_MULTICAST_TTL, 1)

    def send(self, event, replay_name=None):
        if not self.enabled:
            return
        self.seq += 1
        payload = self.station.announce_payload(event, self.seq, replay_name)
        self.sock.sendto(json.dumps(payload).encode(), (ANNOUNCE_GROUP, ANNOUNCE_PORT))
        print(f"fake_beamer[{self.station.args.port}] announce {event} "
              f"seq={self.seq} -> {ANNOUNCE_GROUP}:{ANNOUNCE_PORT}", file=sys.stderr)


def advertise(name, port):
    if sys.platform == "darwin":
        cmd = ["dns-sd", "-R", name, "_beamer._tcp", "local", str(port)]
    else:
        cmd = [
            "avahi-publish",
            "-s",
            name,
            "_beamer._tcp",
            str(port),
        ]
    if not shutil.which(cmd[0]):
        print(
            f"fake_beamer: {cmd[0]} not found; serving HTTP but not advertising, "
            f"so the app will not see this station -- its fleet view is "
            f"discovery-only.",
            file=sys.stderr,
        )
        return None
    print(f"fake_beamer: advertising {name} as _beamer._tcp on {port}")
    return subprocess.Popen(cmd, stdout=subprocess.DEVNULL)


@click.command(context_settings=beamer.HELP_OPTIONS)
@click.option("--name", default="", show_default=True,
              help="mDNS instance name to advertise as. Defaults to --station-name, "
                   "or beamer-fake.")
@click.option("--port", type=int, default=8080, show_default=True,
              help="HTTP port to serve on. Run several on different ports for a fleet.")
@click.option("--station", default="", help="Station uuid. Random per run if unset.")
@click.option("--station-name", default="", show_default=True,
              help="Station name the app displays; a real station sets this with "
                   "its button. Falls back to the station id when unset.")
@click.option("--wifi", default="fake-net", show_default=True, help="ssid to report.")
@click.option("--replays", default="", help="Directory of .slp files to serve.")
@click.option("--game", default="", help=".slp to peek and report as the current game.")
@click.option("--served", type=int, default=DEFAULT_SERVED, show_default=True,
              help="How many replays to publish in the served index.")
@click.option("--cap", type=int, default=DEFAULT_CAP, show_default=True,
              help="replay_cap the station reports.")
@click.option("--rssi", type=int, default=FAKE_RSSI, show_default=True,
              help="rssi to report (dBm). Default is an obviously-fake sentinel.")
@click.option("--phy-mode", default=FAKE_PHY_MODE, show_default=True,
              help="phy_mode to report. Default is an obviously-fake sentinel.")
@click.option("--channel", type=int, default=FAKE_CHANNEL, show_default=True,
              help="channel to report. Default 0 is an obviously-fake sentinel.")
@click.option("--unhealthy", is_flag=True, help='Report health "error".')
@click.option("--warn", default="",
              help='Comma-separated warning labels, e.g. "DRIVE FULL,NO WII"; '
                   'any warning reports health "warn".')
@click.option("--unreported", is_flag=True, help="Answer 503 on GET /status.")
@click.option("--truncate-every", type=int, default=0, metavar="N",
              help="Stop the chunked stream mid-body with no terminating chunk on "
                   "every Nth replay - a dropped-link truncation.")
@click.option("--stall-every", type=int, default=0, metavar="N",
              help="Send one chunk then go quiet on every Nth replay, to trip the "
                   "downloader's stall watchdog.")
@click.option("--rate", type=float, default=0, metavar="KBPS",
              help="Throttle replay bodies to roughly this many KB/s, standing in "
                   "for a congested venue AP.")
@click.option("--stall-seconds", type=float, default=30.0, show_default=True,
              help="How long --stall-every holds the connection open.")
@click.option("--announce/--no-announce", default=True, show_default=True,
              help="Multicast game_started / game_finished on liveness changes.")
@click.option("--ended-delay", type=float, default=0.0, show_default=True, metavar="SECONDS",
              help="Announce game_finished this many seconds after startup, releasing the "
                   "held-back replay in the same instant.")
def main(**opts):
    args = SimpleNamespace(**opts)

    args.name = args.name or args.station_name or "beamer-fake"

    if args.replays and not os.path.isdir(args.replays):
        raise click.BadParameter(f"{args.replays} is not a directory", param_hint="--replays")
    if args.game and not os.path.isfile(args.game):
        raise click.BadParameter(f"{args.game} is not a file", param_hint="--game")

    beamer.banner("fake_beamer", name=args.name, port=args.port,
                  station=args.station_name or "(unnamed)")

    station = Station(args)
    announcer = Announcer(station, args.announce)
    handler = type("BoundHandler", (Handler,), {"station": station})
    server = ThreadingHTTPServer(("0.0.0.0", args.port), handler)
    advertiser = advertise(args.name, args.port)

    def shutdown(_signum, _frame):
        print("\nfake_beamer: stopping")
        if station.live():
            announcer.send("game_finished")
        if advertiser:
            advertiser.terminate()
        threading.Thread(target=server.shutdown).start()

    def reload_game(_signum, _frame):
        was_live = station.live()
        station.set_game(station.read_game())
        now_live = station.live()
        if now_live and not was_live:
            announcer.send("game_started")
        elif was_live and not now_live:
            announcer.send("game_finished")

    signal.signal(signal.SIGINT, shutdown)
    signal.signal(signal.SIGTERM, shutdown)
    signal.signal(signal.SIGHUP, reload_game)

    print(f"fake_beamer: http://localhost:{args.port}/status")
    announcer.send("game_started")

    if args.ended_delay > 0 and station.deferred_name is not None:
        def end_game():
            time.sleep(args.ended_delay)
            name = station.release_deferred()
            announcer.send("game_finished", replay_name=name)

        threading.Thread(target=end_game, daemon=True).start()

    try:
        server.serve_forever()
    finally:
        if advertiser:
            advertiser.terminate()


if __name__ == "__main__":
    main()
