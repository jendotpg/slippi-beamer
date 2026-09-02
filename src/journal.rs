//! The journal: divided into the **MSC timing ring**  and the **console log**
//! Both rotate on boot and come back as sections of `LOGS/debug_N.txt` on the
//! next boot. beamer_msc.c (in the hot path) writes into the ring, but does
//! no formatting or any compute work - this is where the expensive part is
//! done on a low priority.

use std::time::{Duration, Instant};

use esp_idf_svc::hal::cpu::Core;
use esp_idf_svc::hal::task::thread::ThreadSpawnConfiguration;
use esp_idf_svc::nvs::{EspNvs, EspNvsPartition, NvsCustom};
use esp_idf_svc::sys::{beamer_msc_drain, beamer_msc_dropped, beamer_msc_sample_t};

use crate::storage::msc;

const NAMESPACE: &str = "beamer";
const PARTITION: &str = "jrnl";
const KEY: &str = "msc_stats";
const KEY_PREV: &str = "msc_prev";
const KEY_PHASE: &str = "phase";
const KEY_PHASE_PREV: &str = "phase_prev";
const FORMAT: u8 = 1;
const BUCKETS: usize = 16;
const LBA_BUCKETS: usize = 32;
const WORST: usize = 16;
const TAIL: usize = 16;
const DRAIN_CHUNK: usize = 64;
const HEAP_SAMPLE: Duration = Duration::from_millis(200);
const PERSIST: Duration = Duration::from_secs(10);
const HEARTBEAT: Duration = Duration::from_secs(60);

#[derive(Clone, Copy, Default)]
struct Outlier {
    dur_us: u32,
    lba: u32,
    op: u8,
}

#[derive(Clone)]
struct Summary {
    reads: u32,
    writes: u32,
    errors: u32,
    dropped: u32,
    uptime_s: u32,
    mounts: u32,
    umounts: u32,
    reads_ok: u32,
    writes_ok: u32,
    first_err: i32,
    maxlun: u32,
    last_cbw_s: u32,
    persist_fails: u32,
    eject_seen: bool,
    host_owns: bool,
    unsup: Vec<(u8, u32)>,
    census: Vec<(u8, u32)>,
    flushes: u32,
    cache_high_water: u32,
    cache_stalls: u32,
    heap_free: u32,
    heap_largest: u32,
    heap_largest_min: u32,
    read_hist: [u32; BUCKETS],
    write_hist: [u32; BUCKETS],
    flush_hist: [u32; BUCKETS],
    lba_read_hist: [u32; LBA_BUCKETS],
    lba_write_hist: [u32; LBA_BUCKETS],
    tail: [u32; TAIL],
    tail_n: u32,
    worst: [Outlier; WORST],
}

impl Default for Summary {
    fn default() -> Self {
        Summary {
            reads: 0,
            writes: 0,
            errors: 0,
            dropped: 0,
            uptime_s: 0,
            mounts: 0,
            umounts: 0,
            reads_ok: 0,
            writes_ok: 0,
            first_err: 0,
            maxlun: 0,
            last_cbw_s: 0,

            persist_fails: 0,
            eject_seen: false,
            host_owns: false,
            unsup: Vec::new(),
            census: Vec::new(),
            flushes: 0,
            cache_high_water: 0,
            cache_stalls: 0,
            heap_free: 0,
            heap_largest: 0,
            heap_largest_min: 0,
            read_hist: [0; BUCKETS],
            write_hist: [0; BUCKETS],
            flush_hist: [0; BUCKETS],
            lba_read_hist: [0; LBA_BUCKETS],
            lba_write_hist: [0; LBA_BUCKETS],
            tail: [0; TAIL],
            tail_n: 0,
            worst: [Outlier::default(); WORST],
        }
    }
}

fn bucket_of(v: u32, buckets: usize) -> usize {
    if v == 0 {
        0
    } else {
        ((32 - v.leading_zeros()) as usize).min(buckets - 1)
    }
}

fn bucket(dur_us: u32) -> usize {
    bucket_of(dur_us, BUCKETS)
}

