//! vegad's state on this host (docs/design/vegad.md, Lifecycle): shown at
//! the top of the Inbox and in the tab row while it is stopped; `V`
//! starts it. Read from `vega post relay status --json`, started with
//! `vega post relay start` (detached: it keeps none of our pipes).

use crate::bd::output_within;
use crate::uac::resolve_vega;
use anyhow::{bail, Result};
use std::process::Command;
use std::sync::mpsc::{channel, Receiver};
use std::time::Duration;

/// `relay start` waits up to 3 s for the lock.
const TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, PartialEq)]
pub struct Status {
    pub host: String,
    pub running: bool,
    pub pid: Option<String>,
    pub version: Option<String>,
    /// current, stale (binary replaced), unknown, or `-` when stopped.
    pub binary: String,
    pub legacy_unit: bool,
}

impl Status {
    pub fn parse(json: &str) -> Option<Status> {
        let v: serde_json::Value = serde_json::from_str(json.trim()).ok()?;
        let s = |k: &str| v[k].as_str().map(String::from);
        Some(Status {
            host: s("host")?,
            running: v["running"].as_bool()?,
            pid: s("pid"),
            version: s("version"),
            binary: s("binary").unwrap_or_else(|| "-".into()),
            legacy_unit: v["legacy_unit"].is_string(),
        })
    }

    /// One line, e.g. `vegad on proteus: running · pid 812 · 0.1.0 · binary current`.
    pub fn line(&self) -> String {
        if !self.running {
            return format!("vegad on {}: stopped · V starts it", self.host);
        }
        let mut parts = vec![format!("vegad on {}: running", self.host)];
        if let Some(p) = &self.pid {
            parts.push(format!("pid {p}"));
        }
        if let Some(v) = &self.version {
            parts.push(v.clone());
        }
        parts.push(format!("binary {}", self.binary));
        if self.legacy_unit {
            parts.push("legacy unit (vega post relay install)".into());
        }
        parts.join(" · ")
    }
}

fn vega(args: &[&str]) -> Result<String> {
    let Some(vega) = resolve_vega() else {
        bail!("vega not found (PATH, ~/.local/bin): run mise run install in the vega repo");
    };
    let mut cmd = Command::new(vega);
    cmd.args(args);
    let out = output_within(cmd, TIMEOUT)?;
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    if !out.status.success() {
        let line = stdout
            .lines()
            .nth(1)
            .or(stdout.lines().next())
            .unwrap_or("failed");
        bail!("vega {}: {}", args.join(" "), line.trim());
    }
    Ok(stdout)
}

fn read() -> Result<Status, String> {
    let out = vega(&["post", "relay", "status", "--json"]).map_err(|e| e.to_string())?;
    Status::parse(&out).ok_or_else(|| "vega post relay status --json: unreadable".into())
}

/// What a query brings back: what `relay start` said (after `V`), the state.
type Answer = (Option<String>, Result<Status, String>);

/// The last known state and a pending query.
#[derive(Default)]
pub struct Watch {
    pub state: Option<Result<Status, String>>,
    pub loading: Option<Receiver<Answer>>,
    /// What the last `V` said (start's first line or its error).
    pub started: Option<String>,
}

impl Watch {
    /// Queries in the background; one at a time.
    pub fn refresh(&mut self) {
        self.spawn(false);
    }

    /// `relay start`, then a fresh status.
    pub fn start(&mut self) {
        self.spawn(true);
    }

    fn spawn(&mut self, start: bool) {
        if self.loading.is_some() {
            return;
        }
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let said = start.then(|| match vega(&["post", "relay", "start"]) {
                Ok(out) => out.lines().next().unwrap_or("relay: started").to_string(),
                Err(e) => e.to_string(),
            });
            let _ = tx.send((said, read()));
        });
        self.loading = Some(rx);
    }

    /// Takes a finished query; true if something changed.
    pub fn poll(&mut self) -> bool {
        let Some(rx) = &self.loading else {
            return false;
        };
        match rx.try_recv() {
            Ok((said, state)) => {
                if said.is_some() {
                    self.started = said;
                }
                self.state = Some(state);
                self.loading = None;
                true
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => false,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.loading = None;
                false
            }
        }
    }

    /// Known to be stopped (not merely unknown).
    pub fn stopped(&self) -> bool {
        matches!(&self.state, Some(Ok(s)) if !s.running)
    }

    /// The overlay's first line.
    pub fn line(&self) -> String {
        match &self.state {
            Some(Ok(s)) => s.line(),
            Some(Err(e)) => format!("vegad: {e}"),
            None => "vegad: checking...".into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_relay_status_json() {
        let j = r#"{"host":"proteus","running":true,"pid":"812","version":"0.1.0","binary":"current","exe":"~/.local/bin/vega-post","since":"2026-10-10 09:20 CEST","mode":"stream","legacy_unit":null,"log":"~/.local/state/vega/post-relay-proteus.log"}"#;
        let s = Status::parse(j).unwrap();
        assert!(s.running && !s.legacy_unit);
        assert_eq!(
            s.line(),
            "vegad on proteus: running · pid 812 · 0.1.0 · binary current"
        );
        let stopped = r#"{"host":"deimos","running":false,"pid":null,"version":null,"binary":"-","legacy_unit":"~/Library/LaunchAgents/vega.post-relay.plist"}"#;
        let s = Status::parse(stopped).unwrap();
        assert!(!s.running && s.legacy_unit);
        assert_eq!(s.line(), "vegad on deimos: stopped · V starts it");
        assert!(Status::parse("relay: running").is_none());
    }

    #[test]
    fn stopped_only_when_known() {
        let mut w = Watch::default();
        assert!(!w.stopped(), "unknown is not stopped");
        w.state = Some(Err("vega not found".into()));
        assert!(!w.stopped());
        w.state = Some(Ok(Status::parse(r#"{"host":"h","running":false}"#).unwrap()));
        assert!(w.stopped());
    }
}
