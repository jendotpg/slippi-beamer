"""Shared plumbing for beamer tools. For import only.

Two rules for downstream tools:

1. Don't block silently. Anything that can take more than a second announces itself 
   BEFORE it starts (Step context). Anything that can take more than a minute prints a 
   heartbeat while it runs (heartbeat context).
2. Progress goes to stderr, results go to stdout.
"""

from __future__ import annotations

import contextlib
import hashlib
import http.client
import json
import os
import random
import re
import shutil
import socket
import statistics
import struct
import subprocess
import sys
import threading
import time
import zlib
from dataclasses import dataclass, field
from pathlib import Path

import click

HERE = Path(__file__).parent.resolve()
MEASUREMENTS = HERE / "measurements"
HELP_OPTIONS = {"help_option_names": ["-h", "--help"]}

_started = time.monotonic()

def elapsed() -> float:
    return time.monotonic() - _started


def say(message: str) -> None:
    click.echo(f"[{elapsed():6.1f}s] {message}", err=True)


def warn(message: str) -> None:
    click.echo(f"[{elapsed():6.1f}s] {click.style('WARNING', fg='yellow')} {message}", err=True)


def emit(line: str = "") -> None:
    click.echo(line)


def banner(tool: str, **facts: object) -> None:
    shown = "  ".join(f"{k.replace('_', ' ')} {v}" for k, v in facts.items() if v not in (None, ""))
    click.echo(click.style(f"== {tool} ==  {shown}", bold=True), err=True)


class Step:
    def __init__(self, message: str):
        self.message = message
        self.note: str | None = None
        self._t0 = 0.0

    def __enter__(self) -> "Step":
        say(f"{self.message} ...")
        self._t0 = time.monotonic()
        return self

    def done(self, note: str) -> None:
        self.note = note

    def __exit__(self, exc_type, exc, tb) -> bool:
        took = time.monotonic() - self._t0
        if exc_type is not None:
            say(f"  -> failed after {took:.1f}s")
        else:
            say(f"  -> {self.note or 'done'} ({took:.1f}s)")
        return False


@contextlib.contextmanager
def heartbeat(message: str, every: float = 15.0):
    stop = threading.Event()

    def tick() -> None:
        waited = 0.0
        while not stop.wait(every):
            waited += every
            say(f"  {message} (still going, {waited:.0f}s)")

    thread = threading.Thread(target=tick, daemon=True)
    thread.start()
    try:
        yield
    finally:
        stop.set()
        thread.join(timeout=1)


class Counter:
    def __init__(self, total: int, label: str = ""):
        self.total = total
        self.label = label
        self.done = 0

    def update(self, note: str = "") -> None:
        self.done += 1
        if sys.stderr.isatty():
            click.echo(f"\r  {self.label}{self.done}/{self.total}  {note:<24}", err=True, nl=False)
        else:
            say(f"  {self.label}{self.done}/{self.total}  {note}")

    def finish(self) -> None:
        if sys.stderr.isatty():
            click.echo("", err=True)


def is_ipv4(text: str | None) -> bool:
    if not text:
        return False
    parts = text.split(".")
    if len(parts) != 4:
        return False
    return all(part.isdigit() and len(part) <= 3 and int(part) <= 255 for part in parts)


def _dns_sd(arguments: list[str], seconds: float, stop_on=None) -> list[str]:
    if not shutil.which("dns-sd"):
        return []
    try:
        process = subprocess.Popen(
            ["dns-sd", *arguments],
            stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True,
        )
    except OSError:
        return []

    lines: list[str] = []
    reader = threading.Thread(target=lambda: [lines.append(line) for line in process.stdout],
                              daemon=True)
    reader.start()
    deadline = time.monotonic() + seconds
    seen = 0
    while time.monotonic() < deadline:
        if stop_on:
            while seen < len(lines):
                if stop_on(lines[seen]):
                    deadline = 0
                    break
                seen += 1
            if deadline == 0:
                break
        time.sleep(0.05)
    process.terminate()
    with contextlib.suppress(Exception):
        process.wait(timeout=2)
    with contextlib.suppress(Exception):
        process.stdout.close()
    return list(lines)