impl Summary {
    fn record(&mut self, s: &beamer_msc_sample_t) {
        let lba_b = bucket_of(s.lba, LBA_BUCKETS);
        match s.op as u32 {
            // A cache flush is the card's real write latency
            2 => {
                self.flushes += 1;
                self.flush_hist[bucket(s.dur_us)] += 1;
                self.lba_write_hist[lba_b] += 1;
                self.tail[(self.tail_n as usize) % TAIL] = s.lba;
                self.tail_n = self.tail_n.wrapping_add(1);
            }
            1 => {
                self.writes += 1;
                self.write_hist[bucket(s.dur_us)] += 1;
            }
            _ => {
                self.reads += 1;
                self.read_hist[bucket(s.dur_us)] += 1;
                self.lba_read_hist[lba_b] += 1;
            }
        }
        if s.err != 0 {
            self.errors += 1;
        }

        if s.dur_us > self.worst[WORST - 1].dur_us {
            self.worst[WORST - 1] = Outlier {
                dur_us: s.dur_us,
                lba: s.lba,
                op: s.op,
            };
            let mut i = WORST - 1;
            while i > 0 && self.worst[i].dur_us > self.worst[i - 1].dur_us {
                self.worst.swap(i, i - 1);
                i -= 1;
            }
        }
    }

    fn total(&self) -> u32 {
        self.reads + self.writes
    }

    const WORDS: usize = 20 + 3 * BUCKETS + 2 * LBA_BUCKETS + TAIL + 1;
    const UNSUP: usize = 8;
    const CENSUS: usize = 16; // must match BEAMER_MSC_CENSUS

    fn encode(&self) -> Vec<u8> {
        let mut v = Vec::with_capacity(1 + 4 * Self::WORDS + 9 * WORST);
        v.push(FORMAT);
        for n in [
            self.reads,
            self.writes,
            self.errors,
            self.dropped,
            self.uptime_s,
            self.mounts,
            self.umounts,
            self.reads_ok,
            self.writes_ok,
            self.first_err as u32,
            self.maxlun,
            self.last_cbw_s,
            self.persist_fails,
            u32::from(self.eject_seen) | (u32::from(self.host_owns) << 1),
            self.flushes,
            self.cache_high_water,
            self.cache_stalls,
            self.heap_free,
            self.heap_largest,
            self.heap_largest_min,
        ] {
            v.extend_from_slice(&n.to_le_bytes());
        }
        for h in self
            .read_hist
            .iter()
            .chain(self.write_hist.iter())
            .chain(self.flush_hist.iter())
            .chain(self.lba_read_hist.iter())
            .chain(self.lba_write_hist.iter())
            .chain(self.tail.iter())
        {
            v.extend_from_slice(&h.to_le_bytes());
        }
        v.extend_from_slice(&self.tail_n.to_le_bytes());
        for o in &self.worst {
            v.extend_from_slice(&o.dur_us.to_le_bytes());
            v.extend_from_slice(&o.lba.to_le_bytes());
            v.push(o.op);
        }
        for i in 0..Self::UNSUP {
            let (op, count) = self.unsup.get(i).copied().unwrap_or((0, 0));
            v.extend_from_slice(&count.to_le_bytes());
            v.push(op);
        }
        for i in 0..Self::CENSUS {
            let (op, count) = self.census.get(i).copied().unwrap_or((0, 0));
            v.extend_from_slice(&count.to_le_bytes());
            v.push(op);
        }
        v
    }

    fn decode(b: &[u8]) -> Option<Summary> {
        if b.first().copied()? != FORMAT {
            return None;
        }
        let body = b.get(1..)?;

        let mut s = Summary::default();
        let mut c = body.chunks_exact(4);
        let mut next = || -> Option<u32> {
            c.next()
                .map(|w| u32::from_le_bytes([w[0], w[1], w[2], w[3]]))
        };
        s.reads = next()?;
        s.writes = next()?;
        s.errors = next()?;
        s.dropped = next()?;
        s.uptime_s = next()?;
        s.mounts = next()?;
        s.umounts = next()?;
        s.reads_ok = next()?;
        s.writes_ok = next()?;
        s.first_err = next()? as i32;
        s.maxlun = next()?;
        s.last_cbw_s = next()?;
        s.persist_fails = next()?;
        let flags = next()?;
        s.eject_seen = flags & 1 != 0;
        s.host_owns = flags & 2 != 0;
        s.flushes = next()?;
        s.cache_high_water = next()?;
        s.cache_stalls = next()?;
        s.heap_free = next()?;
        s.heap_largest = next()?;
        s.heap_largest_min = next()?;
        for i in 0..BUCKETS {
            s.read_hist[i] = next()?;
        }
        for i in 0..BUCKETS {
            s.write_hist[i] = next()?;
        }
        for i in 0..BUCKETS {
            s.flush_hist[i] = next()?;
        }
        for i in 0..LBA_BUCKETS {
            s.lba_read_hist[i] = next()?;
        }
        for i in 0..LBA_BUCKETS {
            s.lba_write_hist[i] = next()?;
        }
        for i in 0..TAIL {
            s.tail[i] = next()?;
        }
        s.tail_n = next()?;

        let worst_at = 4 * Self::WORDS;
        let worst = body.get(worst_at..)?;
        for (i, o) in worst.chunks_exact(9).take(WORST).enumerate() {
            s.worst[i] = Outlier {
                dur_us: u32::from_le_bytes([o[0], o[1], o[2], o[3]]),
                lba: u32::from_le_bytes([o[4], o[5], o[6], o[7]]),
                op: o[8],
            };
        }

        let unsup_at = worst_at + 9 * WORST;
        if let Some(u) = body.get(unsup_at..) {
            for e in u.chunks_exact(5).take(Self::UNSUP) {
                let count = u32::from_le_bytes([e[0], e[1], e[2], e[3]]);
                if count > 0 {
                    s.unsup.push((e[4], count));
                }
            }
        }
        let census_at = unsup_at + 5 * Self::UNSUP;
        if let Some(c) = body.get(census_at..) {
            for e in c.chunks_exact(5).take(Self::CENSUS) {
                let count = u32::from_le_bytes([e[0], e[1], e[2], e[3]]);
                if count > 0 {
                    s.census.push((e[4], count));
                }
            }
        }
        Some(s)
    }

