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
pub type Live = Arc<Mutex<Option<HashSet<String>>>>;

/// (pane, text) mails and log lines for the relay loop.
pub enum Event {
    Mail(String, String),
    Log(String),
}

pub fn spawn(tx: UnboundedSender<Event>, live: Live) {
    thread::spawn(move || loop {
        let now = unix_now();
        let alive = |pane: &str| {
            live.lock()
                .map(|g| g.as_ref().is_none_or(|s| s.contains(pane)))
                .unwrap_or(true)
        };
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
