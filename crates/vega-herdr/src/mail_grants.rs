//! Receiver-local, expiring grants for exactly one inbox-only peer pair.
//! One atomic file per pair avoids losing unrelated concurrent grants. Revocation
//! is retained as a tombstone for inspection; malformed records fail closed.
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Peer {
    pub host: String,
    pub pane: String,
}

impl Peer {
    pub fn parse(s: &str) -> Result<Self, String> {
        let (host, pane) = s.split_once(':').ok_or("expected <host>:<pane>")?;
        let token = |s: &str| {
            !s.is_empty()
                && s.bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c))
        };
        if !token(host) || !(crate::pane::is_pane_id(pane) || crate::pane::is_hc_id(pane)) {
            return Err(
                "expected literal <host>:w<id>:p<id> (herdr pane) or <host>:t<N> (hc terminal) with alphanumeric IDs (no wildcards)"
                    .into(),
            );
        }
        Ok(Self {
            host: host.to_lowercase(),
            pane: pane.into(),
        })
    }

    pub fn label(&self) -> String {
        format!("{}:{}", self.host, self.pane)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Grant {
    pub sender: Peer,
    pub target: Peer,
    pub created: u64,
    pub expires: u64,
    pub revoked: Option<u64>,
}

impl Grant {
    pub fn live(&self, now: u64) -> bool {
        self.revoked.is_none() && now < self.expires
    }
}

pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

pub fn dir() -> Result<PathBuf, String> {
    let root = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|p| PathBuf::from(p).join(".local/state")))
        .ok_or("HOME is unset")?;
    Ok(root.join("vega/mail-grants"))
}

fn path(sender: &Peer, target: &Peer) -> Result<PathBuf, String> {
    // Only validated peers may become paths, including when reading a corrupt record.
    let sender = Peer::parse(&sender.label())?;
    let target = Peer::parse(&target.label())?;
    Ok(dir()?.join(format!("{}~{}.json", sender.label(), target.label())))
}

pub fn save(g: &Grant) -> Result<(), String> {
    let file = path(&g.sender, &g.target)?;
    std::fs::create_dir_all(file.parent().unwrap()).map_err(|e| e.to_string())?;
    let tmp = file.with_extension(format!("{}.tmp", std::process::id()));
    std::fs::write(
        &tmp,
        serde_json::to_vec_pretty(g).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    std::fs::rename(tmp, file).map_err(|e| e.to_string())
}

pub fn list() -> Result<Vec<Grant>, String> {
    let entries = match std::fs::read_dir(dir()?) {
        Ok(v) => v,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
        Err(e) => return Err(e.to_string()),
    };
    let mut grants = Vec::new();
    for entry in entries {
        let file = entry.map_err(|e| e.to_string())?.path();
        if file.extension().is_none_or(|s| s != "json") {
            continue;
        }
        let g: Grant = serde_json::from_slice(&std::fs::read(&file).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
        if path(&g.sender, &g.target)? != file {
            return Err("grant filename does not match its pair".into());
        }
        grants.push(g);
    }
    grants.sort_by_key(|g| (g.sender.label(), g.target.label()));
    Ok(grants)
}

pub fn matches(sender: &Peer, target: &Peer, now: u64) -> bool {
    let Ok(file) = path(sender, target) else {
        return false;
    };
    std::fs::read(file)
        .ok()
        .and_then(|b| serde_json::from_slice::<Grant>(&b).ok())
        .is_some_and(|g| g.sender == *sender && g.target == *target && g.live(now))
}

pub fn duration(s: &str) -> Option<u64> {
    let (n, unit) = s.split_at_checked(s.len().checked_sub(1)?)?;
    let factor = match unit {
        "s" => 1,
        "m" => 60,
        "h" => 3600,
        "d" => 86400,
        _ => return None,
    };
    n.parse::<u64>()
        .ok()?
        .checked_mul(factor)
        .filter(|n| *n > 0)
}
