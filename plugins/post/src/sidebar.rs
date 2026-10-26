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
}

fn key(cmd: &Cmd) -> Key {
    let at = |i: usize| cmd.get(i).cloned().unwrap_or_default();
    (at(0), at(2))
}

impl State {
    /// A new refresh: its commands replace the last ones; targets it no
    /// longer names are forgotten.
    pub fn set(&mut self, cmds: Vec<Cmd>) {
        let mut wanted: Vec<(Key, Vec<Cmd>)> = Vec::new();
        for cmd in cmds {
            let k = key(&cmd);
            match wanted.iter_mut().find(|(w, _)| *w == k) {
                Some((_, v)) => v.push(cmd),
                None => wanted.push((k, vec![cmd])),
            }
        }
        self.sent.retain(|k, _| wanted.iter().any(|(w, _)| w == k));
        self.wanted = wanted;
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
