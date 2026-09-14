#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.10"
# dependencies = ["click"]
# ///
"""Tries to kill a station over HTTP. This is the de-facto test suite.

Runs four attacks then soaks until interrupted. Each result lands as one
row in the output CSV (measurements/stress_station-<run>.csv). Details are
mirrored to the terminal as they come.

Point this at a live beamer that is serving at least one game and plugged into
a real Wii. For the first test, keep the Wii in a live game.
"""

import concurrent.futures
import csv
import hashlib
import shutil
import socket
import sys
import tempfile
import threading
import time
from pathlib import Path

import click

sys.dont_write_bytecode = True

import beamer_lib as beamer

TOOL = "stress_station"
PHASES = ["writes", "churn", "stalls", "mdns", "soak"]


CSV_COLUMNS = ["elapsed_s", "phase", "kind", "label", "verdict", "value", "unit", "note"]


class Report:
    def __init__(self, path: Path):
        self.path = path
        self.handle = path.open("w", newline="", buffering=1)
        self.writer = csv.writer(self.handle)
        self.writer.writerow(CSV_COLUMNS)
        self.started = time.monotonic()
        self.phase = ""

    def _row(self, kind: str, label: str, verdict: str, value, unit: str, note: str) -> None:
        elapsed = f"{time.monotonic() - self.started:.1f}"
        self.writer.writerow([elapsed, self.phase, kind, label, verdict,
                              "" if value is None else value, unit, note])

    def heading(self, phase: str, text: str) -> None:
        self.phase = phase
        beamer.say(click.style(text, bold=True))

    def line(self, text: str) -> None:
        beamer.say(text)

    def meta(self, label: str, value, unit: str = "", note: str = "") -> None:
        self._row("meta", label, "", value, unit, note)

    def measure(self, label: str, value, unit: str = "", note: str = "") -> None:
        self._row("measure", label, "", value, unit, note)

    def event(self, note: str, label: str = "") -> None:
        self._row("event", label, "", None, "", note)
        beamer.say(note)

    def verdict(self, text: str) -> None:
        outcome = ("PASS" if " PASS" in text else "SKIP" if " SKIP" in text
                   else "PARTIAL" if " PARTIAL" in text else "FAIL")
        self._row("verdict", "", outcome, None, "", text)
        colour = {"PASS": "green", "SKIP": "yellow", "PARTIAL": "yellow"}.get(outcome, "red")
        beamer.say(click.style(f"SUMMARY  {text}", fg=colour, bold=True))


def wait_alive(station: beamer.Station, report: Report, during: str) -> None:
    """A station that dies is power-cycled by a human. Wait for the rescue."""
    if station.alive() and not station.rebooted():
        return
    report.event(f"STATION UNREACHABLE OR RESTARTED during {during}", label="unreachable")
    waited = 0
    while not station.alive():
        time.sleep(15)
        waited += 15
        if waited % 300 == 0:
            report.line(f"  still down after {waited}s - power-cycle it if it is not coming back")
    report.event(f"answering again after {waited}s", label="recovered")
    station.forget_index()


def phase_writes(station: beamer.Station, report: Report, rounds: int) -> None:
    report.heading("writes", "TEST 1: concurrent pulls while the Wii writes")
    target = station.largest_file()
    if target is None:
        report.verdict("TEST1 SKIP (nothing published)")
        return
    url, size = target
    report.line(f"target {url} ({size} B)")
    report.meta("target", url, note=f"{size} B")

    complete = refused = truncated = 0
    digests = set()
    counter = beamer.Counter(rounds, "round ")
    for number in range(1, rounds + 1):
        with beamer.heartbeat(f"round {number}/{rounds}: 3 concurrent pulls", every=20):
            with concurrent.futures.ThreadPoolExecutor(max_workers=3) as pool:
                pulls = [pool.submit(station.pull, url, expect=size, gzip=True, timeout=250)
                         for _ in range(3)]
                results = [future.result() for future in pulls]
        for pull in results:
            if pull.code == 503 or (pull.status != "ok" and pull.decoded_bytes == 0):
                refused += 1
            elif pull.status == "ok":
                complete += 1
                digests.add(pull.digest)
            else:
                truncated += 1
        counter.update(f"complete={complete} refused={refused} truncated={truncated}")
        if not station.alive():
            break
    counter.finish()

    heap = station.heap() or {}
    report.line(f"complete={complete} refused={refused} truncated={truncated} "
                f"distinct_hashes={len(digests)} heap={heap}")
    report.measure("complete", complete, note=f"{rounds} rounds x 3 pulls")
    report.measure("refused", refused)
    report.measure("truncated", truncated)
    report.measure("distinct_hashes", len(digests))
    report.measure("free", heap.get("free"), "B", "after phase")
    report.measure("oom_count", heap.get("oom_count"), note="after phase")
    if station.rebooted():
        report.verdict("TEST1 FAIL  station RESTARTED (index emptied)")
    elif truncated:
        report.verdict(f"TEST1 FAIL  {truncated} truncated responses")
    elif len(digests) > 1:
        report.verdict(f"TEST1 FAIL  {len(digests)} distinct hashes among {complete} complete pulls")
    else:
        report.verdict(f"TEST1 PASS  {complete} complete pulls identical, "
                       f"{refused} refused (expected: only 2 pcbs)")


