//! vegad's sidebar reports, sent from their own thread (docs/design/vegad.md,
//! Sidebar): each metadata refresh hands over its `herdr pane|workspace
//! report-metadata` commands; this thread sends a target's commands when they
//! changed, and again before herdr's TTL drops them. A loop stalled on the
//! broker, the registry or a slow refresh keeps its labels.

use std::collections::HashMap;
use std::sync::mpsc;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

/// How long herdr keeps vegad's sidebar tokens without a report: they vanish
/// on their own when vegad stops. Far above RENEW_AFTER + TICK, so a slow
/// herdr round or a starved process does not blank them.
pub const TTL_MS: &str = "300000";

/// Unchanged tokens are sent again after this long.
const RENEW_AFTER: Duration = Duration::from_secs(60);

/// The thread's wake-up without a new refresh.
const TICK: Duration = Duration::from_secs(5);

type Cmd = Vec<String>;
/// `pane`/`workspace` and its id: one sidebar row.
type Key = (String, String);

/// What the sidebar should show and what herdr was last sent.
#[derive(Default)]
pub struct State {
    /// The latest refresh's commands by target, in their order.
    wanted: Vec<(Key, Vec<Cmd>)>,
    sent: HashMap<Key, (Vec<Cmd>, Instant)>,
    /// Commands that clear rows a refresh no longer names.
    withdrawn: Vec<(Key, Cmd)>,
    /// Withdrawals that failed and wait for the next tick, logged once.
    failing: std::collections::HashSet<Key>,
}

fn key(cmd: &Cmd) -> Key {
    let at = |i: usize| cmd.get(i).cloned().unwrap_or_default();
    (at(0), at(2))
}

/// The command that clears what `cmds` set on their target: each token,
/// the display agent, title and state labels. herdr has no clear-all.
fn withdraw(cmds: &[Cmd]) -> Option<Cmd> {
    let first = cmds.first()?;
    let source = first
        .iter()
        .position(|a| a == "--source")
        .and_then(|i| first.get(i + 1))?;
    let mut out: Cmd = first.iter().take(3).cloned().collect();
    out.extend(["--source".into(), source.clone()]);
    let mut clears: Vec<Cmd> = Vec::new();
    for cmd in cmds {
        for pair in cmd.windows(2) {
            let clear = match pair[0].as_str() {
                "--token" | "--clear-token" => {
                    let name = pair[1].split('=').next().unwrap_or_default();
                    vec!["--clear-token".to_string(), name.to_string()]
                }
                "--display-agent" | "--clear-display-agent" => vec!["--clear-display-agent".into()],
                "--title" | "--clear-title" => vec!["--clear-title".into()],
                "--state-label" | "--clear-state-labels" => vec!["--clear-state-labels".into()],
                _ => continue,
            };
            if !clears.contains(&clear) {
                clears.push(clear);
            }
        }
    }
    out.extend(clears.into_iter().flatten());
    Some(out)
}

impl State {
    /// A new refresh: its commands replace the last ones; targets it no
    /// longer names are withdrawn, not left to the TTL.
    pub fn set(&mut self, cmds: Vec<Cmd>) {
        let mut wanted: Vec<(Key, Vec<Cmd>)> = Vec::new();
        for cmd in cmds {
            let k = key(&cmd);
            match wanted.iter_mut().find(|(w, _)| *w == k) {
                Some((_, v)) => v.push(cmd),
                None => wanted.push((k, vec![cmd])),
            }
        }
        let dropped: Vec<Key> = self
            .sent
            .keys()
            .filter(|k| !wanted.iter().any(|(w, _)| w == *k))
            .cloned()
            .collect();
        for k in dropped {
            if let Some(c) = self.sent.remove(&k).and_then(|(cmds, _)| withdraw(&cmds)) {
                self.withdrawn.push((k, c));
            }
        }
        self.withdrawn
            .retain(|(k, _)| !wanted.iter().any(|(w, _)| w == k));
        self.failing.retain(|k| !wanted.iter().any(|(w, _)| w == k));
        self.wanted = wanted;
    }

    pub fn take_withdrawn(&mut self) -> Vec<(Key, Cmd)> {
        std::mem::take(&mut self.withdrawn)
    }