    fn tail_lbas(&self) -> Vec<u32> {
        let n = (self.tail_n as usize).min(TAIL);
        let start = if self.tail_n as usize <= TAIL {
            0
        } else {
            self.tail_n as usize % TAIL
        };
        (0..n).map(|i| self.tail[(start + i) % TAIL]).collect()
    }

    fn lines(&self) -> Vec<String> {
        use std::fmt::Write as _;

        let mut out = Vec::new();

        out.push(format!(
            "host: {} mount(s), {} unmount(s), medium {} at the end",
            self.mounts,
            self.umounts,
            if self.host_owns { "held" } else { "released" },
        ));
        out.push(format!(
            "host: read {} sector(s), wrote {} sector(s), first error 0x{:x}",
            self.reads_ok, self.writes_ok, self.first_err,
        ));
        let silence = self.uptime_s.saturating_sub(self.last_cbw_s);
        if self.last_cbw_s == 0 {
            out.push("host: never asked for a transfer".into());
        } else if silence >= 30 {
            out.push(format!(
                "host: WENT QUIET at {}s and asked for nothing for {}s after",
                self.last_cbw_s, silence
            ));
        } else {
            out.push(format!("host: last asked at {}s", self.last_cbw_s));
        }
        out.push(format!("host: {} GET MAX LUN", self.maxlun));
        for (op, n) in &self.census {
            out.push(format!("scsi: 0x{op:02x} {} x{n}", msc::opcode_name(*op)));
        }
        out.push(format!(
            "host: eject {}",
            if self.eject_seen {
                "seen"
            } else {
                "never seen -- the cable came out"
            },
        ));
        if self.unsup.is_empty() {
            out.push("scsi: no unsupported commands".into());
        } else {
            for (op, n) in &self.unsup {
                out.push(format!("scsi: REFUSED opcode 0x{op:02x} x{n}"));
            }
        }

        if self.total() == 0 {
            out.push("timing: nothing recorded".into());
            return out;
        }

        out.push(format!(
            "{} reads, {} writes, {} errors, {} samples dropped, {}s in",
            self.reads, self.writes, self.errors, self.dropped, self.uptime_s
        ));
        if self.persist_fails > 0 {
            out.push(format!(
                "journal: {} failed write(s) -- the record above is incomplete",
                self.persist_fails
            ));
        }

        out.push(format!(
            "cache: {} flush(es) for {} host write(s), high water {}/{} sector(s), {} stall(s)",
            self.flushes,
            self.writes,
            self.cache_high_water,
            unsafe { esp_idf_svc::sys::beamer_wbc_capacity() },
            self.cache_stalls,
        ));

        let mut heap = format!(
            "heap: {} B free, largest block {} B",
            self.heap_free, self.heap_largest
        );
        if self.heap_largest_min > 0 {
            let _ = write!(heap, ", low water {} B", self.heap_largest_min);
        }
        out.push(heap);
        if self.cache_stalls > 0 {
            out.push("cache: SATURATED -- host writes blocked waiting for the card".into());
        }

        for (name, hist) in [
            ("read", &self.read_hist),
            ("write", &self.write_hist),
            ("flush", &self.flush_hist),
        ] {
            let mut line = String::new();
            for (i, n) in hist.iter().enumerate() {
                if *n > 0 {
                    let _ = write!(line, " <{}us:{}", 1u32 << i, n);
                }
            }
            if !line.is_empty() {
                out.push(format!("{name} time:{line}"));
            }
        }

        for (name, hist) in [
            ("read", &self.lba_read_hist),
            ("write", &self.lba_write_hist),
        ] {
            let mut line = String::new();
            for (i, n) in hist.iter().enumerate() {
                if *n > 0 {
                    let _ = write!(line, " <lba{}:{}", 1u64 << i, n);
                }
            }
            if !line.is_empty() {
                out.push(format!("{name} where:{line}"));
            }
        }

        let tail = self.tail_lbas();
        if !tail.is_empty() {
            let l: Vec<String> = tail.iter().map(|v| v.to_string()).collect();
            out.push(format!("last writes: {}", l.join(" ")));
        }

        for o in self.worst.iter().filter(|o| o.dur_us > 0) {
            out.push(format!(
                "worst: {}us {} lba {}",
                o.dur_us,
                match o.op {
                    2 => "flush",
                    1 => "write",
                    _ => "read",
                },
                o.lba
            ));
        }
        out
    }

