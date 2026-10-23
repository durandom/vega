//! vegad's theme watcher (docs/design/theme.md, vega-lhc.86): polls the OS
//! appearance every few seconds (`defaults read`, cheap), resolves the due
//! mode from theme.toml (a headless host follows its control host's
//! `<host>.theme` registry entry) and applies a change to every tool in
//! process. The applied mode is state; the relay loop publishes it.
//!
//! Running Claude panes read their theme only at start, so after a change
//! the relay loop types `/config theme=<mode>` into each Claude pane of this
//! host, once, when it is idle or done with an empty input and no dialog.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use tokio::sync::mpsc::UnboundedSender;
use vega_theme_sync::{os, prefs, resolve, state, Env, Mode, Watcher};

/// How often the thread reads the OS appearance.
const EVERY: Duration = Duration::from_secs(3);

/// Fresh hosts' applied modes from the registry, by host token; the relay
/// loop refreshes it after each reconcile.
pub type Remote = Arc<Mutex<HashMap<String, Mode>>>;

pub enum Event {
    Log(String),
    /// The mode changed: switch the running Claude panes.
    Live(Mode),
}

pub fn spawn(tx: UnboundedSender<Event>, remote: Remote) {
    thread::spawn(move || {
        let mut w = Watcher::default();
        let mut last_err = String::new();
        loop {
            if let Some(ev) = tick(&mut w, &remote, &mut last_err) {
                for e in ev {
                    if tx.send(e).is_err() {
                        return;
                    }
                }
            }
            thread::sleep(EVERY);
        }
    });
}

fn tick(w: &mut Watcher, remote: &Remote, last_err: &mut String) -> Option<Vec<Event>> {
    step(&Env::from_env()?, os::mode(), w, remote, last_err)
}

/// One watcher tick against `env` with the OS mode `os_mode`.
pub(crate) fn step(
    env: &Env,
    os_mode: Option<Mode>,
    w: &mut Watcher,
    remote: &Remote,
    last_err: &mut String,
) -> Option<Vec<Event>> {
    let p = match prefs::load(env) {
        Ok(p) => p,
        Err(e) => {
            // Logged once per distinct error; the last good mode stays.
            if *last_err != e {
                *last_err = e.clone();
                return Some(vec![Event::Log(format!("theme: {e}"))]);
            }
            return None;
        }
    };
    last_err.clear();
    let headless = os::headless(p.headless, os_mode);
    let control = vega_registry::token(&p.control);
    let rem = remote.lock().ok().and_then(|r| r.get(&control).copied());
    let (mode, source) = resolve(p.mode, headless, os_mode, &p.control, rem)?;
    w.step(Some(mode))?;
    let before = state::read(env).map(|a| a.mode);
    let live = p.live_claude && is_change(w.last(), before, mode);
    let mut out = Vec::new();
    match vega_theme_sync::apply_due(env, &p, mode, &source) {
        Ok(results) => {
            w.applied(mode);
            let changed: Vec<String> = results
                .iter()
                .filter(|(_, o)| {
                    !matches!(
                        o,
                        vega_theme_sync::adapters::Outcome::Unchanged
                            | vega_theme_sync::adapters::Outcome::Native
                            | vega_theme_sync::adapters::Outcome::Skipped(_)
                    )
                })
                .map(|(t, o)| format!("{t} {}", o.label()))
                .collect();
            out.push(Event::Log(format!(
                "theme: {} ({}){}",
                mode.as_str(),
                source.label(),
                if changed.is_empty() {
                    String::new()
                } else {
                    format!("; {}", changed.join("; "))
                }
            )));
            if live {
                out.push(Event::Live(mode));
            }
        }
        Err(e) => out.push(Event::Log(format!(
            "theme: state not written, retrying: {e}"
        ))),
    }
    Some(out)
}

/// Running panes switch only on a real change: from the mode this vegad
/// applied before, or at its start from the recorded state. A restart or an
/// upgrade with the mode unchanged types nothing, nor does the very first
/// apply on a host (no recorded mode: the panes' theme is not known).
pub fn is_change(last: Option<Mode>, recorded: Option<Mode>, now: Mode) -> bool {
    match last {
        Some(l) => l != now,
        None => recorded.is_some_and(|r| r != now),
    }
}

/// The fresh hosts' modes in a registry view.
pub fn remote_modes(view: &vega_registry::View) -> HashMap<String, Mode> {
    view.entries
        .keys()
        .filter_map(|k| k.strip_suffix(".theme"))
        .filter(|h| !h.contains('.'))
        .filter_map(|h| Some((h.to_string(), Mode::parse(view.theme(h)?)?)))
        .collect()
}

