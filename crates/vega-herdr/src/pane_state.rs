//! Claude hook telemetry. Deliberately separate from authoritative pane status.
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

const MAX_BYTES: u64 = 1024 * 1024;
const MAX_LINES: usize = 200;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Record {
    pub event: String,
    pub ts: u64,
    pub session_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notification_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub question: Option<bool>,
}

/// The same final-paragraph question test used for screen turn ends.
pub fn ends_with_question(text: &str) -> bool {
    text.trim_end().ends_with('?')
}

pub fn path(pane: &str) -> Option<PathBuf> {
    let mut file = crate::state::state_file_on("pane-state", None, pane)?.into_os_string();
    file.push(".jsonl");
    Some(file.into())
}

fn field(v: &Value, key: &str) -> Option<String> {
    let s = v[key].as_str()?;
    (s.len() <= 256 && !s.chars().any(char::is_control)).then(|| s.to_owned())
}

/// Project only metadata; never serialize arbitrary hook input or message text.
pub fn from_input(input: &str, ts: u64) -> Option<Record> {
    let v: Value = serde_json::from_str(input).ok()?;
    let event = field(&v, "hook_event_name")?;
    if !matches!(
        event.as_str(),
        "SessionStart" | "UserPromptSubmit" | "Stop" | "Notification" | "SessionEnd"
    ) {
        return None;
    }
    Some(Record {
        notification_type: (event == "Notification")
            .then(|| field(&v, "notification_type"))
            .flatten(),
        source: (event == "SessionStart")
            .then(|| field(&v, "source"))
            .flatten(),
        question: (event == "Stop")
            .then(|| ends_with_question(v["last_assistant_message"].as_str().unwrap_or(""))),
        event,
        ts,
        session_id: field(&v, "session_id")?,
    })
}

/// Nonblocking writer lock and atomic replacement keep concurrent hooks bounded.
pub fn append(path: &Path, record: &Record) -> std::io::Result<()> {
    let dir = path
        .parent()
        .ok_or_else(|| std::io::Error::other("no parent"))?;
    fs::create_dir_all(dir)?;
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path.with_extension("lock"))?;
    // The lock is released when the file is dropped, including on errors.
    lock.try_lock().map_err(std::io::Error::other)?;
    let old = match read_bounded(path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e),
    };
    let mut lines: Vec<&str> = old.lines().rev().take(MAX_LINES - 1).collect();
    lines.reverse();
    let mut temp = tempfile::NamedTempFile::new_in(dir)?;
    for line in lines {
        writeln!(temp, "{line}")?;
    }
    writeln!(temp, "{}", serde_json::to_string(record)?)?;
    temp.persist(path).map_err(|e| e.error)?;
    Ok(())
}

fn read_bounded(path: &Path) -> std::io::Result<String> {
    let mut text = String::new();
    fs::File::open(path)?
        .take(MAX_BYTES + 1)
        .read_to_string(&mut text)?;
    if text.len() as u64 > MAX_BYTES {
        return Err(std::io::Error::other("pane state too large"));
    }
    Ok(text)
}

pub fn newest(pane: &str) -> Option<Record> {
    let text = read_bounded(&path(pane)?).ok()?;
    serde_json::from_str(text.lines().last()?).ok()
}
