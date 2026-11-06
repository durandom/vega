//! vegad's CI watch thread (docs/design/ci-watch.md): runs the poll cycle of
//! crates/vega-ci-watch every few seconds and hands the mails to the relay
//! loop, which publishes them into the watchers' inboxes. `gh` and `tea` calls block
//! for up to 30 s, so they run here, never on the relay loop.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use tokio::sync::mpsc::UnboundedSender;
use vega_ci_watch::forge::Forge;
use vega_ci_watch::{gh, poll, store, tea, unix_now};

/// How often the thread looks at the watches; each target has its own poll
/// interval (30 s pending, 120 s settled).
const EVERY: Duration = Duration::from_secs(5);

/// The `Vega-From-Agent` of CI mail; it has no sender pane to reply to.
pub const AGENT: &str = "vega ci";

/// The agent panes vegad knows are alive; `None` until the first snapshot
/// (absence proves nothing then).
pub type Live = Arc<Mutex<LiveState>>;

#[derive(Default)]
pub struct LiveState {
    pub panes: HashSet<String>,
    pub herdr: bool,
    pub hc: bool,
}

impl LiveState {
    pub fn snapshot(&mut self, panes: impl Iterator<Item = String>, hc: bool) {
        self.panes.retain(|p| crate::terminals::is_hc(p) != hc);
        self.panes.extend(panes);
        if hc {
            self.hc = true;
        } else {
            self.herdr = true;
        }
    }

    fn alive(&self, pane: &str) -> bool {
        let known = if crate::terminals::is_hc(pane) {
            self.hc
        } else {
            self.herdr
        };
        !known || self.panes.contains(crate::terminals::bare(pane))
    }
}

/// (pane, text) mails and log lines for the relay loop.
pub enum Event {
    Mail(String, String),
    Log(String),
}

pub fn spawn(tx: UnboundedSender<Event>, live: Live) {
    thread::spawn(move || loop {
        let now = unix_now();
        let alive = |pane: &str| live.lock().map(|g| g.alive(pane)).unwrap_or(true);
        let mut fetch = |forge: Forge, host: &str, path: &str, etag: Option<&str>| {
            let r = match forge {
                Forge::GitHub => gh::get(host, path, etag)?,
                // No ETags on Forgejo.
                Forge::Forgejo => tea::get(host, path)?,
            };
            store::note_rate(&r, unix_now());
            Ok(r)
        };
        let out = poll::cycle(now, &alive, &mut fetch);
        for l in out.log {
            if tx.send(Event::Log(l)).is_err() {
                return;
            }
        }
        for (pane, text) in out.mails {
            if tx.send(Event::Mail(pane, text)).is_err() {
                return;
            }
        }
        thread::sleep(EVERY);
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unknown_backend_stays_live_until_its_own_snapshot() {
        let mut live = LiveState::default();
        live.snapshot(["w1:p1".into()].into_iter(), false);
        assert!(live.alive("t7"));
        live.snapshot(["t7".into()].into_iter(), true);
        live.snapshot(std::iter::empty(), false);
        assert!(live.alive("hc:t7"));
        assert!(!live.alive("w1:p1"));
        live.snapshot(std::iter::empty(), true);
        assert!(!live.alive("t7"));
    }
}