/// The control host's mode on a host that may read only that one registry
/// key (an inbox-only host with `auth-config --registry-read
/// <host>=<control>.theme`, vega-lhc.90): one direct get, no bucket. Without
/// its beat the entry's freshness is unknown; the last published mode counts.
pub async fn control_mode(client: &async_nats::Client) -> Result<HashMap<String, Mode>, String> {
    let Some(env) = Env::from_env() else {
        return Ok(HashMap::new());
    };
    mode_of(client, &prefs::load(&env)?.control).await
}

/// `control`'s published mode, by host token, read with one direct get.
pub async fn mode_of(
    client: &async_nats::Client,
    control: &str,
) -> Result<HashMap<String, Mode>, String> {
    let control = vega_registry::token(control);
    let v = vega_registry::get_one(client, &vega_registry::theme_key(&control)).await?;
    Ok(v.and_then(|v| Mode::parse(v["mode"].as_str()?))
        .map(|m| HashMap::from([(control, m)]))
        .unwrap_or_default())
}

/// This host's `<host>.theme` entry for the reconcile, if it ever applied.
pub fn registry_entry(host: &str) -> Option<(String, serde_json::Value)> {
    let env = Env::from_env()?;
    let a = state::read(&env)?;
    Some((vega_registry::theme_key(host), a.to_json()))
}

/// What to do with a pane waiting for `/config theme=<mode>`.
#[derive(Debug, PartialEq)]
pub enum Action {
    Send,
    Wait,
    Drop,
}

/// `status`: the pane's herdr status, `None` when it is gone or no longer
/// Claude. `screen`: read only for an idle or done pane; `None` when it
/// cannot be read. `held`: vegad stashed a draft there.
pub fn decide(status: Option<&str>, held: bool, screen: impl FnOnce() -> Option<String>) -> Action {
    let Some(status) = status else {
        return Action::Drop;
    };
    if !matches!(status, "idle" | "done") || held {
        return Action::Wait;
    }
    let Some(s) = screen() else {
        return Action::Wait;
    };
    // A dialog on screen (also one herdr does not call blocked), or text in
    // the input: the slash command would answer or join it.
    if vega_herdr::dialog::classify(&s, "claude", "unknown").is_some() {
        return Action::Wait;
    }
    if crate::delivery::draft(&s) != crate::delivery::Draft::None {
        return Action::Wait;
    }
    Action::Send
}

/// The Claude panes waiting for the new mode; each gets it once.
#[derive(Default)]
pub struct Live {
    pub mode: Option<Mode>,
    pub pending: HashSet<String>,
}

impl Live {
    /// A new mode replaces the old one for every Claude pane on this host.
    pub fn start(&mut self, mode: Mode, claude_panes: impl IntoIterator<Item = String>) {
        self.mode = Some(mode);
        self.pending = claude_panes.into_iter().collect();
    }

    pub fn line(&self) -> Option<String> {
        self.mode.map(|m| format!("/config theme={}", m.as_str()))
    }
}