def phase_churn(station: beamer.Station, report: Report) -> None:
    report.heading("churn", "TEST 2: connection churn, ramped")
    target = station.largest_file()
    paths = ["/status", "/SLIPPI/", "/debug/heap"] + ([target[0]] if target else [])

    for parallel in (2, 4, 6, 10):
        if not station.alive():
            break
        before = (station.heap() or {}).get("free")
        requests = [paths[index % len(paths)] for index in range(120)]
        codes: dict[str, int] = {}
        with beamer.Step(f"{len(requests)} requests at parallelism {parallel}") as step, \
                beamer.heartbeat(f"churning at parallelism {parallel}", every=20):
            with concurrent.futures.ThreadPoolExecutor(max_workers=parallel) as pool:
                for pull in pool.map(lambda path: station.pull(path, timeout=25), requests):
                    key = str(pull.code) if pull.code else pull.status
                    codes[key] = codes.get(key, 0) + 1
            step.done(" ".join(f"{key}x{count}" for key, count in sorted(codes.items())))
        heap = station.heap() or {}
        after = heap.get("free")
        codes_note = " ".join(f"{key}x{count}" for key, count in sorted(codes.items()))
        report.line(f"parallel={parallel} codes: {codes} free {before}->{after} "
                    f"oom={heap.get('oom_count')}")
        report.measure("free_before", before, "B", f"parallel={parallel}")
        report.measure("free_after", after, "B", f"parallel={parallel} codes: {codes_note}")
        report.measure("oom_count", heap.get("oom_count"), note=f"parallel={parallel}")
        if station.rebooted():
            report.verdict(f"TEST2 FAIL  RESTARTED at parallelism {parallel}")
            return
        if not station.alive():
            report.verdict(f"TEST2 FAIL  stopped answering at parallelism {parallel}")
            return
        time.sleep(20)
    report.verdict("TEST2 PASS  survived churn to 10-way (refusals expected, not failures)")