def _added(line: str) -> bool:
    return "Add" in line.split()


def browse(seconds: float = 5.0) -> list[str]:
    names = set()
    for line in _dns_sd(["-B", "_beamer._tcp"], seconds):
        if _added(line):
            names.add(line.split()[-1])
    return sorted(names)


def resolve(name: str, seconds: float = 3.0) -> str | None:
    if is_ipv4(name):
        return name
    for line in _dns_sd(["-G", "v4", name], seconds, stop_on=_added):
        fields = line.split()
        if _added(line) and len(fields) >= 2 and is_ipv4(fields[-2]):
            return fields[-2]

    if name.endswith(".local"):
        return None
    try:
        return socket.gethostbyname(name)
    except OSError:
        return None


def hostname_for(ip: str, seconds: float = 5.0) -> str | None:
    for name in browse(seconds):
        if resolve(f"{name}.local") == ip:
            return name
    return None


# --- mDNS wire-latency probe ------------------------------------------------
# dns-sd would read mDNSResponder's cache and hide a stall - a silent station
# still looks instant until the cached record ages out ~2 min later - so this
# puts the query on the wire itself, from an ephemeral port (a legacy unicast
# query, RFC 6762 6.7) with the QU bit set: the case the spec is strictest about
# (unique records answered within 10 ms, no random delay). It measures the
# responder, not the fleet view - probe_fleet answers that question.

MDNS_GROUP, MDNS_PORT = "224.0.0.251", 5353
_MDNS_QUERY_UNICAST = 0x8000


def _mdns_encode_name(name: str) -> bytes:
    out = b""
    for label in name.split("."):
        if label:
            out += bytes([len(label)]) + label.encode()
    return out + b"\0"


def _mdns_build_query(query_id: int, name: str) -> bytes:
    header = struct.pack(">HHHHHH", query_id, 0, 1, 0, 0, 0)
    return header + _mdns_encode_name(name) + struct.pack(">HH", 1, _MDNS_QUERY_UNICAST | 1)


def _mdns_skip_name(buffer: bytes, at: int) -> int:
    while at < len(buffer):
        length = buffer[at]
        if length == 0:
            return at + 1
        if length & 0xC0:
            return at + 2
        at += 1 + length
    return at


def _mdns_answer_address(buffer: bytes, query_id: int) -> str | None:
    """The A address in a reply to this query id, or None."""
    if len(buffer) < 12:
        return None
    reply_id, _, questions, answers = struct.unpack(">HHHH", buffer[:8])
    if reply_id != query_id or answers < 1:
        return None
    at = 12
    for _ in range(questions):
        at = _mdns_skip_name(buffer, at) + 4
    for _ in range(answers):
        at = _mdns_skip_name(buffer, at)
        if at + 10 > len(buffer):
            return None
        record_type, _, _, length = struct.unpack(">HHIH", buffer[at:at + 10])
        at += 10
        if record_type == 1 and length == 4:
            return ".".join(str(byte) for byte in buffer[at:at + 4])
        at += length
    return None


