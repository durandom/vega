//! Local vegad loop-budget evidence, shared by UAC and janitor views.
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub const STALE_SECS: i64 = 120;

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Sample {
    pub last_ms: u64,
    pub budget_ms: u64,
    pub consecutive: u64,
    pub active: bool,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Health {
    pub at: i64,
    pub samples: BTreeMap<String, Sample>,
}

pub fn path() -> Option<PathBuf> {
    let root = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/state")))?;
    Some(root.join("vega/vegad-health.json"))
}

impl Health {
    /// Three consecutive overruns trigger attention; a healthy observation clears it.
    pub fn observe(&mut self, step: &str, took_ms: u64, budget_ms: u64) {
        // Local and broker loops are mutually exclusive modes, not separate jobs.
        match step {
            "loop" => {
                self.samples.remove("local loop");
            }
            "local loop" => {
                self.samples.remove("loop");
            }
            _ => {}
        }
        let s = self.samples.entry(step.into()).or_default();
        s.last_ms = took_ms;
        s.budget_ms = budget_ms;
        s.consecutive = if took_ms > budget_ms {
            s.consecutive.saturating_add(1)
        } else {
            0
        };
        s.active = s.consecutive >= 3;
    }

    pub fn write(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, serde_json::to_vec(self)?)?;
        std::fs::rename(tmp, path)
    }
}

pub fn render(health: Option<&Health>, now: i64) -> String {
    let Some(h) = health.filter(|h| h.at <= now && now.saturating_sub(h.at) <= STALE_SECS) else {
        return "vegad: unknown (loop health missing or older than 120 s)\n".into();
    };
    let active: Vec<_> = h.samples.iter().filter(|(_, s)| s.active).collect();
    if active.is_empty() {
        return "vegad: within loop budgets\n".into();
    }
    let mut out = format!(
        "vegad: over budget ({} phases repeatedly over budget; local host)\n",
        active.len()
    );
    out += &format!(
        "vegad_phases[{}]{{phase,last_ms,budget_ms,consecutive}}:\n",
        active.len()
    );
    for (name, s) in active {
        out += &format!(
            "  {},{},{},{}\n",
            crate::cell(name),
            s.last_ms,
            s.budget_ms,
            s.consecutive
        );
    }
    out += "help: vega post relay status  # daemon state and recent log\n";
    out
}

pub fn status() -> String {
    let health = path()
        .and_then(|p| std::fs::read(p).ok())
        .and_then(|b| serde_json::from_slice::<Health>(&b).ok());
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64);
    render(health.as_ref(), now)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn repeated_overruns_recover_and_stale_evidence_is_unknown() {
        let mut h = Health {
            at: 100,
            ..Default::default()
        };
        for _ in 0..2 {
            h.observe("registry", 6000, 5000);
        }
        assert!(!render(Some(&h), 100).contains("over budget"));
        h.observe("registry", 7000, 5000);
        assert!(render(Some(&h), 100).contains("registry,7000,5000,3"));
        assert!(render(Some(&h), 221).contains("unknown"));
        assert!(render(Some(&h), 99).contains("unknown"));
        h.observe("registry", 5000, 5000);
        assert!(render(Some(&h), 100).contains("within loop budgets"));
        assert!(render(None, 100).contains("unknown"));
        for _ in 0..3 {
            h.observe("loop", 6000, 5000);
        }
        assert!(render(Some(&h), 100).contains("loop,6000"));
        h.observe("local loop", 100, 5000);
        assert!(render(Some(&h), 100).contains("within loop budgets"));
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("health.json");
        h.write(&path).unwrap();
        let roundtrip: Health = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        assert_eq!(render(Some(&h), 100), render(Some(&roundtrip), 100));
    }
}