/// `(pane, status)` of this host's Claude agents in `herdr agent list`.
pub fn claude_panes(agents: &[serde_json::Value]) -> HashMap<String, String> {
    agents
        .iter()
        .filter(|a| a["agent"].as_str() == Some("claude"))
        .filter_map(|a| {
            Some((
                a["pane_id"].as_str()?.to_string(),
                a["agent_status"].as_str().unwrap_or("unknown").to_string(),
            ))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn live_only_on_a_real_change() {
        // vegad start, the recorded mode is the same: nothing typed.
        assert!(!is_change(None, Some(Mode::Dark), Mode::Dark));
        // Changed while vegad was down; the first apply ever types nothing.
        assert!(is_change(None, Some(Mode::Light), Mode::Dark));
        assert!(!is_change(None, None, Mode::Dark));
        // An OS change, or `vega theme dark` (which wrote the state first).
        assert!(is_change(Some(Mode::Light), Some(Mode::Dark), Mode::Dark));
        assert!(!is_change(Some(Mode::Dark), Some(Mode::Dark), Mode::Dark));
    }

    #[test]
    fn decide_guards_the_pane() {
        let empty = || Some("output\n────\n❯ \n────".to_string());
        assert_eq!(decide(None, false, empty), Action::Drop);
        assert_eq!(decide(Some("working"), false, empty), Action::Wait);
        assert_eq!(decide(Some("blocked"), false, empty), Action::Wait);
        assert_eq!(decide(Some("unknown"), false, empty), Action::Wait);
        assert_eq!(decide(Some("idle"), true, empty), Action::Wait);
        assert_eq!(decide(Some("idle"), false, || None), Action::Wait);
        assert_eq!(
            decide(Some("idle"), false, || Some("❯ half typed\n────".into())),
            Action::Wait
        );
        assert_eq!(
            decide(Some("done"), false, || Some(
                "Do you want to proceed?\n❯ 1. Yes\n  2. No\n".into()
            )),
            Action::Wait
        );
        assert_eq!(decide(Some("idle"), false, empty), Action::Send);
        assert_eq!(decide(Some("done"), false, empty), Action::Send);
    }

    #[test]
    fn claude_panes_and_line() {
        let agents = [
            json!({"pane_id": "w1:p1", "agent": "claude", "agent_status": "idle"}),
            json!({"pane_id": "w1:p2", "agent": "codex", "agent_status": "idle"}),
        ];
        let c = claude_panes(&agents);
        assert_eq!(c.len(), 1);
        assert_eq!(c["w1:p1"], "idle");
        let mut l = Live::default();
        assert_eq!(l.line(), None);
        l.start(Mode::Light, c.into_keys());
        assert_eq!(l.line().as_deref(), Some("/config theme=light"));
        assert!(l.pending.contains("w1:p1"));
    }

    #[test]
    fn headless_host_follows_the_fake_registry() {
        let dir = tempfile::tempdir().unwrap();
        let env = Env::at(dir.path());
        std::fs::create_dir_all(env.config.join("vega")).unwrap();
        std::fs::write(
            env.config.join("vega/theme.toml"),
            "control = \"proteus\"\n",
        )
        .unwrap();
        std::fs::write(dir.path().join(".claude.json"), "{\"theme\": \"light\"}").unwrap();
        let remote: Remote = Default::default();
        let mut w = Watcher::default();
        let mut err = String::new();
        // No appearance (headless) and nothing published: nothing happens.
        assert!(step(&env, None, &mut w, &remote, &mut err).is_none());
        assert!(state::read(&env).is_none());
        remote.lock().unwrap().insert("proteus".into(), Mode::Dark);
        let ev = step(&env, None, &mut w, &remote, &mut err).unwrap();
        assert!(!ev.iter().any(|e| matches!(e, Event::Live(_))));
        let a = state::read(&env).unwrap();
        assert_eq!(
            (a.mode, a.source.as_str()),
            (Mode::Dark, "registry:proteus")
        );
        assert!(std::fs::read_to_string(dir.path().join(".claude.json"))
            .unwrap()
            .contains("dark"));
        // Unchanged: no second apply.
        assert!(step(&env, None, &mut w, &remote, &mut err).is_none());
        // The control host turns light.
        remote.lock().unwrap().insert("proteus".into(), Mode::Light);
        let ev = step(&env, None, &mut w, &remote, &mut err).unwrap();
        assert!(ev.iter().any(|e| matches!(e, Event::Live(Mode::Light))));
    }

    #[test]
    fn os_change_applies_and_restart_types_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let env = Env::at(dir.path());
        let remote: Remote = Default::default();
        let mut err = String::new();
        let mut w = Watcher::default();
        let ev = step(&env, Some(Mode::Light), &mut w, &remote, &mut err).unwrap();
        assert!(!ev.iter().any(|e| matches!(e, Event::Live(_))));
        assert!(step(&env, Some(Mode::Light), &mut w, &remote, &mut err).is_none());
        let ev = step(&env, Some(Mode::Dark), &mut w, &remote, &mut err).unwrap();
        assert!(ev.iter().any(|e| matches!(e, Event::Live(Mode::Dark))));
        // A new vegad (restart, upgrade) with the same mode: applies, no typing.
        let mut w2 = Watcher::default();
        let ev = step(&env, Some(Mode::Dark), &mut w2, &remote, &mut err).unwrap();
        assert!(!ev.iter().any(|e| matches!(e, Event::Live(_))));
        // live_claude = false: never typed.
        std::fs::create_dir_all(env.config.join("vega")).unwrap();
        std::fs::write(env.config.join("vega/theme.toml"), "live_claude = false\n").unwrap();
        let ev = step(&env, Some(Mode::Light), &mut w2, &remote, &mut err).unwrap();
        assert!(!ev.iter().any(|e| matches!(e, Event::Live(_))));
        // A broken theme.toml is logged once, then quiet.
        std::fs::write(env.config.join("vega/theme.toml"), "mode = \"x\"\n").unwrap();
        assert!(step(&env, Some(Mode::Dark), &mut w2, &remote, &mut err).is_some());
        assert!(step(&env, Some(Mode::Dark), &mut w2, &remote, &mut err).is_none());
    }

    #[test]
    fn remote_modes_of_fresh_hosts() {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        let mut v = vega_registry::View::default();
        v.entries.insert("proteus.beat".into(), json!({"at": now}));
        v.entries
            .insert("proteus.theme".into(), json!({"mode": "dark"}));
        v.entries
            .insert("io-wsl.beat".into(), json!({"at": now - 600}));
        v.entries
            .insert("io-wsl.theme".into(), json!({"mode": "light"}));
        let m = remote_modes(&v);
        assert_eq!(m.get("proteus"), Some(&Mode::Dark));
        assert_eq!(m.get("io-wsl"), None);
    }
}
