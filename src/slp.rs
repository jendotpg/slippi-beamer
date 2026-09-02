//! VERY VERY simple replay parser - only to answer the following from the first
//! 1024 bytes of a replay file:
//!     a) which character, costume colour and nametag is each port playing?
//!     b) is this game live?

use core::fmt::Write as _;
pub const PEEK_BYTES: usize = 1024;

const MAGIC: &[u8] = b"{U\x03raw[$U#l";
const GS_DEEPEST: usize = 0x1A1;

const _: () = assert!(15 + 1 + 255 + GS_DEEPEST <= PEEK_BYTES);

const CHARS: [(&str, &[Option<&str>]); 26] = [
    (
        "Falcon",
        &[
            None,
            Some("black"),
            Some("red"),
            Some("white"),
            Some("green"),
            Some("blue"),
        ],
    ),
    (
        "DK",
        &[
            None,
            Some("black"),
            Some("red"),
            Some("blue"),
            Some("green"),
        ],
    ),
    ("Fox", &[None, Some("red"), Some("blue"), Some("green")]),
    ("GW", &[None, Some("red"), Some("blue"), Some("green")]),
    (
        "Kirby",
        &[
            None,
            Some("yellow"),
            Some("blue"),
            Some("red"),
            Some("green"),
            Some("white"),
        ],
    ),
    ("Bowser", &[None, Some("red"), Some("blue"), Some("black")]),
    (
        "Link",
        &[
            None,
            Some("red"),
            Some("blue"),
            Some("black"),
            Some("white"),
        ],
    ),
    ("Luigi", &[None, Some("white"), Some("blue"), Some("red")]),
    (
        "Mario",
        &[
            None,
            Some("yellow"),
            Some("black"),
            Some("blue"),
            Some("green"),
        ],
    ),
    (
        "Marth",
        &[
            None,
            Some("red"),
            Some("green"),
            Some("black"),
            Some("white"),
        ],
    ),
    ("Mewtwo", &[None, Some("red"), Some("blue"), Some("green")]),
    ("Ness", &[None, Some("gold"), Some("blue"), Some("green")]),
    (
        "Peach",
        &[
            None,
            Some("gold"),
            Some("white"),
            Some("blue"),
            Some("green"),
        ],
    ),
    ("Pikachu", &[None, Some("red"), Some("blue"), Some("green")]),
    ("ICs", &[None, Some("green"), Some("yellow"), Some("red")]),
    (
        "Puff",
        &[None, Some("red"), Some("blue"), Some("green"), Some("gold")],
    ),
    (
        "Samus",
        &[
            None,
            Some("pink"),
            Some("dark"),
            Some("green"),
            Some("blue"),
        ],
    ),
    (
        "Yoshi",
        &[
            None,
            Some("red"),
            Some("blue"),
            Some("yellow"),
            Some("pink"),
            Some("cyan"),
        ],
    ),
    (
        "Zelda",
        &[
            None,
            Some("red"),
            Some("blue"),
            Some("green"),
            Some("white"),
        ],
    ),
    (
        "Sheik",
        &[
            None,
            Some("red"),
            Some("blue"),
            Some("green"),
            Some("white"),
        ],
    ),
    ("Falco", &[None, Some("red"), Some("blue"), Some("green")]),
    (
        "YL",
        &[
            None,
            Some("red"),
            Some("blue"),
            Some("white"),
            Some("black"),
        ],
    ),
    (
        "Doc",
        &[
            None,
            Some("red"),
            Some("blue"),
            Some("green"),
            Some("black"),
        ],
    ),
    (
        "Roy",
        &[None, Some("red"), Some("blue"), Some("green"), Some("gold")],
    ),
    ("Pichu", &[None, Some("red"), Some("blue"), Some("green")]),
    (
        "Ganon",
        &[
            None,
            Some("red"),
            Some("blue"),
            Some("green"),
            Some("purple"),
        ],
    ),
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Port {
    pub port: u8,
    pub char_id: Option<u8>,
    pub char_name: Option<&'static str>,
    pub color: Option<&'static str>,
    pub costume: u8,
    pub nametag: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Game {
    pub live: bool,
    pub ports: Vec<Port>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeekError {
    NotSlp,
    NoEventPayloads,
    BadEventPayloadsSize,
    TruncatedEventPayloads,
    MissingGameStart,
    TruncatedGameStart,
}

impl PeekError {
    pub fn as_str(self) -> &'static str {
        match self {
            PeekError::NotSlp => "not an .slp file",
            PeekError::NoEventPayloads => "no event payloads command",
            PeekError::BadEventPayloadsSize => "bad event payloads size",
            PeekError::TruncatedEventPayloads => "truncated event payloads",
            PeekError::MissingGameStart => "truncated or missing game start",
            PeekError::TruncatedGameStart => "truncated game start",
        }
    }
}

impl core::fmt::Display for PeekError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::error::Error for PeekError {}

#[allow(clippy::manual_is_multiple_of)]
pub fn peek(buf: &[u8]) -> Result<Game, PeekError> {
    let n = buf.len();

    if n < 17 || !buf.starts_with(MAGIC) {
        return Err(PeekError::NotSlp);
    }

    let live = u32::from_be_bytes([buf[11], buf[12], buf[13], buf[14]]) == 0;

    if buf[15] != 0x35 {
        return Err(PeekError::NoEventPayloads);
    }

    let psz = buf[16] as usize;
    if psz < 4 || (psz - 1) % 3 != 0 {
        return Err(PeekError::BadEventPayloadsSize);
    }

    let nent = (psz - 1) / 3;
    if 17 + 3 * nent > n {
        return Err(PeekError::TruncatedEventPayloads);
    }

    let mut gs_size = 0usize;
    for i in 0..nent {
        if buf[17 + 3 * i] == 0x36 {
            gs_size = u16::from_be_bytes([buf[18 + 3 * i], buf[19 + 3 * i]]) as usize;
            break;
        }
    }

    let gs = 15 + 1 + psz;

    if gs + 0xD4 >= n || buf[gs] != 0x36 {
        return Err(PeekError::MissingGameStart);
    }

    let has_nametags = gs_size + 1 >= GS_DEEPEST;
    if has_nametags && gs + GS_DEEPEST > n {
        return Err(PeekError::TruncatedGameStart);
    }

    let mut ports = Vec::with_capacity(4);
    for i in 0..4 {
        let pb = gs + 0x65 + 0x24 * i;
        let cid = buf[pb];
        let player_type = buf[pb + 1];
        let costume = buf[pb + 3];

        if player_type != 0 && player_type != 1 {
            continue;
        }

        let entry = CHARS.get(cid as usize);
        let char_name = entry.map(|(name, _)| *name);
        let color = entry.and_then(|(_, colors)| colors.get(costume as usize).copied().flatten());

        let nametag = if has_nametags {
            decode_nametag(&buf[gs + 0x161 + 0x10 * i..][..16])
        } else {
            None
        };

        ports.push(Port {
            port: (i + 1) as u8,
            char_id: if entry.is_some() { Some(cid) } else { None },
            char_name,
            color,
            costume,
            nametag,
        });
    }

    Ok(Game { live, ports })
}

pub fn peek_reader<R: std::io::Read>(mut r: R) -> std::io::Result<Result<Game, PeekError>> {
    let mut buf = [0u8; PEEK_BYTES];
    let mut n = 0;
    while n < PEEK_BYTES {
        match r.read(&mut buf[n..]) {
            Ok(0) => break,
            Ok(got) => n += got,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        }
    }
    Ok(peek(&buf[..n]))
}

impl Game {
    pub fn port_sig(&self) -> u8 {
        let mut sig = 0u8;
        for p in &self.ports {
            sig |= 1 << (p.port - 1);
        }
        sig
    }

    pub fn character_sig(&self) -> u32 {
        let mut sig = u32::MAX;
        for p in &self.ports {
            let slot = 8 * (p.port - 1) as u32;
            let byte = p.char_id.unwrap_or(0xFE) as u32;
            sig = (sig & !(0xFF << slot)) | (byte << slot);
        }
        sig
    }

    pub fn to_json_into(&self, s: &mut crate::report::GameJson) {
        s.clear();
        let _ = write!(s, "{{\"live\":{},\"ports\":[", self.live);
        for (i, p) in self.ports.iter().enumerate() {
            if i > 0 {
                let _ = s.push(',');
            }
            let _ = write!(s, "{{\"port\":{},\"char\":", p.port);
            match p.char_name {
                Some(name) => {
                    let _ = write!(s, "\"{name}\"");
                }
                None => {
                    let _ = s.push_str("null");
                }
            }
            let _ = s.push_str(",\"char_id\":");
            match p.char_id {
                Some(id) => {
                    let _ = write!(s, "{id}");
                }
                None => {
                    let _ = s.push_str("null");
                }
            }
            let _ = s.push_str(",\"color\":");
            match p.color {
                Some(c) => {
                    let _ = write!(s, "\"{c}\"");
                }
                None => {
                    let _ = s.push_str("null");
                }
            }
            let _ = write!(s, ",\"costume\":{}", p.costume);
            let _ = s.push_str(",\"nametag\":");
            match &p.nametag {
                Some(tag) => {
                    let _ = s.push('"');
                    escape_json_into(tag, s);
                    let _ = s.push('"');
                }
                None => {
                    let _ = s.push_str("null");
                }
            }
            let _ = s.push('}');
        }
        let _ = s.push_str("]}");
    }
}

pub fn escape_json_into<W: core::fmt::Write>(s: &str, out: &mut W) {
    for c in s.chars() {
        match c {
            '"' | '\\' => {
                let _ = out.write_char('\\');
                let _ = out.write_char(c);
            }
            '\n' => {
                let _ = out.write_str("\\n");
            }
            '\r' => {
                let _ = out.write_str("\\r");
            }
            '\t' => {
                let _ = out.write_str("\\t");
            }
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => {
                let _ = out.write_char(c);
            }
        }
    }
}

// --- Shift-JIS ------------------------------------------------------------
//
// Nametags are Shift-JIS, 16 bytes, NUL-terminated, and are decoded in full.
// Only lead bytes 0x81-0x83 occur in Melee's name entry, so this covers the
// punctuation block, full-width alphanumerics, hiragana and katakana rather
// than all of JIS X 0208.
//
// Fully written by Claude...

const REPLACEMENT: char = '\u{FFFD}';

const SJIS_81: [u16; 188] = [
    0x3000, 0x3001, 0x3002, 0xFF0C, 0xFF0E, 0x30FB, 0xFF1A, 0xFF1B, 0xFF1F, 0xFF01, 0x309B, 0x309C,
    0x00B4, 0xFF40, 0x00A8, 0xFF3E, 0xFFE3, 0xFF3F, 0x30FD, 0x30FE, 0x309D, 0x309E, 0x3003, 0x4EDD,
    0x3005, 0x3006, 0x3007, 0x30FC, 0x2015, 0x2010, 0xFF0F, 0xFF3C, 0x301C, 0x2016, 0xFF5C, 0x2026,
    0x2025, 0x2018, 0x2019, 0x201C, 0x201D, 0xFF08, 0xFF09, 0x3014, 0x3015, 0xFF3B, 0xFF3D, 0xFF5B,
    0xFF5D, 0x3008, 0x3009, 0x300A, 0x300B, 0x300C, 0x300D, 0x300E, 0x300F, 0x3010, 0x3011, 0xFF0B,
    0x2212, 0x00B1, 0x00D7, 0x00F7, 0xFF1D, 0x2260, 0xFF1C, 0xFF1E, 0x2266, 0x2267, 0x221E, 0x2234,
    0x2642, 0x2640, 0x00B0, 0x2032, 0x2033, 0x2103, 0xFFE5, 0xFF04, 0x00A2, 0x00A3, 0xFF05, 0xFF03,
    0xFF06, 0xFF0A, 0xFF20, 0x00A7, 0x2606, 0x2605, 0x25CB, 0x25CF, 0x25CE, 0x25C7, 0x25C6, 0x25A1,
    0x25A0, 0x25B3, 0x25B2, 0x25BD, 0x25BC, 0x203B, 0x3012, 0x2192, 0x2190, 0x2191, 0x2193, 0x3013,
    0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x2208,
    0x220B, 0x2286, 0x2287, 0x2282, 0x2283, 0x222A, 0x2229, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000,
    0x0000, 0x0000, 0x0000, 0x2227, 0x2228, 0x00AC, 0x21D2, 0x21D4, 0x2200, 0x2203, 0x0000, 0x0000,
    0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x2220, 0x22A5, 0x2312,
    0x2202, 0x2207, 0x2261, 0x2252, 0x226A, 0x226B, 0x221A, 0x223D, 0x221D, 0x2235, 0x222B, 0x222C,
    0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x212B, 0x2030, 0x266F, 0x266D, 0x266A,
    0x2020, 0x2021, 0x00B6, 0x0000, 0x0000, 0x0000, 0x0000, 0x25EF,
];

fn trail_index(lo: u8) -> usize {
    (lo as usize - 0x40) - usize::from(lo > 0x7F)
}

/// Decode one character, returning it and how many bytes it consumed.
fn sjis_next(p: &[u8]) -> (char, usize) {
    let hi = p[0];

    if (0x20..=0x7E).contains(&hi) {
        return (hi as char, 1);
    }
    // Half-width katakana.
    if (0xA1..=0xDF).contains(&hi) {
        return (char::from_u32(0xFF61 + (hi as u32 - 0xA1)).unwrap(), 1);
    }

    if p.len() < 2 || !(0x81..=0x83).contains(&hi) {
        return (REPLACEMENT, 1);
    }

    let lo = p[1];
    if !(0x40..=0xFC).contains(&lo) || lo == 0x7F {
        return (REPLACEMENT, 1);
    }

    let cp = match hi {
        0x81 => match SJIS_81.get(trail_index(lo)) {
            Some(&c) if c != 0 => c as u32,
            _ => return (REPLACEMENT, 2),
        },
        0x82 => match lo {
            0x4F..=0x58 => 0xFF10 + (lo as u32 - 0x4F), // full-width digits
            0x60..=0x79 => 0xFF21 + (lo as u32 - 0x60), // full-width A-Z
            0x81..=0x9A => 0xFF41 + (lo as u32 - 0x81), // full-width a-z
            0x9F..=0xF1 => 0x3041 + (lo as u32 - 0x9F), // hiragana
            _ => return (REPLACEMENT, 2),
        },
        _ if lo <= 0x96 => 0x30A1 + trail_index(lo) as u32, // katakana
        _ => return (REPLACEMENT, 2),
    };

    (char::from_u32(cp).unwrap_or(REPLACEMENT), 2)
}

fn decode_nametag(tag: &[u8]) -> Option<String> {
    let mut out = String::new();
    let mut i = 0;
    while i < tag.len() {
        if tag[i] == 0 {
            break;
        }
        let (c, used) = sjis_next(&tag[i..]);
        out.push(c);
        i += used;
    }
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}