    /// A clear herdr refused: tried again next tick unless the target is
    /// gone. True on its first failure, to log once.
    pub fn withdraw_failed(&mut self, key: Key, cmd: Cmd, gone: bool) -> bool {
        if gone {
            self.failing.remove(&key);
            return false;
        }
        let first = self.failing.insert(key.clone());
        self.withdrawn.push((key, cmd));
        first
    }

    pub fn withdrawn_ok(&mut self, key: &Key) {
        self.failing.remove(key);
    }

    /// The targets to send now: changed, never sent, or due for renewal.
    pub fn due(&self, now: Instant) -> Vec<(Key, Vec<Cmd>)> {
        self.wanted
            .iter()
            .filter(|(k, cmds)| match self.sent.get(k) {
                Some((last, at)) => last != cmds || now.duration_since(*at) >= RENEW_AFTER,
                None => true,
            })
            .cloned()
            .collect()
    }

    pub fn mark_sent(&mut self, key: Key, cmds: Vec<Cmd>, now: Instant) {
        self.sent.insert(key, (cmds, now));
    }

    /// herdr no longer knows the target: nothing more until a refresh names it.
    pub fn forget(&mut self, key: &Key) {
        self.wanted.retain(|(k, _)| k != key);
        self.sent.remove(key);
    }
}

/// Hands one refresh's commands to the sidebar thread (started on first use).
pub fn submit(cmds: Vec<Cmd>) {
    static TX: OnceLock<mpsc::Sender<Vec<Cmd>>> = OnceLock::new();
    let tx = TX.get_or_init(|| {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || run(rx));
        tx
    });
    let _ = tx.send(cmds);
}