    fn report(&self, when: &str) {
        log::info!("msc timing ({when}):");
        for l in self.lines() {
            log::info!("  {l}");
        }
    }
}

fn partition() -> Option<EspNvsPartition<NvsCustom>> {
    static PART: std::sync::OnceLock<Option<EspNvsPartition<NvsCustom>>> =
        std::sync::OnceLock::new();
    PART.get_or_init(|| match EspNvsPartition::<NvsCustom>::take(PARTITION) {
        Ok(p) => Some(p),
        Err(e) => {
            log::warn!("journal partition '{PARTITION}' unavailable: {e}");
            None
        }
    })
    .clone()
}

fn open() -> Option<EspNvs<NvsCustom>> {
    match EspNvs::new(partition()?, NAMESPACE, true) {
        Ok(nvs) => Some(nvs),
        Err(e) => {
            log::warn!("NVS unavailable, timing will not survive this boot: {e}");
            None
        }
    }
}

pub fn report_previous() {
    let Some(nvs) = open() else { return };

    rotate_log(&nvs);

    let mut buf = [0u8; 1024];
    let rotated = match nvs.get_blob(KEY, &mut buf) {
        Ok(Some(bytes)) => {
            let owned = bytes.to_vec();
            if let Err(e) = nvs.set_blob(KEY_PREV, &owned) {
                log::warn!("msc timing: rotate failed: {e}");
            }
            if let Err(e) = nvs.remove(KEY) {
                log::warn!("msc timing: could not clear the rotated key: {e}");
            }
            Some(owned)
        }
        Ok(None) => None,
        Err(e) => {
            log::warn!("msc timing: read failed: {e}");
            None
        }
    };

    let mut pbuf = [0u8; 8];
    let crumb = nvs
        .get_blob(KEY_PHASE, &mut pbuf)
        .ok()
        .flatten()
        .map(<[u8]>::to_vec)
        .unwrap_or_default();
    if let Err(e) = nvs.set_blob(KEY_PHASE_PREV, &crumb) {
        log::warn!("phase: rotate failed: {e}");
    }
    if let Some(&phase) = crumb.first() {
        log::info!("previous boot reached phase: {}", Phase::from_u8(phase));
    }

    *handle().lock().unwrap() = Some(nvs);

    match rotated.as_deref().and_then(Summary::decode) {
        Some(s) => {
            s.report("previous boot");
            *prev().lock().unwrap() = Some(s);
        }
        None if rotated.is_some() => log::warn!("msc timing: stored summary was malformed"),
        None => log::info!("msc timing: nothing stored from a previous boot"),
    }
}

fn handle() -> &'static std::sync::Mutex<Option<EspNvs<NvsCustom>>> {
    static NVS: std::sync::OnceLock<std::sync::Mutex<Option<EspNvs<NvsCustom>>>> =
        std::sync::OnceLock::new();
    NVS.get_or_init(|| std::sync::Mutex::new(None))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum Phase {
    Unrecorded = 0,
    ClaimIdentity = 1,
    MountCard = 2,
    PrepareForBind = 3,
    BindCard = 4,
    StartJournal = 5,
    EstablishNetworkServices = 6,
    Running = 7,
}

impl Phase {
    fn from_u8(v: u8) -> Phase {
        match v {
            1 => Phase::ClaimIdentity,
            2 => Phase::MountCard,
            3 => Phase::PrepareForBind,
            4 => Phase::BindCard,
            5 => Phase::StartJournal,
            6 => Phase::EstablishNetworkServices,
            7 => Phase::Running,
            _ => Phase::Unrecorded,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Phase::Unrecorded => "Unrecorded",
            Phase::ClaimIdentity => "ClaimIdentity",
            Phase::MountCard => "MountCard",
            Phase::PrepareForBind => "PrepareForBind",
            Phase::BindCard => "BindCard",
            Phase::StartJournal => "StartJournal",
            Phase::EstablishNetworkServices => "EstablishNetworkServices",
            Phase::Running => "Running",
        }
    }
}

