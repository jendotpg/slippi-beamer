use crate::report;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Published {
    pub name: String,
    pub size: u64,
    pub published_at: u64,
}

#[derive(Debug, Clone)]
pub struct PublishedSet {
    entries: heapless::Vec<Published, { crate::config::KEEP_MAX as usize + 1 }>,
    index_buf: report::Buf<{ report::INDEX_CAP }>,
    station: String,
    cap: usize,
}

impl PublishedSet {
    pub const fn empty() -> PublishedSet {
        PublishedSet {
            entries: heapless::Vec::new(),
            index_buf: report::Buf::new(),
            station: String::new(),
            cap: 1,
        }
    }

    pub fn init(&mut self, station: String, cap: usize) {
        self.station = station;
        self.cap = cap.max(1);
        self.render();
    }

    pub fn admit(&mut self, name: &str, size: u64, at: u64) -> bool {
        if self.contains(name) {
            return false;
        }

        if self
            .entries
            .push(Published {
                name: name.to_owned(),
                size,
                published_at: at,
            })
            .is_err()
        {
            return false;
        }

        self.entries.sort_by(|a, b| {
            b.published_at
                .cmp(&a.published_at)
                .then_with(|| a.name.cmp(&b.name))
        });
        self.entries.truncate(self.cap);

        self.render();
        true
    }

    pub fn contains(&self, name: &str) -> bool {
        self.entries.iter().any(|e| e.name == name)
    }

    pub fn index_str(&self) -> &str {
        self.index_buf.as_str()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.render();
    }

    pub fn set_cap(&mut self, cap: usize) {
        self.cap = cap.max(1);
        if self.entries.len() > self.cap {
            self.entries.truncate(self.cap);
            self.render();
        }
    }

    fn render(&mut self) {
        let PublishedSet {
            entries,
            index_buf,
            station,
            ..
        } = self;
        let mut files: heapless::Vec<(&str, u64), { crate::config::KEEP_MAX as usize }> =
            heapless::Vec::new();
        for e in entries.iter() {
            if files.push((e.name.as_str(), e.size)).is_err() {
                break;
            }
        }
        report::index_json(station, &files, index_buf);
    }
}

pub fn is_replay_name(name: &str) -> bool {
    if name.is_empty() || name.len() > 255 {
        return false;
    }
    if name.starts_with('.') {
        return false;
    }
    if !name
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'_' || b == b'-')
    {
        return false;
    }
    name.len() > 4 && name[name.len() - 4..].eq_ignore_ascii_case(".slp")
}