fn run(rx: mpsc::Receiver<Vec<Cmd>>) {
    let mut state = State::default();
    loop {
        match rx.recv_timeout(TICK) {
            Ok(mut cmds) => {
                // Only the newest refresh counts.
                while let Ok(newer) = rx.try_recv() {
                    cmds = newer;
                }
                state.set(cmds);
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
        }
        let started = Instant::now();
        for (k, cmd) in state.take_withdrawn() {
            let refs: Vec<&str> = cmd.iter().map(String::as_str).collect();
            match vega_herdr::herdr_text(&refs) {
                Ok(_) => state.withdrawn_ok(&k),
                Err(f) => {
                    // A target that is gone needs no clearing.
                    let gone = f.code.ends_with("not_found");
                    if state.withdraw_failed(k.clone(), cmd, gone) {
                        crate::log(&format!(
                            "sidebar: clearing {} failed, retrying: {} {}",
                            k.1, f.code, f.message
                        ));
                    }
                }
            }
        }
        let due = state.due(started);
        let n = due.len();
        for (k, cmds) in due {
            let mut gone = false;
            for cmd in &cmds {
                let refs: Vec<&str> = cmd.iter().map(String::as_str).collect();
                // report-metadata prints nothing on success, not JSON.
                if let Err(f) = vega_herdr::herdr_text(&refs) {
                    let what = if k.0 == "workspace" {
                        "inbox"
                    } else {
                        "metadata"
                    };
                    crate::log(&format!(
                        "{what} failed for {}: {} {}",
                        k.1, f.code, f.message
                    ));
                    gone |= f.code.ends_with("not_found");
                }
            }
            if gone {
                state.forget(&k);
            } else {
                state.mark_sent(k, cmds, Instant::now());
            }
        }
        if started.elapsed() > crate::SLOW_STEP {
            crate::log(&format!(
                "slow: sidebar sent {n} rows in {} ms",
                started.elapsed().as_millis()
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cmd(kind: &str, id: &str, token: &str) -> Cmd {
        [kind, "report-metadata", id, "--token", token]
            .map(String::from)
            .to_vec()
    }

    fn send_all(s: &mut State, now: Instant) -> usize {
        let due = s.due(now);
        let n = due.len();
        for (k, c) in due {
            s.mark_sent(k, c, now);
        }
        n
    }

    #[test]
    fn unchanged_rows_wait_and_changed_ones_go_now() {
        let t0 = Instant::now();
        let mut s = State::default();
        s.set(vec![
            cmd("pane", "p1", "a=1"),
            cmd("workspace", "w1", "b=1"),
        ]);
        assert_eq!(send_all(&mut s, t0), 2);
        s.set(vec![
            cmd("pane", "p1", "a=1"),
            cmd("workspace", "w1", "b=2"),
        ]);
        let due = s.due(t0 + Duration::from_secs(15));
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].0, ("workspace".into(), "w1".into()));
    }

    #[test]
    fn labels_survive_a_two_minute_loop_stall() {
        let ttl = Duration::from_millis(TTL_MS.parse().unwrap());
        assert!(ttl >= Duration::from_secs(120) + RENEW_AFTER + TICK);
        let t0 = Instant::now();
        let mut s = State::default();
        s.set(vec![
            cmd("pane", "p1", "label=x"),
            cmd("pane", "p1", "git=✓"),
        ]);
        send_all(&mut s, t0);
        // The loop hands over nothing for 2 minutes; the thread ticks on.
        let mut last = t0;
        let mut now = t0;
        while now < t0 + Duration::from_secs(120) {
            now += TICK;
            if send_all(&mut s, now) > 0 {
                last = now;
            }
            assert!(now.duration_since(last) < ttl, "tokens expired at {now:?}");
            assert!(now.duration_since(last) <= RENEW_AFTER);
        }
        // Both commands of the pane go out together, in order.
        let (_, cmds) = &s.due(now + RENEW_AFTER)[0];
        assert_eq!(cmds.len(), 2);
        assert_eq!(cmds[1][4], "git=✓");
    }

    #[test]
    fn a_row_a_refresh_drops_is_cleared_at_once() {
        let t0 = Instant::now();
        let mut s = State::default();
        let mut p1 = [
            "pane",
            "report-metadata",
            "p1",
            "--source",
            "vega:vegad",
            "--ttl-ms",
            TTL_MS,
            "--display-agent",
            "x",
            "--token",
            "host=h",
            "--clear-token",
            "git",
        ]
        .map(String::from)
        .to_vec();
        s.set(vec![p1.clone(), cmd("pane", "p2", "a=1")]);
        send_all(&mut s, t0);
        assert!(s.take_withdrawn().is_empty());
        s.set(vec![cmd("pane", "p2", "a=1")]);
        let w = s.take_withdrawn();
        assert_eq!(w.len(), 1);
        assert_eq!(
            w[0].1.join(" "),
            "pane report-metadata p1 --source vega:vegad --clear-display-agent --clear-token host --clear-token git"
        );
        // Named again before the clear went out: no clear, a fresh send.
        s.set(vec![cmd("pane", "p2", "a=1")]);
        p1.truncate(9);
        s.set(vec![p1, cmd("pane", "p2", "a=1")]);
        assert!(s.take_withdrawn().is_empty());
        assert_eq!(s.due(t0).len(), 1);
    }

    #[test]
    fn a_failed_clear_is_retried_and_logged_once_until_it_lands() {
        let k: Key = ("pane".into(), "p1".into());
        let c = cmd("pane", "p1", "x");
        let mut s = State::default();
        assert!(s.withdraw_failed(k.clone(), c.clone(), false));
        let again = s.take_withdrawn();
        assert_eq!(again, vec![(k.clone(), c.clone())]);
        assert!(!s.withdraw_failed(k.clone(), c.clone(), false));
        s.take_withdrawn();
        s.withdrawn_ok(&k);
        assert!(s.withdraw_failed(k.clone(), c.clone(), false));
        s.take_withdrawn();
        // A gone target is not retried.
        assert!(!s.withdraw_failed(k.clone(), c, true));
        assert!(s.take_withdrawn().is_empty());
    }

    #[test]
    fn a_gone_or_dropped_target_is_not_sent_again() {
        let t0 = Instant::now();
        let mut s = State::default();
        s.set(vec![cmd("pane", "p1", "a=1"), cmd("pane", "p2", "a=1")]);
        send_all(&mut s, t0);
        s.forget(&("pane".into(), "p1".into()));
        s.set(vec![cmd("pane", "p2", "a=1")]);
        let due = s.due(t0 + RENEW_AFTER);
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].0 .1, "p2");
    }
}