def probe_mdns(hostname: str, seconds: float, interval: float = 250.0,
               timeout: float = 1.0) -> dict:
    """Time HOSTNAME's mDNS A answers on the wire for `seconds`.

    A bare name gets .local appended. Returns a dict of the timing (answered,
    sent, timeouts, p50/p95/max in ms, the addresses seen) plus a one-line
    `summary`. Silent - the caller owns any progress output.
    """
    name = hostname if hostname.endswith(".local") else hostname + ".local"
    gap = interval / 1000
    sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    sock.setsockopt(socket.IPPROTO_IP, socket.IP_MULTICAST_TTL, 255)
    sock.settimeout(timeout)

    latencies: list[float] = []
    timeouts = 0
    addresses: set[str] = set()
    deadline = time.monotonic() + seconds
    try:
        while time.monotonic() < deadline:
            query_id = random.randrange(1, 65536)
            started = time.monotonic()
            try:
                sock.sendto(_mdns_build_query(query_id, name), (MDNS_GROUP, MDNS_PORT))
            except OSError:
                timeouts += 1
                time.sleep(gap)
                continue
            got = None
            # Other responders share this group; keep reading until our id returns.
            while time.monotonic() - started < timeout:
                try:
                    buffer, _ = sock.recvfrom(4096)
                except (socket.timeout, TimeoutError):
                    break
                got = _mdns_answer_address(buffer, query_id)
                if got:
                    break
            if got:
                latencies.append((time.monotonic() - started) * 1000)
                addresses.add(got)
            else:
                timeouts += 1
            time.sleep(gap)
    finally:
        sock.close()

    sent = len(latencies) + timeouts
    if not latencies:
        return {"answered": 0, "sent": sent, "timeouts": timeouts, "p50": None,
                "p95": None, "max_ms": None, "addrs": [],
                "summary": f"answered=0/{sent} timeouts={timeouts} NO ANSWERS AT ALL"}
    ordered = sorted(latencies)
    p50, p95, worst = percentile(ordered, 50), percentile(ordered, 95), ordered[-1]
    return {
        "answered": len(latencies), "sent": sent, "timeouts": timeouts,
        "p50": p50, "p95": p95, "max_ms": worst, "addrs": sorted(addresses),
        "summary": (f"answered={len(latencies)}/{sent} timeouts={timeouts} "
                    f"p50={p50:.1f}ms p95={p95:.1f}ms max={worst:.1f}ms "
                    f"addrs={sorted(addresses)}"),
    }


# --- fleet-presence probe ---------------------------------------------------
# Answers the fleet view's question: was the station continuously present on
# mDNS, and did /status keep answering. The app's own resolver (@fugood/dns-sd)
# would measure the exact code path it runs, but that needs node and the app
# checkout; this uses the same dns-sd browse the rest of this library uses, so
# the test suite stays self-contained - at the cost of watching presence through
# mDNSResponder rather than the app's resolver.

_FLEET_POLL_MS = 10000
_STATUS_TIMEOUT_S = 4.0


def _browse_events(seconds: float, on_event) -> None:
    """Stream `dns-sd -B _beamer._tcp` for `seconds`, calling on_event(action,
    name) for each Add/Rmv line. Instance names are hostnames here, so the
    split()[-1] the rest of this module relies on holds."""
    if not shutil.which("dns-sd"):
        return
    try:
        process = subprocess.Popen(
            ["dns-sd", "-B", "_beamer._tcp"],
            stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True,
        )
    except OSError:
        return

    def reader() -> None:
        for line in process.stdout:
            fields = line.split()
            if len(fields) < 7 or fields[1] not in ("Add", "Rmv"):
                continue
            on_event(fields[1], fields[-1])

    thread = threading.Thread(target=reader, daemon=True)
    thread.start()
    time.sleep(seconds)
    process.terminate()
    with contextlib.suppress(Exception):
        process.wait(timeout=2)
    with contextlib.suppress(Exception):
        process.stdout.close()


