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

The game payload is not canned: it comes from the real slp-peek, compiled from
image/scripts/slp-peek.c, run against a real .slp exactly as status-check.sh
does on a station. That is what makes the character icons in the app a real
test rather than a drawing exercise. Build it once with:

    cc -Os -Wall -o /tmp/slp-peek image/scripts/slp-peek.c

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
from datetime import datetime, timezone
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

SCHEMA = 1
DEFAULT_SERVED = 10


def iso_now():
    return datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def find_slp_peek(explicit):
    """The real thing if we can find it, else nothing and we report no game."""
    for candidate in (explicit, "/tmp/slp-peek", shutil.which("slp-peek")):
        if candidate and os.path.isfile(candidate) and os.access(candidate, os.X_OK):
            return candidate
    return None


class Station:
    """The mutable half: what this station currently claims about itself."""

    def __init__(self, args):
        self.args = args
        self.station_id = args.station or str(uuid.uuid5(uuid.NAMESPACE_DNS, args.name))
        self.station_name = args.station_name or ""
        self.boot = iso_now()
        self.started = time.monotonic()
        self.slp_peek = find_slp_peek(args.slp_peek)
        self.lock = threading.Lock()
        self.generated = iso_now()
        self.game = self.read_game()

    def read_game(self):
        """Run the real slp-peek, the same way status-check.sh does."""
        if not self.args.game or not self.slp_peek:
            return None
        try:
            out = subprocess.run(
                [self.slp_peek, self.args.game],
                capture_output=True,
                timeout=10,
                check=True,
            )
        except (subprocess.SubprocessError, OSError) as e:
            print(f"fake-beamer: slp-peek failed: {e}", file=sys.stderr)
            return None
        try:
            return json.loads(out.stdout)
        except json.JSONDecodeError:
            print(
                "fake-beamer: slp-peek printed something unparseable", file=sys.stderr
            )
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
        """What POST /status does: re-run the checks, restamp the report."""
        with self.lock:
            self.game = self.read_game()
            self.generated = iso_now()

    def status(self):
        with self.lock:
            game, generated = self.game, self.generated
        return {
            "schema": SCHEMA,
            "beamer_arch": self.args.arch,
            "generated": generated,
            "station": self.station_id,
            "station_name": self.station_name,
            "host": self.args.name,
            "boot": self.boot,
            "uptime_s": int(time.monotonic() - self.started),
            "wifi": self.args.wifi,
            "network": "ok",
            "ip": "127.0.0.1",
            "slippi_files": len(self.replays()),
            "slippi_files_capped": False,
            "udc": "fake",
            "host_state": "configured",
            "mtools": True,
            "httpd": True,
            "sshd": True,
            "mdns": True,
            "game": game,
            "result": "fail" if self.args.unhealthy else "pass",
            "errors": (
                ["fake-beamer was started with --unhealthy"]
                if self.args.unhealthy
                else []
            ),
        }

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
                    "name": name,
                    "size": size,
                    "mtime": iso_now(),
                    "url": f"/SLIPPI/{name}",
                }
            )
        return {
            "schema": SCHEMA,
            "station": self.station_id,
            "generated": iso_now(),
            "count": len(files),
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
        self.send_response(200)
        self.send_header("Content-Type", "application/octet-stream")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

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
            f"fake-beamer: {cmd[0]} not found; serving HTTP but not advertising. "
            f"Type the address into the app by hand.",
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
    parser.add_argument(
        "--arch",
        default="armhf",
        help="beamer_arch value; a station reports the target it was built for",
    )
    parser.add_argument("--replays", default="", help="directory of .slp to serve")
    parser.add_argument("--game", default="", help=".slp to report as the current game")
    parser.add_argument("--served", type=int, default=DEFAULT_SERVED)
    parser.add_argument("--slp-peek", default="", help="path to a built slp-peek")
    parser.add_argument("--unhealthy", action="store_true", help='report result "fail"')
    parser.add_argument("--unreported", action="store_true", help="503 on GET /status")
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
    if args.game and not station.slp_peek:
        print(
            "fake-beamer: no slp-peek found, so no game will be reported.\n"
            "             cc -Os -Wall -o /tmp/slp-peek image/scripts/slp-peek.c",
            file=sys.stderr,
        )

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