impl core::fmt::Display for Phase {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.as_str())
    }
}

static STAMP: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
static BOOTS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

pub fn stamp(reset: u32, boots: u32) {
    use std::sync::atomic::Ordering::Relaxed;
    STAMP.store(reset, Relaxed);
    BOOTS.store(boots, Relaxed);
}

const KEY_RESETS: &str = "resets";
const CENSUS_FORMAT: u8 = 1;
const CENSUS_SLOTS: usize = 16;

fn census_text() -> &'static std::sync::Mutex<Vec<String>> {
    static TEXT: std::sync::OnceLock<std::sync::Mutex<Vec<String>>> = std::sync::OnceLock::new();
    TEXT.get_or_init(|| std::sync::Mutex::new(Vec::new()))
}

pub fn count_reset(reset: u32, boots: u32) {
    if !enabled() {
        return;
    }
    let guard = handle().lock().unwrap();
    let Some(nvs) = guard.as_ref() else { return };

    let mut counts = [0u16; CENSUS_SLOTS];
    let mut early = 0u16;
    let mut late = 0u16;

    let mut buf = [0u8; 64];
    match nvs.get_blob(KEY_RESETS, &mut buf) {
        Ok(Some(b)) if b.len() == 1 + CENSUS_SLOTS * 2 + 4 && b[0] == CENSUS_FORMAT => {
            for (i, slot) in counts.iter_mut().enumerate() {
                *slot = u16::from_le_bytes([b[1 + i * 2], b[2 + i * 2]]);
            }
            let tail = 1 + CENSUS_SLOTS * 2;
            early = u16::from_le_bytes([b[tail], b[tail + 1]]);
            late = u16::from_le_bytes([b[tail + 2], b[tail + 3]]);
        }
        Ok(_) => {}
        Err(e) => {
            log::warn!("reset census: read failed: {e}");
            return;
        }
    }

    let idx = if (reset as usize) < CENSUS_SLOTS {
        reset as usize
    } else {
        0
    };
    counts[idx] = counts[idx].saturating_add(1);
    if reset == esp_idf_svc::sys::esp_reset_reason_t_ESP_RST_BROWNOUT {
        if boots > 1 {
            late = late.saturating_add(1);
        } else {
            early = early.saturating_add(1);
        }
    }

    let mut blob = [0u8; 1 + CENSUS_SLOTS * 2 + 4];
    blob[0] = CENSUS_FORMAT;
    for (i, slot) in counts.iter().enumerate() {
        blob[1 + i * 2..3 + i * 2].copy_from_slice(&slot.to_le_bytes());
    }
    let tail = 1 + CENSUS_SLOTS * 2;
    blob[tail..tail + 2].copy_from_slice(&early.to_le_bytes());
    blob[tail + 2..tail + 4].copy_from_slice(&late.to_le_bytes());
    if let Err(e) = nvs.set_blob(KEY_RESETS, &blob) {
        log::warn!("reset census: not recorded: {e}");
    }

    let total: u32 = counts.iter().map(|&n| u32::from(n)).sum();
    let parts: Vec<String> = counts
        .iter()
        .enumerate()
        .filter(|(_, &n)| n > 0)
        .map(|(i, &n)| format!("{n} {}", reset_name(i as u32)))
        .collect();

    let mut out = vec![format!("{total} boots: {}", parts.join(", "))];
    if early > 0 || late > 0 {
        out.push(format!(
            "of those BROWNOUTs, {early} as power came up and {late} with the station already running"
        ));
    }
    for l in &out {
        log::info!("resets: {l}");
    }
    *census_text().lock().unwrap() = out;
}

pub fn census_lines() -> Vec<String> {
    census_text().lock().unwrap().clone()
}

pub fn heap_note() -> String {
    let (free, largest) = heap_now();
    format!("heap: {free} B free, largest block {largest} B")
}

pub fn heap_now() -> (u32, u32) {
    use esp_idf_svc::sys::{
        heap_caps_get_largest_free_block, MALLOC_CAP_8BIT, MALLOC_CAP_INTERNAL,
    };
    let free = unsafe { esp_idf_svc::sys::esp_get_free_heap_size() };
    let largest =
        unsafe { heap_caps_get_largest_free_block(MALLOC_CAP_INTERNAL | MALLOC_CAP_8BIT) };
    (free, largest as u32)
}

