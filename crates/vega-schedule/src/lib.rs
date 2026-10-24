//! The model of vegad's timed tasks (docs/design/schedule.md, vega-lhc.204.2).
//!
//! Pure data and files, no thread, runner, mail or CLI:
//!
//! - [`when`] - `daily`, `weekly` and once (`at`/`in`) slots in the host's
//!   time zone, with the DST rules (a skipped local time fires at the first
//!   valid minute after it, a repeated one at its first occurrence).
//! - [`slot`] - catch-up state (`since`, `pending`, `last_slot`) and the
//!   decision a tick takes; shared by jobs and reminders.
//! - [`job`] - `.vega/schedule.toml` (unknown keys fail), the repo-bound path
//!   rules and the approval hash.
//! - [`approvals`] - `$XDG_CONFIG_HOME/vega/schedule-approved.toml`.
//! - [`run`] - per job `state.json` and run files under
//!   `$XDG_STATE_HOME/vega/schedule/<hex project id>/<job>/`.
//! - [`reminder`] - the reminder store, definitions written by the CLI and
//!   firing state written by vegad.

use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{de::DeserializeOwned, Serialize};

pub use jiff;

pub mod approvals;
pub mod job;
pub mod reminder;
pub mod run;
pub mod slot;
pub mod when;

/// An error with a stable code (`schedule_invalid`, `path_invalid`, ...) for
/// AXI output, and a message naming what is wrong.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    pub code: &'static str,
    pub message: String,
}

impl Error {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Error {
            code,
            message: message.into(),
        }
    }

    fn io(path: &Path, e: impl fmt::Display) -> Self {
        Error::new("io", format!("{}: {e}", path.display()))
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for Error {}

/// `$XDG_STATE_HOME/vega`, else `~/.local/state/vega`.
pub fn state_dir() -> Option<PathBuf> {
    xdg("XDG_STATE_HOME", ".local/state")
}

/// `$XDG_CONFIG_HOME/vega`, else `~/.config/vega`.
pub fn config_dir() -> Option<PathBuf> {
    xdg("XDG_CONFIG_HOME", ".config")
}

fn xdg(var: &str, fallback: &str) -> Option<PathBuf> {
    std::env::var_os(var)
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| Path::new(&h).join(fallback)))
        .map(|d| d.join("vega"))
}

fn tmp_path(path: &Path) -> PathBuf {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("file");
    path.with_file_name(format!(".{name}.tmp{}", std::process::id()))
}

fn write_tmp(path: &Path, data: &[u8]) -> Result<PathBuf, Error> {
    let parent = path.parent().ok_or_else(|| Error::io(path, "no parent"))?;
    fs::create_dir_all(parent).map_err(|e| Error::io(parent, e))?;
    let tmp = tmp_path(path);
    fs::write(&tmp, data).map_err(|e| Error::io(&tmp, e))?;
    Ok(tmp)
}

/// Replaces `path` via a temp file and a rename: a reader never sees half a
/// file.
pub(crate) fn write_atomic(path: &Path, data: &[u8]) -> Result<(), Error> {
    let tmp = write_tmp(path, data)?;
    fs::rename(&tmp, path).map_err(|e| {
        let _ = fs::remove_file(&tmp);
        Error::io(path, e)
    })
}

/// Creates `path` with its full content only if it does not exist (temp
/// file plus hard link). `Ok(false)`: it existed, nothing written.
pub(crate) fn create_new(path: &Path, data: &[u8]) -> Result<bool, Error> {
    let tmp = write_tmp(path, data)?;
    let res = fs::hard_link(&tmp, path);
    let _ = fs::remove_file(&tmp);
    match res {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(false),
        Err(e) => Err(Error::io(path, e)),
    }
}

pub(crate) fn to_json<T: Serialize>(value: &T) -> Vec<u8> {
    let mut v = serde_json::to_vec_pretty(value).expect("model types serialize");
    v.push(b'\n');
    v
}

/// `Ok(None)` when the file is missing; a file that does not parse is an
/// error, never silently empty.
pub(crate) fn read_json<T: DeserializeOwned>(path: &Path) -> Result<Option<T>, Error> {
    match fs::read_to_string(path) {
        Ok(s) => serde_json::from_str(&s)
            .map(Some)
            .map_err(|e| Error::new("state_invalid", format!("{}: {e}", path.display()))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(Error::io(path, e)),
    }
}

pub(crate) fn remove_if_exists(path: &Path) -> Result<bool, Error> {
    match fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(Error::io(path, e)),
    }
}

/// Reversible UTF-8 hex of a project id, the directory name under
/// `schedule/` (ids hold `/` and `:`), as vega_herdr's v2 storage does.
pub fn encode_component(value: &str) -> String {
    value.bytes().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_new_never_overwrites() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a/b.json");
        assert!(create_new(&p, b"one").unwrap());
        assert!(!create_new(&p, b"two").unwrap());
        assert_eq!(fs::read_to_string(&p).unwrap(), "one");
        assert_eq!(fs::read_dir(p.parent().unwrap()).unwrap().count(), 1);
    }

    #[test]
    fn broken_json_is_an_error_not_empty() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("s.json");
        assert_eq!(read_json::<serde_json::Value>(&p).unwrap(), None);
        fs::write(&p, "{").unwrap();
        assert_eq!(
            read_json::<serde_json::Value>(&p).unwrap_err().code,
            "state_invalid"
        );
    }
}
