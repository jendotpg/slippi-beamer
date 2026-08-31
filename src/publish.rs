use crate::report;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Published {
    pub name: String,
    pub size: u64,
    pub published_at: u64,
}

#[derive(Debug, Clone)]
pub struct PublishedSet {
    entries: Vec<Published>,
    index_json: Vec<u8>,
    station: String,
    cap: usize,
}

impl PublishedSet {
    pub fn new(station: String, cap: usize) -> PublishedSet {
        let mut set = PublishedSet {
            entries: Vec::new(),
            index_json: Vec::new(),
            station,
            cap: cap.max(1),
        };
        set.render();
        set
    }

    pub fn admit(&mut self, name: &str, size: u64, at: u64) -> bool {
        if self.contains(name) {
            return false;
        }

        self.entries.push(Published {
            name: name.to_owned(),
            size,
            published_at: at,
        });

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

    pub fn index_json(&self) -> &[u8] {
        &self.index_json
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
        let files: Vec<(String, u64)> = self
            .entries
            .iter()
            .map(|e| (e.name.clone(), e.size))
            .collect();
        self.index_json = report::index_json(&self.station, &files);
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