#[allow(non_upper_case_globals)]
pub fn reset_name(raw: u32) -> &'static str {
    use esp_idf_svc::sys::*;
    match raw {
        esp_reset_reason_t_ESP_RST_POWERON => "power-on",
        esp_reset_reason_t_ESP_RST_EXT => "external pin",
        esp_reset_reason_t_ESP_RST_SW => "software",
        esp_reset_reason_t_ESP_RST_PANIC => "PANIC",
        esp_reset_reason_t_ESP_RST_INT_WDT => "interrupt watchdog",
        esp_reset_reason_t_ESP_RST_TASK_WDT => "task watchdog",
        esp_reset_reason_t_ESP_RST_WDT => "other watchdog",
        esp_reset_reason_t_ESP_RST_DEEPSLEEP => "deep sleep",
        esp_reset_reason_t_ESP_RST_BROWNOUT => "BROWNOUT",
        esp_reset_reason_t_ESP_RST_SDIO => "SDIO",
        esp_reset_reason_t_ESP_RST_USB => "USB peripheral",
        esp_reset_reason_t_ESP_RST_JTAG => "JTAG",
        esp_reset_reason_t_ESP_RST_EFUSE => "eFuse error",
        esp_reset_reason_t_ESP_RST_PWR_GLITCH => "power glitch",
        esp_reset_reason_t_ESP_RST_CPU_LOCKUP => "CPU lockup",
        _ => "unknown",
    }
}

static HIGHEST: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

pub fn mark(phase: Phase) {
    use std::sync::atomic::Ordering::Relaxed;
    if !enabled() {
        return;
    }
    if HIGHEST.fetch_max(phase as u8, Relaxed) >= phase as u8 {
        return;
    }
    let mut blob = [0u8; 6];
    blob[0] = phase as u8;
    blob[1] = STAMP.load(Relaxed) as u8;
    blob[2..].copy_from_slice(&BOOTS.load(Relaxed).to_le_bytes());
    if let Some(nvs) = handle().lock().unwrap().as_ref() {
        if let Err(e) = nvs.set_blob(KEY_PHASE, &blob) {
            log::warn!("phase {phase} not recorded: {e}");
        }
    }
}

pub fn previous_progress() -> Option<(Phase, u32, u32)> {
    let guard = handle().lock().unwrap();
    let nvs = guard.as_ref()?;
    let mut buf = [0u8; 8];
    let b = nvs.get_blob(KEY_PHASE_PREV, &mut buf).ok().flatten()?;
    if b.len() < 6 {
        return None;
    }
    Some((
        Phase::from_u8(b[0]),
        u32::from(b[1]),
        u32::from_le_bytes([b[2], b[3], b[4], b[5]]),
    ))
}

fn prev() -> &'static std::sync::Mutex<Option<Summary>> {
    static PREV: std::sync::OnceLock<std::sync::Mutex<Option<Summary>>> =
        std::sync::OnceLock::new();
    PREV.get_or_init(|| std::sync::Mutex::new(None))
}

pub fn previous_lines() -> Vec<String> {
    match prev().lock().unwrap().as_ref() {
        Some(s) => s.lines(),
        None => Vec::new(),
    }
}

static ENABLED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

pub fn enabled() -> bool {
    ENABLED.load(std::sync::atomic::Ordering::Relaxed)
}

pub fn disable() {
    ENABLED.store(false, std::sync::atomic::Ordering::Relaxed);

    {
        let mut tail = log_tail().lock().unwrap();
        tail.text.clear();
        tail.dirty = false;
    }
    prev_log().lock().unwrap().clear();
    *prev().lock().unwrap() = None;
    census_text().lock().unwrap().clear();

    let guard = handle().lock().unwrap();
    let Some(nvs) = guard.as_ref() else { return };
    for key in [
        KEY,
        KEY_PREV,
        KEY_PHASE,
        KEY_PHASE_PREV,
        KEY_LOG,
        KEY_LOG_PREV,
        KEY_RESETS,
    ] {
        if let Err(e) = nvs.remove(key) {
            log::warn!("journal: could not clear '{key}': {e}");
        }
    }
    log::info!("journal off: DEBUG is not set, nothing will be recorded");
}

const KEY_LOG: &str = "log";
const KEY_LOG_PREV: &str = "log_prev";
const LOG_CAP: usize = 4096;

struct LogTail {
    text: String,
    dirty: bool,
    persist_fails: u32,
}

fn log_tail() -> &'static std::sync::Mutex<LogTail> {
    static TAIL: std::sync::OnceLock<std::sync::Mutex<LogTail>> = std::sync::OnceLock::new();
    TAIL.get_or_init(|| {
        std::sync::Mutex::new(LogTail {
            text: String::with_capacity(LOG_CAP + 512),
            dirty: false,
            persist_fails: 0,
        })
    })
}

fn prev_log() -> &'static std::sync::Mutex<String> {
    static PREV: std::sync::OnceLock<std::sync::Mutex<String>> = std::sync::OnceLock::new();
    PREV.get_or_init(|| std::sync::Mutex::new(String::new()))
}