def stall_level(station: beamer.Station, report: Report, url: str, count: int,
                pullers: int, baseline: dict, seconds: int) -> dict:
    """Holds `count` half-open connections and `pullers` real pulls at the same
    time. An OOM here is allowed (this is way more stations than are allowed). 
    However, the heap is expected to return to baseline.
    """
    stop = threading.Event()
    lock = threading.Lock()
    tally = {"pulls": 0, "refused": 0, "failed": 0}
    digests: set[str] = set()

    def puller() -> None:
        while not stop.is_set():
            pull = station.pull(url, gzip=True, timeout=120)
            with lock:
                if pull.status == "ok":
                    tally["pulls"] += 1
                    digests.add(pull.digest)
                elif pull.code == 503:
                    tally["refused"] += 1
                else:
                    tally["failed"] += 1
            if pull.status != "ok":
                stop.wait(1)

    base_free = baseline.get("free", 0)
    base_block = baseline.get("largest_block", 0)
    base_dma = baseline.get("dma_largest", 0)

    oom_start = (station.heap() or {}).get("oom_count", 0)
    report.line(f"before: {station.heap()}")
    held = []
    for index in range(count):
        try:
            sock = socket.create_connection((station.ip, 80), timeout=10)
            sock.sendall(
                f"GET {url} HTTP/1.1\r\nHost: {station.ip}\r\n"
                f"Accept-Encoding: gzip\r\n\r\n".encode()
            )
            sock.setblocking(False)
            held.append(sock)
        except OSError as problem:
            report.line(f"  stall {index + 1} refused: {problem}")
    report.line(f"  {len(held)} stalled connections held, {pullers} pullers hammering")

    threads = [threading.Thread(target=puller, daemon=True) for _ in range(pullers)]
    for thread in threads:
        thread.start()

    watch = {"seen": False, "peak": oom_start, "largest": 0, "caps": "", "fn": "",
             "min_dma": None, "min_block": None, "min_free": None, "polls": 0}
    wlock = threading.Lock()
    watch_stop = threading.Event()

    def heap_watch() -> None:
        while not watch_stop.is_set():
            heap = station.heap()
            if heap:
                with wlock:
                    watch["polls"] += 1
                    seen_count = heap.get("oom_count", 0)
                    watch["peak"] = max(watch["peak"], seen_count)
                    if seen_count > oom_start and not watch["seen"]:
                        watch.update(seen=True, largest=heap.get("oom_largest", 0),
                                     caps=heap.get("oom_caps", ""), fn=heap.get("oom_fn", ""))
                    dma = heap.get("dma_largest")
                    block = heap.get("largest_block")
                    free = heap.get("free")
                    if dma is not None:
                        watch["min_dma"] = dma if watch["min_dma"] is None else min(watch["min_dma"], dma)
                    if block is not None:
                        watch["min_block"] = block if watch["min_block"] is None else min(watch["min_block"], block)
                    if free is not None:
                        watch["min_free"] = free if watch["min_free"] is None else min(watch["min_free"], free)
            watch_stop.wait(0.25)

    watcher = threading.Thread(target=heap_watch, daemon=True)
    watcher.start()

    oom_logged = False
    for moment in range(0, seconds + 1, 15):
        with wlock:
            snap = dict(watch)
        report.line(f"t+{moment:>3}s min_free={snap['min_free']} min_block={snap['min_block']} "
                    f"min_dma_block={snap['min_dma']} oom_seen={snap['seen']} peak_oom={snap['peak']} "
                    f"heap_polls={snap['polls']}  "
                    f"pulls={tally['pulls']} 503={tally['refused']} fail={tally['failed']}")
        if snap["seen"] and not oom_logged:
            report.line(f"  ** OOM ** failed alloc {snap['largest']} B caps {snap['caps']} "
                        f"in {snap['fn']} (detail caught before it cleared above HEAP_FLOOR)")
            oom_logged = True
        for sock in held:
            try:
                sock.recv(1)
            except OSError:
                pass
        time.sleep(15)

    watch_stop.set()
    watcher.join(timeout=2)
    stop.set()
    for sock in held:
        try:
            sock.close()
        except OSError:
            pass

    best_free = best_block = best_dma = 0
    final_oom = oom_start
    for _ in range(7):
        time.sleep(5)
        heap = station.heap() or {}
        best_free = max(best_free, heap.get("free", 0))
        best_block = max(best_block, heap.get("largest_block", 0))
        best_dma = max(best_dma, heap.get("dma_largest", 0))
        final_oom = max(final_oom, heap.get("oom_count", oom_start))
    oom_delta = final_oom - oom_start
    recovered = (best_free >= base_free * 0.9 and best_block >= base_block * 0.9
                 and (base_dma == 0 or best_dma >= base_dma * 0.9))
    with wlock:
        detail = dict(watch)
    report.line(f"after:  free {best_free}/{base_free}  block {best_block}/{base_block}  "
                f"dma_block {best_dma}/{base_dma}  recovered={recovered}  "
                f"ooms={oom_delta}  distinct_hashes={len(digests)}  ok_pulls={tally['pulls']}")

    return {
        "oomed": oom_delta > 0, "oom_delta": oom_delta, "recovered": recovered,
        "detail_captured": detail["seen"], "largest": detail["largest"],
        "caps": detail["caps"], "fn": detail["fn"], "min_block": detail["min_block"],
        "min_dma": detail["min_dma"], "min_free": detail["min_free"],
        "ok_pulls": tally["pulls"], "distinct_hashes": len(digests),
        "after_free": best_free, "after_block": best_block, "after_dma": best_dma,
    }


def _ladder(gate: int, ceiling: int) -> list[int]:
    """gate*2, gate*4, ... up to ceiling - the rungs to sweep above the gate."""
    rungs, value = [], gate * 2
    while value <= ceiling:
        rungs.append(value)
        value *= 2
    return rungs


