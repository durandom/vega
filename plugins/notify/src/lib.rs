//! Notification delivery primitives shared by the CLI and vegad. Timing/routing is separate.
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

#[derive(Clone, Debug)]
pub struct Config {
    pub notifier: String,
    pub seat_idle: i64,
    pub desktop: bool,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            notifier: "ganymede".into(),
            seat_idle: 300,
            desktop: false,
        }
    }
}
impl Config {
    pub fn load() -> Result<Self, String> {
        let root = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
            .ok_or("no config directory")?;
        match std::fs::read_to_string(root.join("vega/notify.toml")) {
            Ok(s) => Self::parse(&s),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e.to_string()),
        }
    }
    pub fn parse(s: &str) -> Result<Self, String> {
        let doc = s
            .parse::<toml_edit::DocumentMut>()
            .map_err(|e| e.to_string())?;
        let mut config = Self {
            desktop: true,
            ..Self::default()
        };
        if let Some(v) = doc.get("notifier") {
            let h = v
                .as_str()
                .filter(|s| !s.is_empty())
                .ok_or("notifier must be a host name")?;
            if vega_herdr::state::host_token(h) != h {
                return Err("notifier must be a host token".into());
            }
            config.notifier = h.into();
        }
        if let Some(v) = doc.get("seat_idle") {
            let s = v.as_str().ok_or("seat_idle must be a duration, e.g. 5m")?;
            let at = s
                .char_indices()
                .last()
                .map(|(i, _)| i)
                .ok_or("empty seat_idle")?;
            let (n, unit) = s.split_at(at);
            let factor = match unit {
                "s" => 1,
                "m" => 60,
                "h" => 3600,
                _ => return Err("seat_idle must use s, m or h".into()),
            };
            config.seat_idle = n
                .parse::<i64>()
                .ok()
                .filter(|n| *n > 0)
                .and_then(|n| n.checked_mul(factor))
                .ok_or("invalid seat_idle")?;
        }
        Ok(config)
    }
}

/// Successful sends, keyed by qualified ask, channel and reason. Never record failures.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Sent {
    pub receipts: BTreeMap<String, Receipt>,
    pub acked: BTreeMap<String, i64>,
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
                // Baseline raise only. Configurable routing and timers belong to .226.3.
                if channel.name() == "macos"
                    && (!self.config.desktop || ask.level.as_deref() == Some("info"))
                {
                    continue;
                }
                if channel.class() == Class::Seat && suppress(ask) {
                    continue;
                }
                let key = format!("{id}/{}/raise", channel.name());
                let level = ask.level.as_deref().unwrap_or("info");
                if sent
                    .receipts
                    .get(&key)
                    .is_some_and(|r| rank(&r.level) >= rank(level))
                {
                    continue;
                }
                match channel.send(ask, Why::Raise) {
                    Ok(mut receipt) => {
                        receipt.at = self.now;
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
            config: &Config::default(),
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