def probe_fleet(ip: str, host: str | None = None, seconds: float = 180.0,
                poll_ms: float = _FLEET_POLL_MS,
                status_timeout: float = _STATUS_TIMEOUT_S) -> dict:
    """Watch discovery and /status for `seconds`, as the fleet view does.

    `host` is the _beamer._tcp instance name (a hostname); if omitted it is
    resolved from `ip`. Returns present_pct, losses, status_ok/status_fail,
    status_slowest_ms, a nonzero `code` when anything went wrong, and a one-line
    `summary`.
    """
    if host is None:
        host = hostname_for(ip)
    station = Station(ip)
    start = time.monotonic()
    lock = threading.Lock()
    pres = {"present": False, "since": None, "present_s": 0.0, "losses": 0}

    def mark(present_now: bool) -> None:
        now = time.monotonic()
        with lock:
            if present_now and not pres["present"]:
                pres["present"], pres["since"] = True, now
            elif not present_now and pres["present"]:
                pres["present"] = False
                pres["present_s"] += now - pres["since"]
                pres["losses"] += 1

    def on_event(action: str, name: str) -> None:
        if host and name == host:
            mark(action == "Add")

    poll = {"ok": 0, "fail": 0, "slowest": 0.0}
    stop = threading.Event()

    def poller() -> None:
        while True:
            started = time.monotonic()
            answered = station._json("/status", timeout=status_timeout) is not None
            took = (time.monotonic() - started) * 1000
            with lock:
                poll["slowest"] = max(poll["slowest"], took)
                poll["ok" if answered else "fail"] += 1
            if stop.wait(poll_ms / 1000):
                return

    poller_thread = threading.Thread(target=poller, daemon=True)
    poller_thread.start()
    _browse_events(seconds, on_event)
    stop.set()
    poller_thread.join(timeout=status_timeout + 2)

    now = time.monotonic()
    with lock:
        if pres["present"]:
            pres["present_s"] += now - pres["since"]
        losses, present_s = pres["losses"], pres["present_s"]
    total = now - start
    pct = present_s / total * 100 if total else 0.0
    return {
        "present_pct": pct, "losses": losses,
        "status_ok": poll["ok"], "status_fail": poll["fail"],
        "status_slowest_ms": poll["slowest"],
        "code": 1 if (losses > 0 or poll["fail"] > 0) else 0,
        "summary": (f"present={pct:.1f}% of {total:.0f}s losses={losses} "
                    f"status_ok={poll['ok']} status_fail={poll['fail']} "
                    f"status_slowest={poll['slowest']:.0f}ms"),
    }


def find_station(explicit: str | None = None) -> str:
    wanted, source = explicit, "argument"
    if not wanted:
        wanted, source = os.environ.get("BEAMER_HOST"), "BEAMER_HOST"

    if wanted:
        if is_ipv4(wanted):
            say(f"station {wanted} (from {source})")
            return wanted
        with Step(f"resolving {wanted} (from {source})") as step:
            ip = resolve(wanted)
            step.done(ip or "no answer")
        if not ip:
            raise click.ClickException(f'cannot resolve "{wanted}"')
        return ip

    with Step("looking for a beamer over mDNS (up to 5s)") as step:
        names = browse()
        step.done(f"{len(names)} advertising: {', '.join(names) if names else 'none'}")
    for name in names:
        with Step(f"resolving {name}.local") as step:
            ip = resolve(f"{name}.local")
            step.done(ip or "no answer")
        if ip:
            return ip
    raise click.ClickException(
        "no beamer found over mDNS - pass --station <ip>, or set BEAMER_HOST.\n"
        "  (mDNS often does not cross a phone hotspot; read the IP off the screen)"
    )


def station_option(function):
    """--station, shared by every tool that talks to a beamer."""
    return click.option(
        "--station", "station", metavar="NAME-OR-IP", default=None,
        help="Station to test. Defaults to $BEAMER_HOST, then the first beamer on mDNS.",
    )(function)


@dataclass
class Pull:
    status: str          # ok | short | http_error | timeout | error
    code: int | None
    total_seconds: float
    ttfb_seconds: float
    decoded_bytes: int   # what the client app ends up with
    wire_bytes: int      # what crossed the air
    digest: str
    error: str = ""

    @property
    def kilobytes_per_second(self) -> float:
        return self.decoded_bytes / self.total_seconds / 1024 if self.total_seconds > 0 else 0.0


