use core::fmt;

/// `1eba09f7-1bc0-42a3-b69f-697f76c34358`, picked randomly (but shared w the pi version)
pub const NAMESPACE: [u8; 16] = [
    0x1e, 0xba, 0x09, 0xf7, 0x1b, 0xc0, 0x42, 0xa3, 0xb6, 0x9f, 0x69, 0x7f, 0x76, 0xc3, 0x43, 0x58,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct StationId([u8; 16]);

impl StationId {
    pub fn from_mac(mac: [u8; 6]) -> Option<StationId> {
        if mac == [0; 6] {
            return None;
        }
        Some(StationId(uuid5(&NAMESPACE, mac_name(mac).as_bytes())))
    }

    pub fn as_bytes(&self) -> &[u8; 16] {
        &self.0
    }

    pub fn usb_serial(&self) -> String {
        format!("BEAMER-{self}")
    }
}

impl fmt::Display for StationId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let b = &self.0;
        for (i, byte) in b.iter().enumerate() {
            if matches!(i, 4 | 6 | 8 | 10) {
                f.write_str("-")?;
            }
            write!(f, "{byte:02x}")?;
        }
        Ok(())
    }
}

pub fn mac_name(mac: [u8; 6]) -> String {
    let mut s = String::with_capacity(12);
    for byte in mac {
        use fmt::Write as _;
        let _ = write!(s, "{byte:02x}");
    }
    s
}

pub fn uuid5(namespace: &[u8; 16], name: &[u8]) -> [u8; 16] {
    let mut h = Sha1::new();
    h.update(namespace);
    h.update(name);
    let digest = h.finish();

    let mut out = [0u8; 16];
    out.copy_from_slice(&digest[..16]);
    out[6] = (out[6] & 0x0f) | 0x50;
    out[8] = (out[8] & 0x3f) | 0x80;
    out
}

pub fn sha1(data: &[u8]) -> [u8; 20] {
    let mut h = Sha1::new();
    h.update(data);
    h.finish()
}

struct Sha1 {
    state: [u32; 5],
    block: [u8; 64],
    used: usize,
    len: u64,
}

impl Sha1 {
    fn new() -> Self {
        Sha1 {
            state: [0x67452301, 0xefcdab89, 0x98badcfe, 0x10325476, 0xc3d2e1f0],
            block: [0; 64],
            used: 0,
            len: 0,
        }
    }

    fn update(&mut self, mut data: &[u8]) {
        self.len = self.len.wrapping_add(data.len() as u64);
        while !data.is_empty() {
            let take = (64 - self.used).min(data.len());
            self.block[self.used..self.used + take].copy_from_slice(&data[..take]);
            self.used += take;
            data = &data[take..];
            if self.used == 64 {
                self.compress();
                self.used = 0;
            }
        }
    }

    fn finish(mut self) -> [u8; 20] {
        let bits = self.len.wrapping_mul(8);
        self.update(&[0x80]);
        while self.used != 56 {
            self.update(&[0x00]);
        }
        self.block[56..].copy_from_slice(&bits.to_be_bytes());
        self.compress();

        let mut out = [0u8; 20];
        for (chunk, word) in out.chunks_exact_mut(4).zip(self.state) {
            chunk.copy_from_slice(&word.to_be_bytes());
        }
        out
    }

    fn compress(&mut self) {
        let mut w = [0u32; 80];
        for (i, chunk) in self.block.chunks_exact(4).enumerate() {
            w[i] = u32::from_be_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }

        let [mut a, mut b, mut c, mut d, mut e] = self.state;
        for (i, &word) in w.iter().enumerate() {
            let (f, k) = match i {
                0..=19 => ((b & c) | (!b & d), 0x5a827999),
                20..=39 => (b ^ c ^ d, 0x6ed9eba1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8f1bbcdc),
                _ => (b ^ c ^ d, 0xca62c1d6),
            };
            let tmp = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(word);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = tmp;
        }

        self.state[0] = self.state[0].wrapping_add(a);
        self.state[1] = self.state[1].wrapping_add(b);
        self.state[2] = self.state[2].wrapping_add(c);
        self.state[3] = self.state[3].wrapping_add(d);
        self.state[4] = self.state[4].wrapping_add(e);
    }
}

pub fn identity() -> Result<StationId, crate::status::Label> {
    use crate::status::Label;
    use esp_idf_svc::sys::{esp_efuse_mac_get_default, ESP_OK};

    let mut mac = [0u8; 6];
    // SAFETY: the callee writes exactly six bytes.
    let err = unsafe { esp_efuse_mac_get_default(mac.as_mut_ptr()) };
    if err != ESP_OK {
        log::error!("esp_efuse_mac_get_default failed: {err}");
        return Err(Label::NoId);
    }
    StationId::from_mac(mac).ok_or(Label::NoId)
}
