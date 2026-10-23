//! `$XDG_STATE_HOME/vega/theme.json`: the mode last applied on this host,
//! where it came from and when. vegad publishes it to the registry as
//! `<host>.theme`; headless hosts read the control host's entry.

use std::fs;
use std::path::PathBuf;

use serde_json::{json, Value};

use crate::{files, Env, Mode};

#[derive(Debug, Clone, PartialEq)]
pub struct Applied {
    pub mode: Mode,
    /// `fixed`, `os` or `registry:<host>` (crate::Source::label).
    pub source: String,
    /// Unix seconds.
    pub at: i64,
}

impl Applied {
    pub fn to_json(&self) -> Value {
        json!({"mode": self.mode.as_str(), "source": self.source, "at": self.at})
    }

    pub fn from_json(v: &Value) -> Option<Applied> {
        Some(Applied {
            mode: Mode::parse(v["mode"].as_str()?)?,
            source: v["source"].as_str().unwrap_or("").to_string(),
            at: v["at"].as_i64().unwrap_or(0),
        })
    }
}

pub fn path(env: &Env) -> PathBuf {
    env.state.join("vega").join("theme.json")
}

pub fn read(env: &Env) -> Option<Applied> {
    let text = fs::read_to_string(path(env)).ok()?;
    Applied::from_json(&serde_json::from_str(&text).ok()?)
}

pub fn write(env: &Env, a: &Applied) -> Result<(), String> {
    let p = path(env);
    if let Some(dir) = p.parent() {
        fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    files::write_atomic(&p, &format!("{}\n", a.to_json()))
}

pub fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let env = Env::at(dir.path());
        assert_eq!(read(&env), None);
        let a = Applied {
            mode: Mode::Light,
            source: "registry:proteus".into(),
            at: 7,
        };
        write(&env, &a).unwrap();
        assert_eq!(read(&env), Some(a));
    }
}
