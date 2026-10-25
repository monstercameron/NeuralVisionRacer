//! Engine logs for people and agents: JSON lines in a file, plus a bounded in-memory tail the console serves.
//!
//! One record per line, for example:
//! `{"seq":12,"tick":340,"level":"info","module":"physics","msg":"vehicle settled at y=0.41"}`
//! Agents can read the file with any tool, or ask the console for the newest records with `logs`.

use std::collections::VecDeque;
use std::fmt;
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::Path;
use std::str::FromStr;
use std::sync::Mutex;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

impl Level {
    pub fn as_str(self) -> &'static str {
        match self {
            Level::Trace => "trace",
            Level::Debug => "debug",
            Level::Info => "info",
            Level::Warn => "warn",
            Level::Error => "error",
        }
    }
}

impl FromStr for Level {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "trace" => Ok(Level::Trace),
            "debug" => Ok(Level::Debug),
            "info" => Ok(Level::Info),
            "warn" => Ok(Level::Warn),
            "error" => Ok(Level::Error),
            _ => Err(format!("unknown log level `{s}` (trace, debug, info, warn, error)")),
        }
    }
}

impl fmt::Display for Level {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Record {
    pub seq: u64,
    pub tick: u64,
    pub level: Level,
    pub module: String,
    pub message: String,
}

impl Record {
    /// The record as one JSON object, with the string escaped.
    pub fn to_json(&self) -> String {
        format!(
            "{{\"seq\":{},\"tick\":{},\"level\":\"{}\",\"module\":{},\"msg\":{}}}",
            self.seq,
            self.tick,
            self.level,
            json_string(&self.module),
            json_string(&self.message)
        )
    }
}

struct Inner {
    file: Option<File>,
    min_level: Level,
    memory: VecDeque<Record>,
    capacity: usize,
    next_seq: u64,
}

/// Thread-safe logger. Records below `min_level` are dropped before they reach the file or the memory tail.
pub struct Logger {
    inner: Mutex<Inner>,
}

impl Logger {
    /// Opens (or creates, with its folder) the log file. `path = None` keeps logs in memory only.
    pub fn open(path: Option<&Path>, min_level: Level, capacity: usize) -> io::Result<Self> {
        let file = match path {
            Some(p) => {
                if let Some(dir) = p.parent().filter(|d| !d.as_os_str().is_empty()) {
                    fs::create_dir_all(dir)?;
                }
                Some(File::create(p)?)
            }
            None => None,
        };
        Ok(Self {
            inner: Mutex::new(Inner { file, min_level, memory: VecDeque::new(), capacity: capacity.max(1), next_seq: 1 }),
        })
    }

    pub fn set_min_level(&self, level: Level) {
        self.inner.lock().expect("logger lock").min_level = level;
    }

    pub fn min_level(&self) -> Level {
        self.inner.lock().expect("logger lock").min_level
    }

    pub fn log(&self, tick: u64, level: Level, module: &str, message: impl Into<String>) {
        let mut inner = self.inner.lock().expect("logger lock");
        if level < inner.min_level {
            return;
        }
        let record = Record { seq: inner.next_seq, tick, level, module: module.to_string(), message: message.into() };
        inner.next_seq += 1;
        if let Some(file) = inner.file.as_mut() {
            // A failed write must not stop the simulation; the record is still kept in memory.
            let _ = writeln!(file, "{}", record.to_json());
        }
        if inner.memory.len() == inner.capacity {
            inner.memory.pop_front();
        }
        inner.memory.push_back(record);
    }

    /// The newest `count` records at or above `min_level`, oldest first.
    pub fn tail(&self, count: usize, min_level: Level) -> Vec<Record> {
        let inner = self.inner.lock().expect("logger lock");
        let mut picked: Vec<Record> = inner.memory.iter().rev().filter(|r| r.level >= min_level).take(count).cloned().collect();
        picked.reverse();
        picked
    }
}

/// Escapes a string as a JSON string literal.
pub fn json_string(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_below_the_minimum_level_are_dropped() {
        let log = Logger::open(None, Level::Info, 10).unwrap();
        log.log(1, Level::Debug, "x", "hidden");
        log.log(2, Level::Warn, "x", "shown");
        let tail = log.tail(10, Level::Trace);
        assert_eq!(tail.len(), 1);
        assert_eq!(tail[0].message, "shown");
    }

    #[test]
    fn memory_tail_is_bounded_and_ordered() {
        let log = Logger::open(None, Level::Trace, 3).unwrap();
        for i in 0..5 {
            log.log(i, Level::Info, "m", format!("n{i}"));
        }
        let messages: Vec<String> = log.tail(10, Level::Trace).into_iter().map(|r| r.message).collect();
        assert_eq!(messages, ["n2", "n3", "n4"]);
    }

    #[test]
    fn json_lines_escape_quotes_and_newlines() {
        let record = Record { seq: 1, tick: 2, level: Level::Error, module: "a\"b".into(), message: "x\ny".into() };
        assert_eq!(record.to_json(), r#"{"seq":1,"tick":2,"level":"error","module":"a\"b","msg":"x\ny"}"#);
    }
}
