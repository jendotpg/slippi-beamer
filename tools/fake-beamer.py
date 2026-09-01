#!/usr/bin/env python3
"""
fake-beamer - pretend to be a Beamer station, without a Beamer.

Everything replay-manager talks to is an mDNS advertisement and four HTTP
endpoints. None of it needs a Pi, a USB gadget, an LED or a Wii. So this serves
the endpoints and advertises itself, which is enough to develop and test the
whole app-side fleet view on one laptop:

tools/fake-beamer.py --name beamer-virtual-1 --port 8081 --replays ~/Slippi/ --game ~/Slippi/Game_20230110T102627.slp --station-name "Fake 1"

tools/fake-beamer.py --name beamer-virtual-2 --port 8082 --replays ~/Slippi/ --game ~/Slippi/Game_20230110T102700.slp  --station-name "Fake 2"

Run several on different ports to simulate a fleet. The app honours the port a
station advertises, so they coexist happily on one machine.

The game payload is not canned: --game is read out of a real .slp by the peek
below, a port of the same beamer::slp the firmware runs. That is what makes the
character icons in the app a real test rather than a drawing exercise, and it
needs nothing built -- no Rust, no C, no cross-compiler.

What this deliberately does NOT emulate: the gadget, the LED, the config file,
the reset endpoint's actual destruction, and any of the timing of a real Zero W.
It is a stand-in for the fleet view, not for a station.
"""

import argparse
import json
import os
import re
import shutil
import signal
import subprocess
import sys
import threading
import time
import uuid
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

SCHEMA = 1
DEFAULT_SERVED = 10
DEFAULT_CAP = 512
CHUNK = 8 * 1024


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
    """Any failure here means do not publish. Messages match PeekError::as_str
    in src/slp.rs, and slp-peek.c before it, so a log line greps the
    same across all three."""


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
    """Decode a 16-byte nametag field. None for an empty tag, matching the C,
    which prints null when it decoded nothing."""
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

    # The `raw` element's length. Slippi writes it last, so zero means the game
    # is still being played.
    live = int.from_bytes(buf[11:15], "big") == 0

    if buf[15] != 0x35:
        raise PeekError("no event payloads command")

    # Event Payloads: one size byte covering itself plus three bytes per entry.
    psz = buf[16]
    if psz < 4 or (psz - 1) % 3 != 0:
        raise PeekError("bad event payloads size")

    nent = (psz - 1) // 3
    if 17 + 3 * nent > n:
        raise PeekError("truncated event payloads")

    # The declared size of the Game Start payload says whether this replay is
    # old enough to predate nametags.
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
    # No costume: a colour swap is not a character change.
    return tuple((p["port"], p["char_id"]) for p in game["ports"]) if game else None