pub fn previous_log_lines() -> Vec<String> {
    prev_log()
        .lock()
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect()
}

fn rotate_log(nvs: &EspNvs<NvsCustom>) {
    let mut buf = vec![0u8; LOG_CAP + 1];
    let carried = match nvs.get_blob(KEY_LOG, &mut buf) {
        Ok(Some(bytes)) => String::from_utf8_lossy(bytes).into_owned(),
        _ => String::new(),
    };

    if carried.is_empty() {
        let _ = nvs.remove(KEY_LOG_PREV);
    } else if let Err(e) = nvs.set_blob(KEY_LOG_PREV, carried.as_bytes()) {
        log::warn!("log: rotate failed: {e}");
    }
    let _ = nvs.remove(KEY_LOG);

    *prev_log().lock().unwrap() = carried;
}

fn append_tail(tail: &mut LogTail, chunk: &str) {
    if !enabled() {
        return;
    }
    tail.text.push_str(chunk);
    tail.dirty = true;

    if tail.text.len() <= LOG_CAP {
        return;
    }
    let excess = tail.text.len() - LOG_CAP;
    let cut = match tail.text[excess..].find('\n') {
        Some(nl) => excess + nl + 1,
        None => {
            let mut c = excess;
            while c < tail.text.len() && !tail.text.is_char_boundary(c) {
                c += 1;
            }
            c
        }
    };
    tail.text.drain(..cut);
}

fn persist_log(nvs: Option<&mut EspNvs<NvsCustom>>) {
    if !enabled() {
        return;
    }
    let Some(nvs) = nvs else { return };
    let mut tail = log_tail().lock().unwrap();
    if !tail.dirty {
        return;
    }
    match nvs.set_blob(KEY_LOG, tail.text.as_bytes()) {
        Ok(()) => tail.dirty = false,
        Err(e) => {
            tail.persist_fails += 1;
            let _ = e;
        }
    }
}

pub fn persist_now() {
    if !enabled() {
        return;
    }
    PERSIST_NOW.store(true, std::sync::atomic::Ordering::Relaxed);
    let mut nvs = open();
    persist_log(nvs.as_mut());
}

static PERSIST_NOW: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
struct Logger;

struct LineBuf {
    buf: [u8; 192],
    len: usize,
}

impl core::fmt::Write for LineBuf {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let room = self.buf.len() - self.len;
        let n = s.len().min(room);
        self.buf[self.len..self.len + n].copy_from_slice(&s.as_bytes()[..n]);
        self.len += n;
        Ok(())
    }
}

impl log::Log for Logger {
    fn enabled(&self, _: &log::Metadata) -> bool {
        true
    }

    fn log(&self, record: &log::Record) {
        use core::fmt::Write as _;

        let marker = match record.level() {
            log::Level::Error => 'E',
            log::Level::Warn => 'W',
            log::Level::Info => 'I',
            log::Level::Debug => 'D',
            log::Level::Trace => 'V',
        };
        let ms = unsafe { esp_idf_svc::sys::esp_log_timestamp() };

        let mut line = LineBuf {
            buf: [0; 192],
            len: 0,
        };
        let _ = writeln!(
            line,
            "{marker} ({ms}) {}: {}",
            record.target(),
            record.args()
        );

        unsafe {
            esp_idf_svc::sys::beamer_log_push(
                line.buf.as_ptr() as *const core::ffi::c_char,
                line.len,
            )
        };
    }

    fn flush(&self) {}
}

static LOGGER: Logger = Logger;

pub fn spawn_log() -> anyhow::Result<()> {
    unsafe { esp_idf_svc::sys::beamer_log_install() };
    let _ = log::set_logger(&LOGGER);
    log::set_max_level(log::LevelFilter::Info);

    const STACK: usize = 4096;
    ThreadSpawnConfiguration {
        name: Some(c"jrnl-log"),
        stack_size: STACK,
        priority: 1,
        pin_to_core: Some(Core::Core1),
        ..Default::default()
    }
    .set()?;

    std::thread::Builder::new()
        .stack_size(STACK)
        .spawn(move || {
            use std::io::Write as _;

            let mut nvs = open();
            let mut buf = [0u8; 512];
            let mut last_persist = Instant::now();
            let mut last_dropped = 0u32;

            loop {
                let n = unsafe {
                    esp_idf_svc::sys::beamer_log_drain(
                        buf.as_mut_ptr() as *mut core::ffi::c_char,
                        buf.len(),
                    )
                };

                if n > 0 {
                    let chunk = String::from_utf8_lossy(&buf[..n]);
                    let _ = std::io::stdout().write_all(chunk.as_bytes());
                    append_tail(&mut log_tail().lock().unwrap(), &chunk);
                }

                let dropped = unsafe { esp_idf_svc::sys::beamer_log_dropped() };
                if dropped != last_dropped {
                    let note = format!("[log: {} byte(s) dropped]\n", dropped - last_dropped);
                    last_dropped = dropped;
                    append_tail(&mut log_tail().lock().unwrap(), &note);
                }

                if n < buf.len() {
                    std::thread::sleep(Duration::from_millis(200));
                }

                if last_persist.elapsed() >= PERSIST {
                    persist_log(nvs.as_mut());
                    last_persist = Instant::now();
                }
            }
        })?;

    ThreadSpawnConfiguration::default().set()?;
    Ok(())
}