def phase_stalls(station: beamer.Station, report: Report, gate: int,
                 ceiling: int) -> None:
    report.heading("stalls", "TEST 3: stalls x pullers - gate, then sweep each axis")
    target = station.largest_file()
    if target is None:
        report.verdict("TEST3 SKIP (nothing published)")
        return
    url, _ = target

    baseline = station.heap() or {}
    if not baseline.get("free"):
        report.verdict("TEST3 SKIP (no heap reading for a baseline)")
        return
    report.line(f"baseline: free {baseline['free']}, largest_block "
                f"{baseline.get('largest_block')}, dma_largest {baseline.get('dma_largest')}")
    report.measure("baseline_free", baseline["free"], "B")
    report.measure("baseline_largest_block", baseline.get("largest_block"), "B")
    report.measure("baseline_dma_largest", baseline.get("dma_largest"), "B")

    def run(stalls: int, pullers: int) -> tuple[bool, str]:
        """One level. A pass is: recovered to baseline, no reboot, no corruption.
        An OOM that recovers is NOT a failure - that is the whole point."""
        if not station.alive():
            return False, "station unreachable"
        report.line(f"--- {stalls} stalls x {pullers} pullers ---")
        with beamer.heartbeat(f"{stalls} stalls, {pullers} pullers", every=30):
            result = stall_level(station, report, url, stalls, pullers, baseline, seconds=90)
        rung = f"{stalls}x{pullers}"
        report.measure("oom_delta", result["oom_delta"], note=rung)
        report.measure("recovered", int(result["recovered"]), note=rung)
        report.measure("min_free", result["min_free"], "B", rung)
        report.measure("min_block", result["min_block"], "B", rung)
        report.measure("min_dma", result["min_dma"], "B", rung)
        report.measure("ok_pulls", result["ok_pulls"], note=rung)
        report.measure("distinct_hashes", result["distinct_hashes"], note=rung)
        report.measure("after_free", result["after_free"], "B", rung)
        report.measure("after_block", result["after_block"], "B", rung)
        report.measure("after_dma", result["after_dma"], "B", rung)
        if result["oomed"] and result["detail_captured"]:
            report.measure("oom_largest", result["largest"], "B",
                           f"{rung} caps {result['caps']} in {result['fn']}")
        if station.rebooted():
            return False, "station RESTARTED"
        if not station.alive():
            return False, "stopped answering"
        if result["distinct_hashes"] > 1:
            return False, f"{result['distinct_hashes']} distinct hashes - transfer corrupted"
        if result["oomed"]:
            detail = (f"failed alloc {result['largest']} B caps {result['caps']}"
                      if result["detail_captured"]
                      else "detail not captured - poller starved under load")
            oom_note = f"OOMed x{result['oom_delta']} ({detail})"
        else:
            oom_note = "no OOM"
        if not result["recovered"]:
            return False, (f"{oom_note}; heap did NOT return to baseline "
                           f"(free {result['after_free']}/{baseline['free']}, "
                           f"dma_block {result['after_dma']}/{baseline.get('dma_largest')})")
        report.line(f"  PASS: {oom_note}; recovered "
                    f"(floors: block {result['min_block']}, dma {result['min_dma']}, "
                    f"free {result['min_free']})")
        time.sleep(20)
        return True, oom_note

    passed, note = run(gate, gate)
    if not passed:
        report.verdict(f"TEST3 FAIL  did not survive the {gate} stalls x {gate} pullers "
                       f"baseline: {note}")
        return
    report.line(f"gate {gate}x{gate} held ({note}) - sweeping each axis to {ceiling}")

    def sweep(axis: str) -> str:
        reached = gate
        for rung in _ladder(gate, ceiling):
            stalls, pullers = (rung, gate) if axis == "stalls" else (gate, rung)
            ok, why = run(stalls, pullers)
            if not ok:
                report.line(f"{axis} axis: stopped at {rung} - {why}")
                return f"{axis} to {reached} (failed at {rung})"
            reached = rung
        return f"{axis} to {reached}+ (ceiling)"

    stalls_result = sweep("stalls")
    pullers_result = sweep("pullers")
    report.verdict(f"TEST3 PASS  gate {gate}x{gate} held; {stalls_result}; {pullers_result}")


