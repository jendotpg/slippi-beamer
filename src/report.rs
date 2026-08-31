use core::fmt::Write as _;

use crate::slp::escape_json_into;

pub const ARCH: &str = "esp32";
pub const SCHEMA: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Health {
    Ok,
    Starting,
    Warn,
    Error,
}

impl Health {
    pub fn as_str(self) -> &'static str {
        match self {
            Health::Ok => "ok",
            Health::Starting => "starting",
            Health::Warn => "warn",
            Health::Error => "error",
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Fast {
    pub replay_count: u32,
    pub game: Option<String>,
    pub port_change_at: Option<u64>,
    pub character_change_at: Option<u64>,
}

#[allow(clippy::too_many_arguments)]
pub fn status_json(
    station_id: &str,
    station_name: &str,
    ssid: Option<&str>,
    fast: &Fast,
    replay_cap: u32,
    now_s: u64,
    has_errors: bool,
    warnings: &[&str],
    net: Health,
) -> String {
    let health = match (has_errors, net) {
        (true, _) | (_, Health::Error) => Health::Error,
        (_, Health::Starting) => Health::Starting,
        _ if !warnings.is_empty() => Health::Warn,
        (_, v) => v,
    };

    let mut s = String::with_capacity(512);
    s.push_str("{\n");
    let _ = writeln!(s, "  \"schema\": {SCHEMA},");
    let _ = writeln!(s, "  \"arch\": \"{ARCH}\",");

    line_str(&mut s, "station_id", Some(station_id));
    line_str(&mut s, "station_name", Some(station_name));
    line_str(&mut s, "ssid", ssid);

    let _ = writeln!(s, "  \"replay_count\": {},", fast.replay_count);
    let _ = writeln!(s, "  \"replay_cap\": {replay_cap},");
    s.push_str("  \"ssh\": false,\n"); // always false - esp32 beamers have no ssh!

    match &fast.game {
        Some(g) => {
            let _ = writeln!(s, "  \"game\": {g},");
        }
        None => s.push_str("  \"game\": null,\n"),
    }
    line_secs_since(&mut s, "secs_since_port_change", fast.port_change_at, now_s);
    line_secs_since(
        &mut s,
        "secs_since_character_change",
        fast.character_change_at,
        now_s,
    );
    line_str(&mut s, "health", Some(health.as_str()));

    s.push_str("  \"warnings\": ");
    json_array(&mut s, warnings);
    s.push('\n');
    s.push('}');
    s.push('\n');
    s
}

pub fn index_json(station_id: &str, files: &[(String, u64)]) -> Vec<u8> {
    let mut s = String::with_capacity(256 + files.len() * 128);
    s.push_str("{\n");
    let _ = writeln!(s, "  \"schema\": {SCHEMA},");
    line_str(&mut s, "station_id", Some(station_id));
    let _ = writeln!(s, "  \"served_replay_count\": {},", files.len());
    s.push_str("  \"files\": [");
    for (i, (name, size)) in files.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        s.push_str("\n    {\"name\": \"");
        escape_json_into(name, &mut s);
        let _ = write!(s, "\", \"size\": {size}, \"url\": \"/SLIPPI/");
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

fn line_secs_since(out: &mut String, key: &str, at: Option<u64>, now_s: u64) {
    match at {
        Some(at) => {
            let _ = writeln!(out, "  \"{key}\": {},", now_s.saturating_sub(at));
        }
        None => {
            let _ = writeln!(out, "  \"{key}\": null,");
        }
    }
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

fn json_array<S: AsRef<str>>(out: &mut String, items: &[S]) {
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
        escape_json_into(item.as_ref(), out);
        out.push('"');
    }
    out.push_str("\n  ]");
}
