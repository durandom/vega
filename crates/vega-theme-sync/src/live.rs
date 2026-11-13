//! Durable live-Claude intent and delivery receipts. The watcher owns the request;
//! the relay owns the journal, so applying tool settings cannot overwrite receipts.
use std::collections::{BTreeMap, BTreeSet};
use std::fs;

use serde::{Deserialize, Serialize};

use crate::{files, Env, Mode};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Request {
    pub id: String,
    pub mode: Mode,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Pane {
    /// Last mode successfully sent, not an acknowledgement from Claude.
    pub applied: Option<Mode>,
    pub error: Option<String>,
    pub attempts: u32,
    pub retry_at: i64,
    pub pending: bool,
}

impl Pane {
    pub fn status(&self) -> String {
        if let Some(e) = &self.error {
            format!("failed: {e}")
        } else if self.pending {
            "pending".into()
        } else if self.applied.is_some() {
            "applied".into()
        } else {
            "unknown".into()
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Journal {
    pub request: Option<Request>,
    /// Backends whose recipients have been collected for this request.
    pub collected: BTreeSet<String>,
    pub panes: BTreeMap<String, Pane>,
}

fn read<T: serde::de::DeserializeOwned>(env: &Env, name: &str) -> Result<Option<T>, String> {
    let path = env.state.join("vega").join(name);
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    serde_json::from_str(&text)
        .map(Some)
        .map_err(|e| format!("{}: {e}", path.display()))
}

fn write(env: &Env, name: &str, value: &impl Serialize) -> Result<(), String> {
    let path = env.state.join("vega").join(name);
    fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
    let text = format!(
        "{}\n",
        serde_json::to_string(value).map_err(|e| e.to_string())?
    );
    if fs::read_to_string(&path).ok().as_deref() == Some(&text) {
        return Ok(());
    }
    files::write_atomic(&path, &text)
}

pub fn request(env: &Env, mode: Mode) -> Result<(), String> {
    let id = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_nanos()
        .to_string();
    write(env, "theme-request.json", &Request { id, mode })
}

pub fn read_request(env: &Env) -> Result<Option<Request>, String> {
    read(env, "theme-request.json")
}

impl Journal {
    pub fn load(env: &Env) -> Result<Self, String> {
        Ok(read(env, "theme-live.json")?.unwrap_or_default())
    }

    pub fn save(&self, env: &Env) -> Result<(), String> {
        write(env, "theme-live.json", self)
    }

    pub fn accept(&mut self, request: Request) {
        if self.request.as_ref().is_some_and(|r| r.id == request.id) {
            return;
        }
        self.request = Some(request);
        self.collected.clear();
        // Keep last sent modes while replacing obsolete pending work.
        for pane in self.panes.values_mut() {
            pane.pending = true;
            pane.error = None;
            pane.attempts = 0;
            pane.retry_at = 0;
        }
    }

    /// Called only for a successful inventory. Outages neither collect nor prune.
    pub fn inventory(&mut self, backend: &str, panes: impl IntoIterator<Item = String>) {
        let panes: BTreeSet<_> = panes.into_iter().collect();
        self.panes.retain(|id, _| {
            let hc = vega_herdr::pane::is_hc_id(id) || id.starts_with("hc:");
            (backend == "hc") != hc || panes.contains(id)
        });
        for id in &panes {
            self.panes.entry(id.clone()).or_default();
        }
        if self.request.is_some() && self.collected.insert(backend.into()) {
            for id in panes {
                let pane = self.panes.entry(id).or_default();
                pane.pending = true;
            }
        }
    }

    pub fn due(&self, now: i64) -> Vec<String> {
        self.panes
            .iter()
            .filter(|(_, p)| p.pending && p.retry_at <= now)
            .map(|(id, _)| id.clone())
            .collect()
    }

    /// The same injectable delivery boundary is used by vegad and fake transports.
    pub fn send(
        &mut self,
        env: &Env,
        id: &str,
        now: i64,
        send: impl FnOnce() -> Result<(), String>,
    ) -> Result<(), String> {
        self.result(id, now, send());
        self.save(env)
    }

    pub fn result(&mut self, id: &str, now: i64, result: Result<(), String>) {
        let Some(pane) = self.panes.get_mut(id) else {
            return;
        };
        pane.attempts = pane.attempts.saturating_add(1);
        match result {
            Ok(()) => {
                pane.applied = self.request.as_ref().map(|r| r.mode);
                pane.pending = false;
                pane.error = None;
                pane.retry_at = 0;
            }
            Err(error) => {
                pane.error = Some(error);
                pane.retry_at = now.saturating_add((2_i64 << pane.attempts.min(7)).min(120));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timeout_then_restart_retries_and_keeps_success_receipt() {
        let temp = tempfile::tempdir().unwrap();
        let env = Env::at(temp.path());
        request(&env, Mode::Dark).unwrap();
        let mut journal = Journal::load(&env).unwrap();
        journal.accept(read_request(&env).unwrap().unwrap());
        journal.inventory("hc", ["t31".into(), "t3p".into()]);
        journal.save(&env).unwrap();
        let mut calls = 0;
        let mut fake_hc = || {
            calls += 1;
            if calls == 1 {
                Err("hc_timeout hc did not answer before the deadline".into())
            } else {
                Ok(())
            }
        };
        journal.send(&env, "t31", 100, &mut fake_hc).unwrap();
        journal.send(&env, "t3p", 100, &mut fake_hc).unwrap();
        drop(journal);
        let mut restarted = Journal::load(&env).unwrap();
        restarted.accept(read_request(&env).unwrap().unwrap());
        assert!(restarted.panes["t31"]
            .status()
            .starts_with("failed: hc_timeout"));
        assert_eq!(restarted.panes["t3p"].applied, Some(Mode::Dark));
        assert!(restarted.due(103).is_empty());
        assert_eq!(restarted.due(104), ["t31"]);
        restarted.send(&env, "t31", 104, &mut fake_hc).unwrap();
        assert_eq!(calls, 3);
        let loaded = Journal::load(&env).unwrap();
        assert!(loaded.due(999).is_empty());
        assert_eq!(loaded.panes["t31"].status(), "applied");
    }

    #[test]
    fn intent_survives_before_recipient_collection_and_outages_do_not_drop_work() {
        let temp = tempfile::tempdir().unwrap();
        let env = Env::at(temp.path());
        request(&env, Mode::Light).unwrap();
        // Restart before any event was consumed or recipients collected.
        let mut journal = Journal::load(&env).unwrap();
        journal.accept(read_request(&env).unwrap().unwrap());
        journal.inventory("hc", ["t1k".into()]);
        journal.save(&env).unwrap();
        let mut journal = Journal::load(&env).unwrap();
        // Only herdr answered; hc must remain pending.
        journal.inventory("herdr", ["w1:p1".into()]);
        assert_eq!(journal.due(0), ["t1k", "w1:p1"]);
        // A newer mode supersedes old attempts, retaining last sent mode.
        journal.result("t1k", 1, Ok(()));
        journal.result("w1:p1", 1, Err("timeout".into()));
        journal.accept(Request {
            id: "new".into(),
            mode: Mode::Dark,
        });
        journal.inventory("hc", ["t1k".into()]);
        assert_eq!(journal.panes["t1k"].applied, Some(Mode::Light));
        assert_eq!(journal.panes["t1k"].status(), "pending");
        assert_eq!(journal.panes["t1k"].attempts, 0);
        journal.inventory("herdr", []);
        assert!(!journal.panes.contains_key("w1:p1"));
        for n in 0..30 {
            journal.result("t1k", n, Err("timeout".into()));
        }
        assert_eq!(journal.panes["t1k"].retry_at, 29 + 120);
    }

    #[test]
    fn corrupted_journal_is_an_error_and_first_inventory_is_unknown() {
        let temp = tempfile::tempdir().unwrap();
        let env = Env::at(temp.path());
        let mut journal = Journal::default();
        journal.inventory("hc", ["t1".into()]);
        assert_eq!(journal.panes["t1"].status(), "unknown");
        journal.save(&env).unwrap();
        fs::write(env.state.join("vega/theme-live.json"), "broken").unwrap();
        assert!(Journal::load(&env).is_err());
    }
}
