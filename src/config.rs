use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

#[derive(Clone, PartialEq)]
pub enum Item {
    Bool(bool),
    Int(i64),
    Num(f64),
    Text(String),
}

pub struct Config {
    path: Option<PathBuf>,
    values: BTreeMap<String, Item>,
    extra: Vec<String>,
    pending: BTreeMap<String, Option<Item>>,
    seen: Option<String>,
    error: String,
    saved: Instant,
    checked: Instant,
    tick: Duration,
}

pub fn valid_key(key: &str) -> bool {
    !key.is_empty() && key.trim() == key && !key.starts_with('#') && !key.contains(['=', '\n', '\r'])
}

fn format(item: &Item) -> String {
    match item {
        Item::Bool(b) => b.to_string(),
        Item::Int(i) => i.to_string(),
        Item::Num(n) => format!("{n:?}"),
        Item::Text(s) => {
            let mut out = String::from("\"");
            for c in s.chars() {
                match c {
                    '"' => out.push_str("\\\""),
                    '\\' => out.push_str("\\\\"),
                    '\n' => out.push_str("\\n"),
                    '\r' => out.push_str("\\r"),
                    '\t' => out.push_str("\\t"),
                    c => out.push(c),
                }
            }
            out.push('"');
            out
        }
    }
}

fn parse_text(quoted: &str) -> Option<String> {
    let inner = quoted.strip_prefix('"')?.strip_suffix('"')?;
    let mut out = String::new();
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => out.push(match chars.next()? {
                '"' => '"',
                '\\' => '\\',
                'n' => '\n',
                'r' => '\r',
                't' => '\t',
                _ => return None,
            }),
            '"' => return None,
            c => out.push(c),
        }
    }
    Some(out)
}

fn parse_line(line: &str) -> Option<(String, Item)> {
    let (key, value) = line.split_once('=')?;
    let (key, value) = (key.trim(), value.trim());
    if !valid_key(key) {
        return None;
    }
    let item = match value {
        "true" => Item::Bool(true),
        "false" => Item::Bool(false),
        _ if value.starts_with('"') => Item::Text(parse_text(value)?),
        _ => value
            .parse()
            .map(Item::Int)
            .or_else(|_| value.parse().map(Item::Num))
            .ok()?,
    };
    Some((key.to_owned(), item))
}

impl Config {
    pub fn new(path: Option<PathBuf>) -> Config {
        let now = Instant::now();
        let mut config = Config {
            path,
            values: BTreeMap::new(),
            extra: Vec::new(),
            pending: BTreeMap::new(),
            seen: None,
            error: String::new(),
            saved: now,
            checked: now,
            tick: Duration::from_millis(16),
        };
        config.check();
        config.apply();
        config
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    pub fn get(&self, key: &str) -> Option<&Item> {
        self.values.get(key)
    }

    pub fn all(&self) -> &BTreeMap<String, Item> {
        &self.values
    }

    pub fn tick(&self) -> Duration {
        self.tick
    }

    pub fn set(&mut self, key: &str, item: Option<Item>) {
        if self.values.get(key) == item.as_ref() {
            return;
        }
        match &item {
            Some(item) => self.values.insert(key.to_owned(), item.clone()),
            None => self.values.remove(key),
        };
        self.pending.insert(key.to_owned(), item);
        self.apply();
    }

    pub fn poll(&mut self) {
        if self.path.is_none() {
            return;
        }
        if !self.pending.is_empty() && self.saved.elapsed() >= Duration::from_secs(1) {
            self.save();
        } else if self.checked.elapsed() >= Duration::from_millis(500) {
            self.check();
        }
    }

    pub fn flush(&mut self) {
        if self.path.is_some() && !self.pending.is_empty() {
            self.save();
        }
    }

    fn apply(&mut self) {
        let tick = match self.values.get("engine.tick_ms") {
            Some(Item::Int(i)) => *i,
            Some(Item::Num(n)) => *n as i64,
            _ => 16,
        };
        self.tick = Duration::from_millis(tick.clamp(1, 1000) as u64);
        crate::log_to_file(!matches!(self.values.get("engine.log_file"), Some(Item::Bool(false))));
    }

    fn report(&mut self, text: String) {
        if text != self.error {
            crate::log(&text);
            self.error = text;
        }
    }

    fn check(&mut self) -> bool {
        self.checked = Instant::now();
        let Some(path) = &self.path else {
            return false;
        };
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => {
                self.report(format!("{}: {e}; keeping the settings in memory", path.display()));
                return false;
            }
        };
        if self.seen.as_ref() == Some(&text) {
            return true;
        }
        let mut values = BTreeMap::new();
        let mut extra = Vec::new();
        let mut bad = Vec::new();
        for (n, line) in text.trim_start_matches('\u{feff}').lines().enumerate() {
            match parse_line(line) {
                Some((key, item)) => {
                    values.insert(key, item);
                }
                None if line.trim().is_empty() => {}
                None => {
                    if !line.trim_start().starts_with('#') && !self.extra.iter().any(|b| b == line) {
                        bad.push(format!("line {}: {line}", n + 1));
                    }
                    extra.push(line.to_owned());
                }
            }
        }
        for (key, item) in &self.pending {
            match item {
                Some(item) => values.insert(key.clone(), item.clone()),
                None => values.remove(key),
            };
        }
        let name = path.display().to_string();
        let reloaded = self.seen.is_some();
        self.values = values;
        self.extra = extra;
        self.seen = Some(text);
        self.error.clear();
        self.apply();
        if !bad.is_empty() {
            crate::log(&format!("{name}: not key = value, kept as written: {}", bad.join("; ")));
        }
        if reloaded {
            crate::log(&format!("{name} changed on disk, reloaded"));
        }
        true
    }

    fn save(&mut self) {
        self.saved = Instant::now();
        if !self.check() {
            return;
        }
        let Some(path) = self.path.clone() else {
            return;
        };
        let mut text = String::new();
        for line in &self.extra {
            text.push_str(line);
            text.push('\n');
        }
        for (key, item) in &self.values {
            text.push_str(&format!("{key} = {}\n", format(item)));
        }
        if self.seen.as_ref() != Some(&text) {
            let temp = path.with_extension(format!("cfg.{}.tmp", std::process::id()));
            if let Err(e) = std::fs::write(&temp, &text).and_then(|()| std::fs::rename(&temp, &path)) {
                let _ = std::fs::remove_file(&temp);
                return self.report(format!("{}: {e}; will try again", path.display()));
            }
            self.seen = Some(text);
        }
        self.error.clear();
        self.pending.clear();
    }
}