class Station:
    def __init__(self, ip: str):
        self.ip = ip
        self._index_high_water = 0

    def _json(self, path: str, timeout: float = 8.0) -> dict | None:
        try:
            connection = http.client.HTTPConnection(self.ip, 80, timeout=timeout)
            connection.request("GET", path, headers={"Accept-Encoding": "identity"})
            response = connection.getresponse()
            body = response.read()
            connection.close()
            if response.status != 200:
                return None
            return json.loads(body)
        except (OSError, http.client.HTTPException, ValueError, socket.timeout):
            return None

    def status(self) -> dict | None:
        return self._json("/status")

    def heap(self) -> dict | None:
        """Live heap. The only debug endpoint that reads the current boot."""
        return self._json("/debug/heap")

    def transfer(self) -> dict | None:
        return self._json("/debug/transfer")

    def index(self) -> dict | None:
        return self._json("/SLIPPI/", timeout=10)

    def files(self) -> list[dict]:
        index = self.index()
        return (index or {}).get("files") or []

    def largest_file(self) -> tuple[str, int] | None:
        files = self.files()
        if not files:
            return None
        biggest = max(files, key=lambda entry: entry["size"])
        return biggest["url"], biggest["size"]

    def alive(self) -> bool:
        if self.heap() is not None:
            return True
        time.sleep(3)
        return self.heap() is not None

    def note_index(self) -> None:
        index = self.index()
        if index:
            count = index.get("served_replay_count", 0)
            self._index_high_water = max(self._index_high_water, count)

    def rebooted(self) -> bool:
        index = self.index()
        if index is None or self._index_high_water == 0:
            return False
        return index.get("served_replay_count", 0) == 0

    def forget_index(self) -> None:
        self._index_high_water = 0

    def pull(
        self,
        path: str,
        *,
        expect: int | None = None,
        gzip: bool = False,
        gzip_level: int | None = None,
        resume_from: int | None = None,
        range_from: int | None = None,
        timeout: float = 120.0,
        sink: Path | None = None,
    ) -> Pull:
        headers = {"Accept-Encoding": "gzip" if gzip else "identity"}
        if gzip and gzip_level is not None:
            headers["X-Beamer-Gz-Level"] = str(gzip_level)
        if resume_from is not None:
            headers["X-Replay-From"] = str(resume_from)
        if range_from is not None:
            # The uncompressed resume path: the station answers 206 with the
            # remainder as identity bytes. Range wins over gzip on the station.
            headers["Range"] = f"bytes={range_from}-"

        hasher = hashlib.sha256()
        decompressor = zlib.decompressobj(16 + zlib.MAX_WBITS)
        wire = decoded = 0
        handle = sink.open("wb") if sink else None
        started = time.monotonic()
        ttfb = 0.0
        connection = None
        try:
            connection = http.client.HTTPConnection(self.ip, 80, timeout=timeout)
            connection.request("GET", path, headers=headers)
            response = connection.getresponse()
            ttfb = time.monotonic() - started
            compressed = response.getheader("Content-Encoding", "").lower() == "gzip"
            deadline = started + timeout
            while True:
                if time.monotonic() > deadline:
                    return Pull("timeout", response.status, time.monotonic() - started,
                                ttfb, decoded, wire, "")
                chunk = response.read(32768)
                if not chunk:
                    break
                wire += len(chunk)
                plain = decompressor.decompress(chunk) if compressed else chunk
                if plain:
                    decoded += len(plain)
                    hasher.update(plain)
                    if handle:
                        handle.write(plain)
            if compressed:
                tail = decompressor.flush()
                if tail:
                    decoded += len(tail)
                    hasher.update(tail)
                    if handle:
                        handle.write(tail)
            total = time.monotonic() - started
            code = response.status
            ok_code = 206 if range_from is not None else 200
            if code != ok_code:
                status = "http_error"
            elif expect is not None and decoded != expect:
                status = "short"
            else:
                status = "ok"
            return Pull(status, code, total, ttfb, decoded, wire, hasher.hexdigest()[:12])
        except (socket.timeout, TimeoutError):
            return Pull("timeout", None, time.monotonic() - started, ttfb, decoded, wire, "")
        except (OSError, http.client.HTTPException, zlib.error) as problem:
            return Pull("error", None, time.monotonic() - started, ttfb, decoded, wire, "",
                        error=f"{type(problem).__name__}: {problem}")
        finally:
            if handle:
                handle.close()
            if connection:
                with contextlib.suppress(Exception):
                    connection.close()

    def describe(self) -> str:
        status = self.status()
        if not status:
            return f"{self.ip} (no /status - is it up?)"
        game = status.get("game") or {}
        bits = [
            status.get("station_name", "?"),
            f"fw {status.get('firmware_version', '?')}",
            f"ssid {status.get('ssid', '?')}",
            f"rssi {status.get('rssi', '?')} dBm",
            f"ch {status.get('channel', '?')}",
            f"replays {status.get('replay_count', '?')}",
            f"live={str(game.get('live')).lower()}",
        ]
        return f"{self.ip}  " + "  ".join(str(bit) for bit in bits)