class Station:
    """The mutable half: what this station currently claims about itself."""

    def __init__(self, args):
        self.args = args
        self.station_id = args.station or str(uuid.uuid5(uuid.NAMESPACE_DNS, args.name))
        self.station_name = args.station_name or ""
        self.lock = threading.Lock()
        self.replay_requests = 0
        self.port_sig = None
        self.character_sig = None
        self.port_change_at = None
        self.character_change_at = None
        self.game = None
        self.set_game(self.read_game())

    def set_game(self, game):
        """What publish_game does: stamp the clocks when a signature changes."""
        self.game = game
        if game is None:
            return
        now = time.monotonic()
        ports, chars = port_sig(game), character_sig(game)
        if self.port_sig != ports:
            self.port_sig = ports
            self.port_change_at = now
        if self.character_sig != chars:
            self.character_sig = chars
            self.character_change_at = now

    def read_game(self):
        """Peek at --game, the same way the scan tick does on a station."""
        if not self.args.game:
            return None
        try:
            with open(self.args.game, "rb") as f:
                buf = f.read(PEEK_BYTES)
        except OSError as e:
            print(f"fake-beamer: cannot read {self.args.game}: {e}", file=sys.stderr)
            return None
        try:
            return peek(buf)
        except PeekError as e:
            print(f"fake-beamer: {self.args.game}: {e}", file=sys.stderr)
            return None

    def replays(self):
        """Newest first, capped, mirroring what the flush publishes."""
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

    def refresh(self):
        """What POST /status does: re-run the checks."""
        with self.lock:
            self.set_game(self.read_game())

    def status(self):
        with self.lock:
            game = self.game
            now = time.monotonic()
            since_ports = secs_since(self.port_change_at, now)
            since_chars = secs_since(self.character_change_at, now)
        return {
            "schema": SCHEMA,
            "arch": "fake",
            "firmware_version": "fake",
            "station_id": self.station_id,
            "station_name": self.station_name,
            "ssid": self.args.wifi,
            "replay_count": len(self.replays()),
            "replay_cap": self.args.cap,
            "ssh": False,
            "game": game,
            "secs_since_port_change": since_ports,
            "secs_since_character_change": since_chars,
            "health": self.health(),
            "warnings": self.warnings(),
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
        print(f"fake-beamer[{self.station.args.port}] {fmt % args}", file=sys.stderr)

    def send_json(self, code, payload):
        body = json.dumps(payload).encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Cache-Control", "no-store")
        self.end_headers()
        self.wfile.write(body)

    def send_error_json(self, code, message):
        self.send_json(code, {"ok": False, "error": message})

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

        self.send_error_json(403 if path == "/" else 404, "no such endpoint")

    def parse_range(self, total):
        """`bytes=N-` only, matching the firmware. Returns a start offset,
        None for no Range, or "bad" for anything unsatisfiable."""
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

    def serve_replay(self, name):
        if not SAFE_NAME.match(name) or not self.station.args.replays:
            self.send_error_json(404, "no such replay")
            return
        full = os.path.join(self.station.args.replays, name)
        if not os.path.isfile(full):
            self.send_error_json(404, "no such replay")
            return
        try:
            with open(full, "rb") as f:
                body = f.read()
        except OSError:
            self.send_error_json(500, "could not read that replay")
            return

        total = len(body)
        start = self.parse_range(total)
        if start == "bad":
            self.send_response(416)
            self.send_header("Content-Range", f"bytes */{total}")
            self.send_header("Accept-Ranges", "bytes")
            self.send_header("Content-Length", "0")
            self.end_headers()
            return

        ranged = start is not None
        if not ranged:
            start = 0
        body = body[start:]

        self.station.replay_requests += 1
        n = self.station.replay_requests
        truncate = self.station.args.truncate_every
        stall = self.station.args.stall_every

        chunked = self.station.args.chunked
        self.send_response(206 if ranged else 200)
        self.send_header("Content-Type", "application/octet-stream")
        if chunked:
            self.send_header("Transfer-Encoding", "chunked")
        else:
            self.send_header("Content-Length", str(len(body)))
        self.send_header("Accept-Ranges", "bytes")
        if ranged:
            self.send_header("Content-Range", f"bytes {start}-{total - 1}/{total}")
        self.end_headers()

        if stall and n % stall == 0:
            # a chunk, then nothing - the client's stall watchdog should fire
            self.write_chunk(body[:CHUNK])
            time.sleep(self.station.args.stall_seconds)
            return
        if truncate and n % truncate == 0:
            # half a body under a full Content-Length, then hang up
            # no terminating chunk: the body just stops, which is what a
            # dropped link looks like
            self.write_body(body[: len(body) // 2], last=False)
            return
        self.write_body(body)

    def write_body(self, body, last=True):
        """--rate exists so the byte-level progress and stall paths are
        observable at all: on localhost a replay arrives in one gulp."""
        rate = self.station.args.rate
        per_chunk = CHUNK / (rate * 1024) if rate else 0
        for i in range(0, len(body), CHUNK):
            self.write_chunk(body[i : i + CHUNK])
            if per_chunk:
                time.sleep(per_chunk)
        if last and self.station.args.chunked:
            self.wfile.write(b"0\r\n\r\n")
            self.wfile.flush()

    def write_chunk(self, piece):
        if self.station.args.chunked:
            self.wfile.write(f"{len(piece):x}\r\n".encode())
            self.wfile.write(piece)
            self.wfile.write(b"\r\n")
        else:
            self.wfile.write(piece)
        self.wfile.flush()

    def do_POST(self):
        path = self.path.split("?", 1)[0].rstrip("/") or "/"
        length = self.headers.get("Content-Length")
        if length is None:
            self.send_error_json(411, "Length Required")
            return
        self.rfile.read(int(length))

        if path == "/status":
            time.sleep(self.station.args.post_delay)
            self.station.refresh()
            self.send_json(200, self.station.status())
            return

        if path == "/reset-beamer":
            if self.headers.get("X-Beamer-Confirm") != "reset":
                self.send_error_json(
                    400,
                    "POST /reset-beamer needs the header 'X-Beamer-Confirm: reset'.",
                )
                return
            self.send_json(200, {"ok": True, "message": "reset OK (fake, no-op)"})
            return

        self.send_error_json(404, "no such endpoint")


def advertise(name, port):
    """Register over mDNS using whatever the OS already has."""
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
            f"fake-beamer: {cmd[0]} not found; serving HTTP but not advertising, "
            f"so the app will not see this station -- its fleet view is "
            f"discovery-only.",
            file=sys.stderr,
        )
        return None
    print(f"fake-beamer: advertising {name} as _beamer._tcp on {port}")
    return subprocess.Popen(cmd, stdout=subprocess.DEVNULL)


def main():
    parser = argparse.ArgumentParser(description="Pretend to be a Beamer station.")
    parser.add_argument("--name", default="beamer-fake", help="mDNS instance name")
    parser.add_argument("--port", type=int, default=8080)
    parser.add_argument("--station", default="", help="station uuid (derived if unset)")
    parser.add_argument("--station-name", default="", help="STATION-NAME value")
    parser.add_argument("--wifi", default="fake-net")
    parser.add_argument("--replays", default="", help="directory of .slp to serve")
    parser.add_argument("--game", default="", help=".slp to report as the current game")
    parser.add_argument("--served", type=int, default=DEFAULT_SERVED)
    parser.add_argument(
        "--cap", type=int, default=DEFAULT_CAP, help="REPLAY-CAP the station reports"
    )
    parser.add_argument(
        "--unhealthy", action="store_true", help='report health "error"'
    )
    parser.add_argument(
        "--warn",
        default="",
        help='comma-separated warning labels, e.g. "DRIVE FULL,NO WII"; '
        'any warning reports result "warn"',
    )
    parser.add_argument("--unreported", action="store_true", help="503 on GET /status")
    parser.add_argument(
        "--truncate-every",
        type=int,
        default=0,
        metavar="N",
        help="hang up halfway through every Nth replay request, under a full "
        "Content-Length - the silent-truncation case",
    )
    parser.add_argument(
        "--stall-every",
        type=int,
        default=0,
        metavar="N",
        help="send one chunk then go quiet on every Nth replay request, to trip "
        "the downloader's stall watchdog",
    )
    parser.add_argument(
        "--chunked",
        action="store_true",
        help="stream replies chunked with no Content-Length, as the firmware "
        "does - the shape that actually ships",
    )
    parser.add_argument(
        "--rate",
        type=float,
        default=0,
        metavar="KBPS",
        help="throttle replay bodies to roughly this many KB/s, standing in "
        "for a congested venue AP",
    )
    parser.add_argument(
        "--stall-seconds",
        type=float,
        default=30.0,
        help="how long --stall-every holds the connection open",
    )
    parser.add_argument(
        "--post-delay",
        type=float,
        default=1.0,
        help="seconds POST /status takes, so the spinner is visible",
    )
    args = parser.parse_args()

    if args.replays and not os.path.isdir(args.replays):
        parser.error(f"--replays {args.replays} is not a directory")
    if args.game and not os.path.isfile(args.game):
        parser.error(f"--game {args.game} is not a file")

    station = Station(args)
    handler = type("BoundHandler", (Handler,), {"station": station})
    server = ThreadingHTTPServer(("0.0.0.0", args.port), handler)
    advertiser = advertise(args.name, args.port)

    def shutdown(_signum, _frame):
        print("\nfake-beamer: stopping")
        if advertiser:
            advertiser.terminate()
        threading.Thread(target=server.shutdown).start()

    signal.signal(signal.SIGINT, shutdown)
    signal.signal(signal.SIGTERM, shutdown)

    print(f"fake-beamer: http://localhost:{args.port}/status")
    try:
        server.serve_forever()
    finally:
        if advertiser:
            advertiser.terminate()


if __name__ == "__main__":
    main()