def phase_mdns(station: beamer.Station, report: Report) -> None:
    report.heading("mdns", "TEST 4: does the station stay on the fleet view under load")
    with beamer.Step("asking Bonjour which instance answers on this IP") as step:
        host = beamer.hostname_for(station.ip)
        step.done(host or "no _beamer._tcp instance")
    if not host:
        report.verdict(f"TEST4 SKIP (no _beamer._tcp instance on {station.ip}; "
                       "set BEAMER_HOST=<hostname>)")
        return
    status = station.status() or {}
    report.line(f"station hostname {host}, rssi {status.get('rssi', '?')} dBm")
    report.measure("rssi", status.get("rssi"), "dBm")
    target = station.largest_file()
    if target is None:
        report.verdict("TEST4 SKIP (nothing published to transfer)")
        return
    url, size = target

    warm, load = 40, 240

    report.line(f"--- baseline: no load, {warm}s ---")
    with beamer.heartbeat("baseline fleet probe", every=15):
        baseline = beamer.probe_fleet(station.ip, host, warm)
    report.line(f"baseline: {baseline['summary']}")
    report.measure("fleet_baseline", baseline["losses"],
                   note=f"losses; {baseline['summary']}")

    report.line(f"--- loaded: continuous {size}B transfers, {load}s ---")

    stop = threading.Event()

    def keep_pulling() -> None:
        while not stop.is_set():
            station.pull(url, gzip=True, timeout=250)

    puller = threading.Thread(target=keep_pulling, daemon=True)
    puller.start()

    wire_result: dict = {}

    def run_wire() -> None:
        wire_result["mdns"] = beamer.probe_mdns(host, load, interval=250.0)

    wire_thread = threading.Thread(target=run_wire)
    wire_thread.start()
    with beamer.heartbeat("loaded window", every=30):
        loaded = beamer.probe_fleet(station.ip, host, load)
        wire_thread.join()

    stop.set()
    puller.join(timeout=260)

    wire_summary = wire_result.get("mdns", {}).get("summary", "(none)")
    report.line(f"loaded:   {loaded['summary']}")
    report.line(f"wire:     {wire_summary}")

    losses = loaded["losses"]
    failures = loaded["status_fail"]
    report.measure("losses", losses, note=f"under load; {loaded['summary']}")
    report.measure("status_fail", failures, note="under load")
    report.measure("wire", wire_summary, note="probe_mdns under load")
    if station.rebooted():
        report.verdict("TEST4 FAIL  station RESTARTED under load")
    elif losses:
        report.verdict(f"TEST4 FAIL  fleet view DROPPED the station {losses}x under load: {loaded['summary']}")
    elif failures:
        report.verdict(f"TEST4 FAIL  {failures} /status polls failed under load: {loaded['summary']}")
    elif loaded["code"]:
        report.verdict(f"TEST4 FAIL  fleet probe reported a problem: {loaded['summary']}")
    else:
        report.verdict(f"TEST4 PASS  station stayed on the fleet view for the whole {load}s: {loaded['summary']}")


def soak_round(station: beamer.Station, report: Report, scratch: Path,
               number: int, url: str, size: int) -> int:
    failures = 0
    plain_file = scratch / "soak_identity.bin"
    plain = station.pull(url, expect=size, gzip=False, timeout=250, sink=plain_file)
    if plain.status != "ok":
        report.line(f"round {number} identity {plain.status} "
                    f"bytes={plain.decoded_bytes} (skipped)")
        return -1

    zipped = station.pull(url, expect=size, gzip=True, timeout=250)
    if zipped.status == "ok" and zipped.digest != plain.digest:
        failures += 1
        report.line(f"!! round {number} GZIP != IDENTITY "
                    f"identity={plain.digest} gzip={zipped.digest}")

    offset = size // 3
    if offset > 1000:
        prefix = plain_file.read_bytes()[:offset]

        def splice(label: str, **resume) -> int:
            tail_file = scratch / f"soak_tail_{label}.bin"
            station.pull(url, timeout=250, sink=tail_file, **resume)
            rebuilt = prefix + tail_file.read_bytes()
            if len(rebuilt) != size:
                return 0
            digest = hashlib.sha256(rebuilt).hexdigest()[:12]
            if digest == plain.digest:
                return 0
            report.line(f"!! round {number} {label.upper()} SPLICE MISMATCH at "
                        f"{offset} got={digest} want={plain.digest}")
            return 1

        failures += splice("gzip", gzip=True, resume_from=offset)
        failures += splice("range", gzip=False, range_from=offset)
    return failures