def run_directory(tool: str, run_name: str) -> Path:
    """measurements/<tool>-<run-name> - output dir for a tool"""
    if "/" in run_name or run_name.startswith("."):
        raise click.ClickException(f'"{run_name}" is not usable as a run name')
    out = MEASUREMENTS / f"{tool}-{run_name}"
    if out.exists():
        raise click.ClickException(
            f"{out} already exists - pick a fresh run name, or remove it first"
        )
    out.mkdir(parents=True)
    return out


def run_csv(tool: str, run_name: str) -> Path:
    """measurements/<tool>-<run-name>.csv - output file for a tool"""
    if "/" in run_name or run_name.startswith("."):
        raise click.ClickException(f'"{run_name}" is not usable as a run name')
    MEASUREMENTS.mkdir(parents=True, exist_ok=True)
    out = MEASUREMENTS / f"{tool}-{run_name}.csv"
    if out.exists():
        raise click.ClickException(
            f"{out} already exists - pick a fresh run name, or remove it first"
        )
    return out


def percentile(sorted_values: list[float], point: float) -> float:
    if not sorted_values:
        raise ValueError("no values")
    index = int(round(point / 100 * (len(sorted_values) - 1)))
    return sorted_values[min(len(sorted_values) - 1, max(0, index))]


def median(values) -> float:
    return statistics.median(values)


@dataclass
class AirScan:
    """Mac-specific. Sorry."""
    signal: list[int] = field(default_factory=list)
    noise: list[int] = field(default_factory=list)
    rates: list[int] = field(default_factory=list)
    mine: dict = field(default_factory=dict)
    channels_24: dict = field(default_factory=dict)   # channel -> {ssid}
    channels_5: dict = field(default_factory=dict)
    scans: int = 0

    def contention(self) -> dict[int, int]:
        return {
            channel: sum(len(names) for other, names in self.channels_24.items()
                         if abs(other - channel) <= 2)
            for channel in (1, 6, 11)
        }

    def snr(self) -> list[int]:
        return [high - low for high, low in zip(self.signal, self.noise)]


def airport_scan() -> str:
    try:
        return subprocess.run(
            ["system_profiler", "SPAirPortDataType"],
            capture_output=True, text=True, timeout=40,
        ).stdout
    except (OSError, subprocess.TimeoutExpired):
        return ""


def parse_air(text: str, into: AirScan | None = None) -> AirScan:
    scan = into or AirScan()
    scan.scans += 1
    if "Current Network Information:" in text:
        block = text.split("Current Network Information:", 1)[1][:600]

        def field_of(key: str) -> str | None:
            found = re.search(rf"{key}:\s*(.+)", block)
            return found.group(1).strip() if found else None

        pair = re.match(r"(-?\d+)\s*dBm\s*/\s*(-?\d+)\s*dBm", field_of("Signal / Noise") or "")
        if pair:
            scan.signal.append(int(pair.group(1)))
            scan.noise.append(int(pair.group(2)))
        rate = field_of("Transmit Rate")
        if rate and rate.isdigit():
            scan.rates.append(int(rate))
        scan.mine.setdefault("channel", field_of("Channel"))
        scan.mine.setdefault("phy", field_of("PHY Mode"))

    tail = text.split("Other Local Wi-Fi Networks", 1)
    if len(tail) > 1:
        for block in re.split(r"\n {12}(?=\S)", tail[1]):
            name = block.split(":", 1)[0].strip()
            band_24 = re.search(r"Channel:\s*(\d+)\s*\(2GHz", block)
            band_5 = re.search(r"Channel:\s*(\d+)\s*\(5GHz", block)
            if band_24:
                scan.channels_24.setdefault(int(band_24.group(1)), set()).add(name)
            elif band_5:
                scan.channels_5.setdefault(int(band_5.group(1)), set()).add(name)
    return scan
