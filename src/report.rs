use core::fmt::Write as _;
use std::net::Ipv4Addr;

use crate::slp::escape_json_into;

pub const ARCH: &str = "esp32";
pub const SCHEMA: u32 = 1;
pub const UDC: &str = "esp32s3-otg";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Pass,
    Pending,
    Fail,
}

impl Verdict {
    pub fn as_str(self) -> &'static str {
        match self {
            Verdict::Pass => "pass",
            Verdict::Pending => "pending",
            Verdict::Fail => "fail",
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Fast {
    pub bind_time_s: f32,
    pub host_state: &'static str,
    pub mtools: bool,
    pub slippi_files: u32,
    pub slippi_files_capped: bool,
    pub game: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct Health {
    pub station: String,
    pub station_name: String,
    pub host: String,
    pub uptime_s: u64,
    pub wifi: Option<String>,
    pub network: Option<String>,
    pub ip: Option<Ipv4Addr>,
    pub httpd: bool,
    pub sshd: bool, // always false - theres no ssh!
    pub mdns: bool,
}

pub fn status_json(
    now_s: u64,
    fast: &Fast,
    health: &Health,
    errors: &[String],
    has_errors: bool,
    net: Verdict,
) -> String {
    let result = if has_errors { Verdict::Fail } else { net };

    let mut s = String::with_capacity(1024);
    s.push_str("{\n");
    let _ = writeln!(s, "  \"schema\": {SCHEMA},");
    let _ = writeln!(s, "  \"beamer_arch\": \"{ARCH}\",");
    line_str(&mut s, "generated", Some(&iso_epoch(now_s)));

    let _ = writeln!(s, "  \"udc\": \"{UDC}\",");
    let _ = writeln!(s, "  \"bind_time_s\": {:.3},", fast.bind_time_s);
    line_str(&mut s, "host_state", Some(fast.host_state));
    let _ = writeln!(s, "  \"mtools\": {},", fast.mtools);
    let _ = writeln!(s, "  \"slippi_files\": {},", fast.slippi_files);
    let _ = writeln!(
        s,
        "  \"slippi_files_capped\": {},",
        fast.slippi_files_capped
    );

    line_str(&mut s, "station", Some(&health.station));
    line_str(&mut s, "station_name", Some(&health.station_name));
    line_str(&mut s, "host", Some(&health.host));
    line_str(&mut s, "boot", Some(&iso_epoch(0)));
    let _ = writeln!(s, "  \"uptime_s\": {},", health.uptime_s);
    line_str(&mut s, "wifi", health.wifi.as_deref());
    line_str(&mut s, "network", health.network.as_deref());
    match health.ip {
        Some(ip) => line_str(&mut s, "ip", Some(&ip.to_string())),
        None => line_str(&mut s, "ip", None),
    }
    let _ = writeln!(s, "  \"httpd\": {},", health.httpd);
    let _ = writeln!(s, "  \"sshd\": {},", health.sshd);
    let _ = writeln!(s, "  \"mdns\": {},", health.mdns);

    match &fast.game {
        Some(g) => {
            let _ = writeln!(s, "  \"game\": {g},");
        }
        None => s.push_str("  \"game\": null,\n"),
    }
    line_str(&mut s, "result", Some(result.as_str()));

    s.push_str("  \"errors\": ");
    json_array(&mut s, errors);
    s.push('\n');
    s.push('}');
    s.push('\n');
    s
}

pub fn index_json(station: &str, generated_s: u64, files: &[(String, u64, u64)]) -> Vec<u8> {
    let mut s = String::with_capacity(256 + files.len() * 160);
    s.push_str("{\n");
    let _ = writeln!(s, "  \"schema\": {SCHEMA},");
    line_str(&mut s, "station", Some(station));
    line_str(&mut s, "generated", Some(&iso_epoch(generated_s)));
    let _ = writeln!(s, "  \"count\": {},", files.len());
    s.push_str("  \"files\": [");
    for (i, (name, size, at)) in files.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        s.push_str("\n    {\"name\": \"");
        escape_json_into(name, &mut s);
        let _ = write!(
            s,
            "\", \"size\": {size}, \"mtime\": \"{}\", \"url\": \"/SLIPPI/",
            iso_epoch(*at)
        );
        escape_json_into(name, &mut s);
        s.push_str("\"}");
    }
    if !files.is_empty() {
        s.push('\n');
        s.push_str("  ");
    }
    s.push_str("]\n}\n");
    s.into_bytes()
}

fn line_str(out: &mut String, key: &str, value: Option<&str>) {
    let _ = write!(out, "  \"{key}\": ");
    match value {
        Some(v) => {
            out.push('"');
            escape_json_into(v, out);
            out.push('"');
        }
        None => out.push_str("null"),
    }
    out.push_str(",\n");
}

fn json_array(out: &mut String, items: &[String]) {
    if items.is_empty() {
        out.push_str("[]");
        return;
    }
    out.push('[');
    for (i, item) in items.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str("\n    \"");
        escape_json_into(item, out);
        out.push('"');
    }
    out.push_str("\n  ]");
}

pub fn iso_epoch(secs: u64) -> String {
    //this is probably wrong sometimes, but oh well - its mostly just so all dates are on a monotonically increasing timer
    let days = secs / 86_400;
    let rem = secs % 86_400;
    let (h, mi, s) = (rem / 3600, (rem % 3600) / 60, rem % 60);

    let mut y = 1970u64;
    let mut d = days;
    loop {
        let len = if is_leap(y) { 366 } else { 365 };
        if d < len {
            break;
        }
        d -= len;
        y += 1;
    }

    let months = [
        31,
        if is_leap(y) { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    let mut m = 0usize;
    while d >= months[m] {
        d -= months[m];
        m += 1;
    }

    format!("{y:04}-{:02}-{:02}T{h:02}:{mi:02}:{s:02}Z", m + 1, d + 1)
}

#[allow(clippy::manual_is_multiple_of)]
fn is_leap(y: u64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}