def phase_soak(station: beamer.Station, report: Report, scratch: Path) -> None:
    report.heading("soak", "SOAK (continuous - Ctrl-C to stop)")
    number = failures = 0
    while True:
        number += 1
        if not station.alive():
            wait_alive(station, report, f"soak round {number}")
            continue
        station.note_index()
        target = station.largest_file()
        if target is None:
            report.line(f"round {number}: nothing published, waiting 30s")
            time.sleep(30)
            continue
        url, size = target

        with beamer.heartbeat(f"soak round {number} ({size / 1e6:.1f} MB x3)", every=30):
            found = soak_round(station, report, scratch, number, url, size)
        if found < 0:
            time.sleep(10)
            continue
        failures += found

        heap = station.heap() or {}
        if heap.get("oom_count"):
            failures += 1
            report.line(f"!! round {number} OOM_COUNT={heap['oom_count']}")
        if station.rebooted():
            failures += 1
            report.event(f"round {number} STATION RESTARTED", label="restarted")
            station.forget_index()
        if number % 10 == 0:
            report.verdict(f"SOAK round {number} failures={failures} heap={heap}")
        report.line(f"round {number} ok size={size} free={heap.get('free')} "
                    f"oom={heap.get('oom_count')}")
        note = f"round={number}"
        report.measure("soak_free", heap.get("free"), "B", note)
        report.measure("soak_oom", heap.get("oom_count"), note=note)
        report.measure("soak_failures", failures, note=note)
        for leftover in scratch.glob("*.bin"):
            leftover.unlink(missing_ok=True)
        time.sleep(10)


@click.command(context_settings=beamer.HELP_OPTIONS)
@click.argument("run_name")
@beamer.station_option
@click.option("--phase", "phases", multiple=True,
              type=click.Choice(PHASES + ["all"]), default=("all",), show_default=True,
              help="Phase to run. Repeat for several. 'all' runs them in order and "
                   "then soaks until interrupted.")
@click.option("--rounds", type=int, default=12, show_default=True,
              help="Rounds of concurrent pulls in the writes phase.")
@click.option("--gate", type=int, default=8, show_default=True,
              help="Stalls-phase baseline. The station must survive this many stalls "
                   "AND this many concurrent pullers - recovering from any OOM - to "
                   "pass; both axes are then swept above it.")
@click.option("--ceiling", type=int, default=64, show_default=True,
              help="Top of each stalls-phase sweep. After the gate passes, stalls "
                   "(pullers pinned at the gate) and pullers (stalls pinned at the "
                   "gate) are each pushed gate*2, gate*4, ... up to here or the first "
                   "level that fails to recover.")
def main(run_name, station, phases, rounds, gate, ceiling):
    """Attack one station over HTTP and write measurements/<run-name>.csv."""
    chosen = PHASES if "all" in phases else [phase for phase in PHASES if phase in phases]
    beamer.banner(TOOL, run=run_name, phases=",".join(chosen))

    out = beamer.run_csv(TOOL, run_name)
    scratch = Path(tempfile.mkdtemp(prefix=f"{TOOL}-{run_name}-"))
    report = Report(out)
    beamer.say(f"writing {out}")

    target = beamer.Station(beamer.find_station(station))
    report.line(f"=== {TOOL} against {target.ip} ===")
    with beamer.Step("reading the station") as step:
        step.done(target.describe())
    status = target.status() or {}
    heap = target.heap() or {}
    report.line(f"firmware: {status.get('firmware_version', 'unknown')}  "
                f"station {status.get('station_name', '?')}  ssid {status.get('ssid', '?')}")
    report.line(f"baseline heap: {heap}")
    report.meta("run_name", run_name, note=",".join(chosen))
    report.meta("started", time.strftime("%Y-%m-%dT%H:%M:%S"))
    report.meta("station_ip", target.ip)
    report.meta("firmware_version", status.get("firmware_version"))
    report.meta("station_name", status.get("station_name"))
    report.meta("ssid", status.get("ssid"))
    report.meta("baseline_free", heap.get("free"), "B")
    report.meta("baseline_largest_block", heap.get("largest_block"), "B")
    report.meta("baseline_dma_largest", heap.get("dma_largest"), "B")
    target.note_index()

    runners = {
        "writes": lambda: phase_writes(target, report, rounds),
        "churn": lambda: phase_churn(target, report),
        "stalls": lambda: phase_stalls(target, report, gate, ceiling),
        "mdns": lambda: phase_mdns(target, report),
        "soak": lambda: phase_soak(target, report, scratch),
    }
    try:
        for name in chosen:
            runners[name]()
            if name != chosen[-1]:
                wait_alive(target, report, name)
    except KeyboardInterrupt:
        report.event("interrupted", label="interrupted")
    finally:
        shutil.rmtree(scratch, ignore_errors=True)
    beamer.say(f"results: {out}")


if __name__ == "__main__":
    main()