pub fn spawn() -> anyhow::Result<()> {
    const STACK: usize = 8192;

    ThreadSpawnConfiguration {
        name: Some(c"journal"),
        stack_size: STACK,
        priority: 1,
        pin_to_core: Some(Core::Core1),
        ..Default::default()
    }
    .set()?;

    std::thread::Builder::new()
        .stack_size(STACK)
        .spawn(move || {
            let mut nvs = open();
            let mut summary = Summary::default();
            static mut DRAIN: [beamer_msc_sample_t; DRAIN_CHUNK] = [beamer_msc_sample_t {
                start_us: 0,
                dur_us: 0,
                lba: 0,
                blocks: 0,
                op: 0,
                err: 0,
            }; DRAIN_CHUNK];
            let buf = unsafe { &mut *core::ptr::addr_of_mut!(DRAIN) };
            let started = Instant::now();

            if let Some(nvs) = nvs.as_mut() {
                if let Err(e) = nvs.set_blob(KEY, &Summary::default().encode()) {
                    log::warn!("msc timing: could not claim the key: {e}");
                }
            }

            let mut last_persist = Instant::now();
            let (_, mut heap_low) = heap_now();
            let mut last_heap = Instant::now();
            let mut dirty = false;
            let mut last_counters = (0u32, 0u32, 0u32, 0u32, 0i32, false, false);

            loop {
                let n = unsafe { beamer_msc_drain(buf.as_mut_ptr(), DRAIN_CHUNK) };
                for s in &buf[..n] {
                    summary.record(s);
                    dirty = true;
                }
                summary.dropped = unsafe { beamer_msc_dropped() };

                if n < DRAIN_CHUNK {
                    std::thread::sleep(Duration::from_millis(200));
                }

                if last_heap.elapsed() >= HEAP_SAMPLE {
                    last_heap = Instant::now();
                    let (_, largest) = heap_now();
                    heap_low = heap_low.min(largest);
                }

                let counters = (
                    msc::mounts(),
                    msc::umounts(),
                    msc::reads_ok(),
                    msc::writes_ok(),
                    msc::first_err(),
                    msc::eject_seen(),
                    msc::host_owns(),
                );
                if counters != last_counters {
                    last_counters = counters;
                    dirty = true;
                }

                let asked = PERSIST_NOW.swap(false, std::sync::atomic::Ordering::Relaxed);
                if asked
                    || (dirty && last_persist.elapsed() >= PERSIST)
                    || last_persist.elapsed() >= HEARTBEAT
                {
                    summary.uptime_s = started.elapsed().as_secs() as u32;
                    let (m, um, r, w, err, eject, owns) = counters;
                    summary.mounts = m;
                    summary.umounts = um;
                    summary.reads_ok = r;
                    summary.writes_ok = w;
                    summary.first_err = err;
                    summary.maxlun = msc::maxlun_asks();
                    summary.last_cbw_s = (msc::last_cbw_us() / 1_000_000) as u32;
                    summary.eject_seen = eject;
                    summary.host_owns = owns;
                    summary.unsup = msc::unsupported();
                    summary.census = msc::census();
                    summary.cache_high_water = msc::cache_high_water();
                    summary.cache_stalls = msc::cache_stalls();
                    let (free, largest) = heap_now();
                    summary.heap_free = free;
                    summary.heap_largest = largest;
                    heap_low = heap_low.min(largest);
                    summary.heap_largest_min = heap_low;
                    if let Some(nvs) = nvs.as_mut() {
                        if let Err(e) = nvs.set_blob(KEY, &summary.encode()) {
                            log::warn!("msc timing: persist failed: {e}");
                            summary.persist_fails += 1;
                        }
                    }
                    summary.report("this boot, so far");
                    last_persist = Instant::now();
                    dirty = false;
                }
            }
        })?;

    ThreadSpawnConfiguration::default().set()?;
    Ok(())
}
