//! Notification routing and durable delivery primitives shared by the CLI and vegad.
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    time::Duration,
};
use vega_herdr::{notes::Note, Failure};

pub type Ask = Note;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class {
    Seat,
    Remote,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Why {
    Raise,
    Escalate,
    Remind,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Receipt {
    pub at: i64,
    pub level: String,
}
pub trait Channel {
    fn name(&self) -> &str;
    fn class(&self) -> Class;
    fn send(&self, ask: &Ask, why: Why) -> Result<Receipt, Failure>;
    fn acked(&self, _receipt: &Receipt) -> Option<bool> {
        None
    }
    fn resolve(&self, _receipt: &Receipt) -> Result<(), Failure> {
        Ok(())
    }
}

pub struct Toast;
impl Channel for Toast {
    fn name(&self) -> &str {
        "toast"
    }
    fn class(&self) -> Class {
        Class::Seat
    }
    fn send(&self, ask: &Ask, _why: Why) -> Result<Receipt, Failure> {
        vega_herdr::notes::toast(&ask.title, &body(ask))
            .map_err(|e| unavailable("toast", &e.message))?;
        Ok(receipt(ask))
    }
}
pub struct Macos;
impl Channel for Macos {
    fn name(&self) -> &str {
        "macos"
    }
    fn class(&self) -> Class {
        Class::Seat
    }
    fn send(&self, ask: &Ask, _why: Why) -> Result<Receipt, Failure> {
        let mut cmd = desktop_command(&ask.title, &body(ask));
        let out = vega_herdr::output_within(&mut cmd, Duration::from_secs(5))
            .map_err(|e| unavailable("macos", &e.to_string()))?
            .ok_or_else(|| unavailable("macos", "desktop notification timed out"))?;
        if !out.status.success() {
            return Err(unavailable("macos", "desktop notification failed"));
        }
        Ok(receipt(ask))
    }
}
fn receipt(ask: &Ask) -> Receipt {
    Receipt {
        at: (vega_herdr::notes::now_ms() / 1000) as i64,
        level: ask.level.clone().unwrap_or_else(|| "info".into()),
    }
}
fn body(ask: &Ask) -> String {
    [ask.cmd.as_deref(), ask.bead.as_deref()]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join("\n")
}
fn apple_string(s: &str) -> String {
    format!(
        "\"{}\"",
        s.replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('\n', "\\n")
            .replace('\r', "\\r")
    )
}
fn desktop_command(title: &str, body: &str) -> std::process::Command {
    if cfg!(target_os = "macos") {
        let mut cmd = std::process::Command::new("osascript");
        cmd.args([
            "-e",
            &format!(
                "display notification {} with title {}",
                apple_string(body),
                apple_string(title)
            ),
        ]);
        cmd
    } else {
        let mut cmd = std::process::Command::new("notify-send");
        cmd.args(["--", title, body]);
        cmd
    }
}
fn unavailable(channel: &str, why: &str) -> Failure {
    Failure::new(
        "channel_unavailable",
        format!("{channel}: {why}"),
        "check the desktop session; vega notify test <channel>",
    )
}
pub fn test(channel: &str) -> Result<(), Failure> {
    let channel: Box<dyn Channel> = match channel {
        "toast" => Box::new(Toast),
        "macos" => Box::new(Macos),
        other => {
            return Err(Failure::new(
                "channel_unavailable",
                format!("{other}: channel support is not installed"),
                "phone channels land in a later build; vega notify test toast|macos",
            ))
        }
    };
    channel.send(
        &Ask {
            title: "vega notify: test notification".into(),
            level: Some("info".into()),
            ..Default::default()
        },
        Why::Raise,
    )?;
    Ok(())
}

mod routing;
pub use routing::{ChannelConfig, Config, Level};

/// Delivery ledger: successful sends by ask/channel/reason, durable acks and quiet deferrals.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Sent {
    pub receipts: BTreeMap<String, Receipt>,
    pub acked: BTreeMap<String, i64>,
    #[serde(default)]
    pub deferred: BTreeMap<String, i64>,
}
impl Sent {
    pub fn path() -> Option<PathBuf> {
        Some(
            vega_herdr::notes::Store::local()?
                .dir
                .parent()?
                .join("notify/sent.json"),
        )
    }
    pub fn load(path: &Path) -> Result<Self, String> {
        match std::fs::read(path) {
            Ok(b) => serde_json::from_slice(&b).map_err(|e| e.to_string()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e.to_string()),
        }
    }
    pub fn save(&self, path: &Path) -> Result<(), String> {
        let dir = path.parent().ok_or("no receipt directory")?;
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        let mut tmp = tempfile::NamedTempFile::new_in(dir).map_err(|e| e.to_string())?;
        use std::io::Write;
        tmp.write_all(&serde_json::to_vec(self).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
        tmp.as_file().sync_all().map_err(|e| e.to_string())?;
        tmp.persist(path).map_err(|e| e.to_string())?;
        Ok(())
    }
    pub fn merge(&mut self, other: Self) {
        for (k, v) in other.receipts {
            if self
                .receipts
                .get(&k)
                .is_none_or(|r| rank(&r.level) < rank(&v.level))
            {
                self.receipts.insert(k, v);
            }
        }
        for (k, at) in other.deferred {
            self.deferred
                .entry(k)
                .and_modify(|a| *a = (*a).min(at))
                .or_insert(at);
        }
        for (k, at) in other.acked {
            self.acked
                .entry(k)
                .and_modify(|a| *a = (*a).max(at))
                .or_insert(at);
        }
    }
}
fn rank(s: &str) -> u8 {
    match s {
        "urgent" => 2,
        "action" => 1,
        _ => 0,
    }
}

/// Deterministic election. Stale seats retain ownership: no failover; ties go to lower host.
pub fn seat(view: &vega_registry::View) -> Option<(&str, i64)> {
    view.entries
        .iter()
        .filter_map(|(key, v)| {
            let host = key.strip_suffix(".beat")?;
            Some((host, v["human_at"].as_i64()?))
        })
        .max_by(|(a, at), (b, bt)| at.cmp(bt).then_with(|| b.cmp(a)))
}
pub fn notifier_online(view: &vega_registry::View, config: &Config, now: i64) -> bool {
    view.beat_at(&config.notifier)
        .is_some_and(|at| now.saturating_sub(at) <= vega_registry::FRESH.as_secs() as i64)
}
#[allow(clippy::too_many_arguments)]
pub fn suppressed(
    ask: &Ask,
    own: &str,
    focus: Option<&str>,
    human_at: Option<i64>,
    now: i64,
    idle: i64,
    draft: bool,
) -> bool {
    ask.host == own
        && (draft
            || (ask.pane.as_deref().is_some_and(|p| Some(p) == focus)
                && human_at.is_some_and(|at| now.saturating_sub(at) < idle)))
}

pub struct Round<'a> {
    pub host: &'a str,
    pub local: bool,
    pub toast_only: bool,
    pub view: &'a vega_registry::View,
    pub config: &'a Config,
    pub now: i64,
}
impl Round<'_> {
    /// Per-ask/channel checkpoint, before moving on to another send. Callers publish it too.
    pub fn deliver(
        &self,
        asks: &[Ask],
        channels: &[&dyn Channel],
        sent: &mut Sent,
        suppress: impl Fn(&Ask) -> bool,
        mut checkpoint: impl FnMut(&Sent) -> Result<(), String>,
    ) -> Result<Vec<String>, String> {
        let mut failures = vec![];
        for ask in asks.iter().filter(|a| {
            a.kind == "ask" && a.resolved.is_none() && (!self.local || a.host == self.host)
        }) {
            let id = format!("{}={}", ask.host, ask.id);
            if let Some(at) = ask.acked {
                sent.acked.insert(id.clone(), at);
            }
            if ask.acked.is_some() || sent.acked.contains_key(&id) {
                continue;
            }
            for channel in channels {
                let owner = self.local
                    || match channel.class() {
                        Class::Seat => seat(self.view).is_some_and(|(h, _)| h == self.host),
                        Class::Remote => self.config.notifier == self.host,
                    };
                if !owner || (self.toast_only && channel.name() != "toast") {
                    continue;
                }
                let level = ask.level.as_deref().unwrap_or("info");
                let policy = self.config.level(level);
                let age = self.now.saturating_sub(ask.at());
                let remind = ask
                    .project
                    .as_ref()
                    .and_then(|p| self.config.projects.get(p))
                    .copied()
                    .or(policy.remind);
                let engaged = !self.local
                    && self
                        .view
                        .live_uber_at(self.now)
                        .is_some_and(|c| self.view.uber_engaged(&c, self.now).on());
                let remote = channel.class() == Class::Remote;
                let is_uber = channel.name() == "uber";
                if !self.config.allowed(channel.name(), ask.project.as_deref()) {
                    continue;
                }
                if remote && is_uber != engaged {
                    continue;
                }
                if !remote && suppress(ask) {
                    continue;
                }
                let initial = policy.channels.iter().any(|n| n == channel.name());
                let escalation = policy.escalate.iter().any(|n| n == channel.name());
                // Uber stands in for any configured phone channel, never a seat channel.
                let phone_initial = policy.channels.iter().any(|n| {
                    !matches!(n.as_str(), "toast" | "macos")
                        && self.config.allowed(n, ask.project.as_deref())
                });
                let phone_escalate = policy.escalate.iter().any(|n| {
                    !matches!(n.as_str(), "toast" | "macos")
                        && self.config.allowed(n, ask.project.as_deref())
                });
                let initial = initial || (is_uber && phone_initial);
                let escalation = escalation || (is_uber && phone_escalate);
                let covered = |why: &str, sent: &Sent| {
                    let own = format!("{id}/{}/{why}", channel.name());
                    let uber = format!("{id}/uber/{why}");
                    let valid = |r: &Receipt| rank(&r.level) >= rank(level);
                    sent.receipts.get(&own).is_some_and(valid)
                        || (remote && sent.receipts.get(&uber).is_some_and(valid))
                        || (is_uber
                            && sent.receipts.iter().any(|(k, r)| {
                                k.starts_with(&format!("{id}/"))
                                    && k.ends_with(&format!("/{why}"))
                                    && !k.contains("/toast/")
                                    && !k.contains("/macos/")
                                    && valid(r)
                            }))
                };
                let phase = if initial && !covered("raise", sent) {
                    Some((Why::Raise, "raise"))
                } else if escalation && age >= policy.after && !covered("escalate", sent) {
                    Some((Why::Escalate, "escalate"))
                } else if remote
                    && (initial || escalation)
                    && remind.is_some_and(|t| age >= t)
                    && !covered("remind", sent)
                {
                    Some((Why::Remind, "remind"))
                } else {
                    None
                };
                let Some((why, reason)) = phase else {
                    continue;
                };
                let deferred = format!("{id}/{}/deferred", channel.name());
                if remote
                    && self.config.is_quiet(self.now)
                    && !(ask.source.as_deref() == Some("pagerduty")
                        && ask.urgency.as_deref() == Some("high"))
                {
                    sent.deferred.entry(deferred).or_insert(self.now);
                    checkpoint(sent)?;
                    continue;
                }
                let key = format!("{id}/{}/{reason}", channel.name());
                match channel.send(ask, why) {
                    Ok(mut receipt) => {
                        receipt.at = self.now;
                        // A quiet-hour backlog collapses to one phone send; consume the
                        // reminder as well when its deadline already passed.
                        let deadline = ask.at().saturating_add(match why {
                            Why::Raise => 0,
                            Why::Escalate => policy.after,
                            Why::Remind => remind.unwrap_or(0),
                        });
                        // A notifier that slept through the night has no deferral marker.
                        let due_during_quiet = self.config.is_quiet(deadline)
                            && !(ask.source.as_deref() == Some("pagerduty")
                                && ask.urgency.as_deref() == Some("high"));
                        let was_deferred = due_during_quiet
                            || sent.deferred.keys().any(|k| {
                                k.starts_with(&format!("{id}/")) && k.ends_with("/deferred")
                            });
                        if remote && was_deferred && remind.is_some_and(|t| age >= t) {
                            sent.receipts
                                .insert(format!("{id}/{}/remind", channel.name()), receipt.clone());
                        }
                        sent.receipts.insert(key, receipt);
                        checkpoint(sent)?;
                    }
                    Err(e) => failures.push(format!("{}: {}", channel.name(), e.message)),
                }
            }
        }
        checkpoint(sent)?;
        Ok(failures)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::cell::Cell;
    struct Fake {
        calls: Cell<usize>,
        class: Class,
        fail: Cell<bool>,
    }
    impl Channel for Fake {
        fn name(&self) -> &str {
            if self.class == Class::Seat {
                "toast"
            } else {
                "phone"
            }
        }
        fn class(&self) -> Class {
            self.class
        }
        fn send(&self, ask: &Ask, _why: Why) -> Result<Receipt, Failure> {
            self.calls.set(self.calls.get() + 1);
            if self.fail.get() {
                Err(unavailable(self.name(), "busy"))
            } else {
                Ok(receipt(ask))
            }
        }
    }
    fn channel(class: Class) -> Fake {
        Fake {
            calls: Cell::new(0),
            class,
            fail: Cell::new(false),
        }
    }
    fn ask() -> Ask {
        Ask {
            host: "origin".into(),
            id: 1000,
            kind: "ask".into(),
            level: Some("action".into()),
            pane: Some("w1:p1".into()),
            ..Default::default()
        }
    }
    fn view(a: i64, b: i64) -> vega_registry::View {
        vega_registry::View {
            entries: BTreeMap::from([
                ("a.beat".into(), json!({"at":100,"human_at":a})),
                ("b.beat".into(), json!({"at":100,"human_at":b})),
            ]),
            ..Default::default()
        }
    }
    #[allow(clippy::too_many_arguments)]
    fn run(
        host: &str,
        view: &vega_registry::View,
        local: bool,
        sent: &mut Sent,
        channel: &dyn Channel,
        asks: &[Ask],
        now: i64,
        suppress: bool,
    ) -> Vec<String> {
        Round {
            host,
            local,
            toast_only: false,
            view,
            config: &if channel.class() == Class::Remote {
                let mut c =
                    Config::parse("[channels.phone]\n[levels.action]\nchannels = ['phone']")
                        .unwrap();
                c.quiet = None;
                c
            } else {
                Config::default()
            },
            now,
        }
        .deliver(asks, &[channel], sent, |_| suppress, |_| Ok(()))
        .unwrap()
    }
    #[test]
    fn seat_handoff_restart_and_two_hosts_do_not_repeat() {
        let clock = 100;
        let c = channel(Class::Seat);
        let mut a = Sent::default();
        let mut b = Sent::default();
        run(
            "b",
            &view(90, 80),
            false,
            &mut b,
            &c,
            &[ask()],
            clock,
            false,
        );
        assert_eq!(c.calls.get(), 0);
        run(
            "a",
            &view(90, 80),
            false,
            &mut a,
            &c,
            &[ask()],
            clock,
            false,
        );
        assert_eq!(c.calls.get(), 1);
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("sent.json");
        a.save(&file).unwrap();
        let mut restart = Sent::load(&file).unwrap();
        run(
            "a",
            &view(90, 80),
            false,
            &mut restart,
            &c,
            &[ask()],
            clock + 30,
            false,
        );
        b.merge(restart);
        run(
            "b",
            &view(90, 140),
            false,
            &mut b,
            &c,
            &[ask()],
            clock + 60,
            false,
        );
        run(
            "a",
            &view(90, 140),
            false,
            &mut a,
            &c,
            &[ask()],
            clock + 60,
            false,
        );
        assert_eq!(c.calls.get(), 1);
        assert_eq!(seat(&view(90, 90)).unwrap().0, "a");
        let mut next = ask();
        next.id += 1;
        run(
            "b",
            &view(90, 140),
            false,
            &mut b,
            &c,
            &[next],
            clock + 90,
            false,
        );
        assert_eq!(c.calls.get(), 2);
    }
    #[test]
    fn suppression_retries_and_failed_channels_are_never_receipts() {
        let c = channel(Class::Seat);
        let mut sent = Sent::default();
        run(
            "a",
            &view(90, 80),
            false,
            &mut sent,
            &c,
            &[ask()],
            100,
            true,
        );
        assert_eq!(c.calls.get(), 0);
        c.fail.set(true);
        assert_eq!(
            run(
                "a",
                &view(90, 80),
                false,
                &mut sent,
                &c,
                &[ask()],
                130,
                false
            )
            .len(),
            1
        );
        assert!(sent.receipts.is_empty());
        c.fail.set(false);
        run(
            "a",
            &view(90, 80),
            false,
            &mut sent,
            &c,
            &[ask()],
            160,
            false,
        );
        assert_eq!(sent.receipts.values().next().unwrap().at, 160);
        let n = Ask {
            host: "a".into(),
            ..ask()
        };
        assert!(suppressed(
            &n,
            "a",
            Some("w1:p1"),
            Some(90),
            100,
            300,
            false
        ));
        assert!(!suppressed(
            &n,
            "a",
            Some("w1:p1"),
            Some(90),
            390,
            300,
            false
        ));
        assert!(suppressed(&n, "a", None, Some(90), 500, 300, true));
        assert!(!suppressed(
            &n,
            "b",
            Some("w1:p1"),
            Some(90),
            100,
            300,
            true
        ));
    }
    #[test]
    fn local_and_offline_notifier_have_no_failover() {
        let c = channel(Class::Remote);
        let mut sent = Sent::default();
        let v = view(90, 80);
        assert!(!notifier_online(&v, &Config::default(), 100));
        run("a", &v, false, &mut sent, &c, &[ask()], 100, false);
        assert_eq!(c.calls.get(), 0);
        run("a", &v, true, &mut sent, &c, &[ask()], 130, false);
        assert_eq!(c.calls.get(), 0, "local mode ignores foreign asks");
        let own = Ask {
            host: "a".into(),
            ..ask()
        };
        run("a", &v, true, &mut sent, &c, &[own], 160, false);
        assert_eq!(c.calls.get(), 1);
        let config = Config {
            notifier: "b".into(),
            ..Config::default()
        };
        assert!(notifier_online(&v, &config, 100));
        assert!(!notifier_online(
            &v,
            &config,
            100 + vega_registry::FRESH.as_secs() as i64 + 1
        ));
    }
    #[test]
    fn ack_resolution_level_increase_and_checkpoint_failure() {
        let c = channel(Class::Seat);
        let mut sent = Sent::default();
        let mut n = ask();
        n.acked = Some(100);
        run("a", &view(90, 80), false, &mut sent, &c, &[n], 100, false);
        run(
            "a",
            &view(90, 80),
            false,
            &mut sent,
            &c,
            &[ask()],
            130,
            false,
        );
        assert_eq!(
            c.calls.get(),
            0,
            "imported ack survives stale origin snapshot"
        );
        sent = Sent::default();
        let mut n = ask();
        n.resolved = Some(100);
        run("a", &view(90, 80), false, &mut sent, &c, &[n], 100, false);
        run(
            "a",
            &view(90, 80),
            false,
            &mut sent,
            &c,
            &[ask()],
            130,
            false,
        );
        let mut n = ask();
        n.level = Some("urgent".into());
        run(
            "a",
            &view(90, 80),
            false,
            &mut sent,
            &c,
            &[n.clone()],
            160,
            false,
        );
        assert_eq!(c.calls.get(), 2);
        let old: Sent = serde_json::from_value(
            json!({"receipts":{"origin=1000/toast/raise":{"at":100,"level":"action"}},"acked":{}}),
        )
        .unwrap();
        sent.merge(old);
        run("a", &view(90, 80), false, &mut sent, &c, &[n], 190, false);
        assert_eq!(c.calls.get(), 2);
        let round = Round {
            host: "a",
            local: true,
            toast_only: false,
            view: &vega_registry::View::default(),
            config: &Config::default(),
            now: 200,
        };
        let mut n = ask();
        n.id += 1;
        n.host = "a".into();
        assert!(round
            .deliver(
                &[n],
                &[&c],
                &mut sent,
                |_| false,
                |_| Err("disk full".into())
            )
            .is_err());
    }
    #[test]
    fn config_and_applescript_literals() {
        assert_eq!(Config::default().notifier, "ganymede");
        assert!(!Config::default().desktop);
        let c = Config::parse("notifier = 'titan'\nseat_idle = '2m'").unwrap();
        assert_eq!(
            (c.notifier.as_str(), c.seat_idle, c.desktop),
            ("titan", 120, true)
        );
        for s in [
            "seat_idle = '0m'",
            "seat_idle = '💥'",
            "seat_idle = 3",
            "notifier = ''",
        ] {
            assert!(Config::parse(s).is_err());
        }
        assert_eq!(apple_string("a\"\\\nb"), "\"a\\\"\\\\\\nb\"");
    }
}

/// Notification-only mail: it cannot enter the Uber decision/approval protocol.
pub struct Uber;
impl Channel for Uber {
    fn name(&self) -> &str {
        "uber"
    }
    fn class(&self) -> Class {
        Class::Remote
    }
    fn send(&self, ask: &Ask, why: Why) -> Result<Receipt, Failure> {
        let text = uber_text(ask, why);
        let out = vega_herdr::output_within(
            std::process::Command::new("vega").args([
                "post",
                "send",
                "--uber",
                "--from-tool",
                "vega-notify",
                &text,
            ]),
            Duration::from_secs(10),
        )
        .map_err(|e| unavailable("uber", &e.to_string()))?
        .ok_or_else(|| unavailable("uber", "notification mail timed out"))?;
        if !out.status.success() {
            return Err(unavailable("uber", "notification mail failed"));
        }
        Ok(receipt(ask))
    }
}
pub fn uber_text(ask: &Ask, why: Why) -> String {
    format!("Notification only (no decision or approval): {}={} · {:?}\nproject: {}\npane: {}\n{}\ncmd: {}\nReturn to the originating pane for the question; this mail provides no answer path.",
        ask.host, ask.id, why, ask.project.as_deref().unwrap_or("-"),
        ask.pane.as_deref().unwrap_or("-"), ask.title, ask.cmd.as_deref().unwrap_or("-"))
}

#[cfg(test)]
mod routing_tests;
