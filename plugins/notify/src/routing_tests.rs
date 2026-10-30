use super::*;
use std::cell::RefCell;
struct Fake {
    name: &'static str,
    class: Class,
    calls: RefCell<Vec<Why>>,
}
impl Fake {
    fn new(name: &'static str, class: Class) -> Self {
        Self {
            name,
            class,
            calls: RefCell::new(vec![]),
        }
    }
}
impl Channel for Fake {
    fn name(&self) -> &str {
        self.name
    }
    fn class(&self) -> Class {
        self.class
    }
    fn send(&self, ask: &Ask, why: Why) -> Result<Receipt, Failure> {
        self.calls.borrow_mut().push(why);
        Ok(receipt(ask))
    }
}
fn config() -> Config {
    let mut c = Config::parse(
        "[channels.ntfy]\nurl = 'https://ntfy.test'\n[channels.pushover]\nprojects = ['allowed']",
    )
    .unwrap();
    c.quiet = None;
    c
}
fn ask(level: &str) -> Ask {
    Ask {
        id: 1000000,
        host: "origin".into(),
        level: Some(level.into()),
        kind: "ask".into(),
        ..Default::default()
    }
}
fn tick(
    c: &Config,
    n: &Ask,
    age: i64,
    sent: &mut Sent,
    channels: &[&dyn Channel],
    view: &vega_registry::View,
    suppress: bool,
) {
    Round {
        host: "ganymede",
        local: false,
        toast_only: false,
        view,
        config: c,
        now: n.at() + age,
    }
    .deliver(
        std::slice::from_ref(n),
        channels,
        sent,
        |_| suppress,
        |_| Ok(()),
    )
    .unwrap();
}
fn view() -> vega_registry::View {
    vega_registry::View {
        entries: BTreeMap::from([("ganymede.beat".into(), serde_json::json!({"human_at":1000}))]),
        ..Default::default()
    }
}
#[test]
fn timing_table_every_row_boundaries_and_only_one_reminder() {
    for (level, expected) in [
        ("info", vec![]),
        ("action", vec![Why::Escalate, Why::Remind]),
        ("urgent", vec![Why::Raise, Why::Remind]),
    ] {
        let toast = Fake::new("toast", Class::Seat);
        let macos = Fake::new("macos", Class::Seat);
        let phone = Fake::new("ntfy", Class::Remote);
        let n = ask(level);
        let mut sent = Sent::default();
        for age in [0, 899, 900, 901, 3599, 3600, 3601, 86400] {
            tick(
                &config(),
                &n,
                age,
                &mut sent,
                &[&toast, &macos, &phone],
                &view(),
                false,
            );
            if age == 899 {
                assert_eq!(phone.calls.borrow().len(), usize::from(level == "urgent"));
            }
            if age == 900 {
                assert_eq!(
                    phone.calls.borrow().len(),
                    match level {
                        "info" => 0,
                        "action" => 1,
                        _ => 2,
                    }
                );
            }
        }
        assert_eq!(*phone.calls.borrow(), expected, "{level}");
        assert_eq!(*toast.calls.borrow(), vec![Why::Raise]);
        assert_eq!(macos.calls.borrow().len(), usize::from(level != "info"));
    }
}
#[test]
fn focus_does_not_stop_phone_and_ack_stops_all_later_steps() {
    let phone = Fake::new("ntfy", Class::Remote);
    let toast = Fake::new("toast", Class::Seat);
    let mut sent = Sent::default();
    let mut n = ask("action");
    tick(
        &config(),
        &n,
        900,
        &mut sent,
        &[&toast, &phone],
        &view(),
        true,
    );
    assert!(toast.calls.borrow().is_empty());
    assert_eq!(*phone.calls.borrow(), vec![Why::Escalate]);
    n.acked = Some(1901);
    tick(
        &config(),
        &n,
        3600,
        &mut sent,
        &[&toast, &phone],
        &view(),
        false,
    );
    n.acked = None;
    tick(
        &config(),
        &n,
        7200,
        &mut sent,
        &[&toast, &phone],
        &view(),
        false,
    );
    assert_eq!(phone.calls.borrow().len(), 1);
}
fn local_time(hour: u16, minute: u16) -> i64 {
    // Find a fake timestamp in the requested local minute without mutating process TZ.
    (0..86400)
        .step_by(60)
        .find(|at| {
            let c = Config {
                quiet: Some((hour * 60 + minute, hour * 60 + minute + 1)),
                ..Config::default()
            };
            c.is_quiet(*at)
        })
        .unwrap()
}
#[test]
fn quiet_boundaries_defer_and_coalesce_the_backlog_after_restart() {
    let mut c = config();
    c.quiet = Some((1350, 450));
    for (h, m, quiet) in [
        (22, 29, false),
        (22, 30, true),
        (0, 0, true),
        (7, 29, true),
        (7, 30, false),
    ] {
        assert_eq!(c.is_quiet(local_time(h, m)), quiet);
    }
    for level in ["action", "urgent"] {
        let phone = Fake::new("ntfy", Class::Remote);
        let now = local_time(23, 0) + 86400;
        let n = Ask {
            id: ((now - 3600) * 1000) as u64,
            ..ask(level)
        };
        let mut sent = Sent::default();
        tick(&c, &n, 3600, &mut sent, &[&phone], &view(), false);
        assert!(phone.calls.borrow().is_empty());
        let bytes = serde_json::to_vec(&sent).unwrap();
        sent = serde_json::from_slice(&bytes).unwrap();
        let morning = now + 8 * 3600 + 30 * 60;
        tick(
            &c,
            &n,
            morning - n.at(),
            &mut sent,
            &[&phone],
            &view(),
            false,
        );
        tick(
            &c,
            &n,
            morning - n.at() + 30,
            &mut sent,
            &[&phone],
            &view(),
            false,
        );
        assert_eq!(
            phone.calls.borrow().len(),
            1,
            "no reminder backlog: {level}"
        );
        let slept_phone = Fake::new("ntfy", Class::Remote);
        let slept_note = Ask {
            id: (now * 1000) as u64,
            ..n.clone()
        };
        let mut slept_sent = Sent::default();
        for at in [morning, morning + 30] {
            tick(
                &c,
                &slept_note,
                at - slept_note.at(),
                &mut slept_sent,
                &[&slept_phone],
                &view(),
                false,
            );
        }
        assert_eq!(
            slept_phone.calls.borrow().len(),
            1,
            "notifier slept through quiet hours: {level}"
        );
    }
}
#[test]
fn only_pagerduty_high_breaks_quiet_hours() {
    let mut c = config();
    c.quiet = Some((1350, 450));
    let now = local_time(23, 0) + 86400;
    for (source, urgency, count) in [
        (None, None, 0),
        (Some("pagerduty"), Some("low"), 0),
        (Some("agent"), Some("high"), 0),
        (Some("pagerduty"), Some("high"), 1),
    ] {
        let phone = Fake::new("ntfy", Class::Remote);
        let n = Ask {
            id: ((now - 900) * 1000) as u64,
            source: source.map(String::from),
            urgency: urgency.map(String::from),
            ..ask("urgent")
        };
        tick(&c, &n, 900, &mut Sent::default(), &[&phone], &view(), false);
        assert_eq!(phone.calls.borrow().len(), count);
    }
}
#[test]
fn project_remind_channel_allowlist_and_missing_config() {
    let mut c = config();
    c.projects.insert("allowed".into(), 7200);
    let mut n = ask("action");
    n.project = Some("allowed".into());
    let phone = Fake::new("ntfy", Class::Remote);
    let mut sent = Sent::default();
    for age in [900, 3600, 7199] {
        tick(&c, &n, age, &mut sent, &[&phone], &view(), false);
    }
    assert_eq!(phone.calls.borrow().len(), 1);
    tick(&c, &n, 7200, &mut sent, &[&phone], &view(), false);
    assert_eq!(phone.calls.borrow().len(), 2);
    assert!(c.allowed("pushover", Some("allowed")));
    assert!(!c.allowed("pushover", Some("private")));
    c.channels.get_mut("ntfy").unwrap().projects = Some(vec!["allowed".into()]);
    let private_phone = Fake::new("ntfy", Class::Remote);
    tick(
        &c,
        &Ask {
            project: Some("private".into()),
            ..ask("urgent")
        },
        0,
        &mut Sent::default(),
        &[&private_phone],
        &view(),
        false,
    );
    assert!(
        private_phone.calls.borrow().is_empty(),
        "project allowlist must apply before Channel.send"
    );
    let toast = Fake::new("toast", Class::Seat);
    let macos = Fake::new("macos", Class::Seat);
    let phone = Fake::new("ntfy", Class::Remote);
    let mut sent = Sent::default();
    for age in [0, 900, 3600, 86400] {
        tick(
            &Config::default(),
            &ask("urgent"),
            age,
            &mut sent,
            &[&toast, &macos, &phone],
            &view(),
            false,
        );
    }
    assert_eq!(toast.calls.borrow().len(), 1);
    assert!(macos.calls.borrow().is_empty());
    assert!(phone.calls.borrow().is_empty());
}
#[test]
fn config_reads_all_routing_fields_and_rejects_bad_values() {
    let c = Config::parse(
        r#"
quiet = "21:00-08:15"
[levels.action]
channels = ["macos"]
escalate = { after = "20m", channels = ["ntfy"] }
remind = "3h"
[projects."github.com/durandom/orrery"]
remind = "2h"
[channels.ntfy]
url = "https://example.test"
topic = "vega"
"#,
    )
    .unwrap();
    assert_eq!(c.quiet, Some((1260, 495)));
    assert_eq!(c.level("action").after, 1200);
    assert_eq!(c.level("action").remind, Some(10800));
    assert_eq!(c.projects["github.com/durandom/orrery"], 7200);
    assert_eq!(c.channels["ntfy"].settings["topic"].as_str(), Some("vega"));
    for bad in [
        "quiet='25:00-07:00'",
        "quiet='07:00-07:00'",
        "[levels.wrong]",
        "[levels.action]\nremind='0m'",
        "[channels.ntfy]\nprojects=3",
    ] {
        assert!(Config::parse(bad).is_err(), "{bad}");
    }
}
#[test]
fn uber_engaged_reroutes_each_phone_step_once_and_inactive_uses_phone() {
    let c = config();
    let n = ask("action");
    let phone = Fake::new("ntfy", Class::Remote);
    let uber = Fake::new("uber", Class::Remote);
    let mut v = view();
    v.entries.insert(
        vega_registry::agent_key("ganymede", "w9:p1"),
        serde_json::json!({"status":"idle"}),
    );
    let mut claim = vega_registry::uber_state::value("w9:p1", "ganymede", 0, 20);
    claim["engaged_at"] = serde_json::json!(1800);
    v.entries
        .insert(vega_registry::uber_key("ganymede"), claim.clone());
    v.entries.get_mut("ganymede.beat").unwrap()["at"] = serde_json::json!(1900);
    let mut sent = Sent::default();
    tick(&c, &n, 900, &mut sent, &[&phone, &uber], &v, false);
    assert_eq!(*uber.calls.borrow(), vec![Why::Escalate]);
    assert!(phone.calls.borrow().is_empty());
    // Ending engagement cannot redeliver that same escalation to the phone.
    claim["engaged_at"] = serde_json::json!(0);
    v.entries
        .insert(vega_registry::uber_key("ganymede"), claim.clone());
    tick(&c, &n, 930, &mut sent, &[&phone, &uber], &v, false);
    assert!(phone.calls.borrow().is_empty());
    // A later reminder follows the then-current route.
    tick(&c, &n, 3600, &mut sent, &[&phone, &uber], &v, false);
    assert_eq!(*phone.calls.borrow(), vec![Why::Remind]);
    claim["engaged_at"] = serde_json::json!(4600);
    v.entries.insert(vega_registry::uber_key("ganymede"), claim);
    v.entries.get_mut("ganymede.beat").unwrap()["at"] = serde_json::json!(4600);
    tick(&c, &n, 3600, &mut sent, &[&uber, &phone], &v, false);
    assert_eq!(
        uber.calls.borrow().len(),
        1,
        "route switch must not duplicate a sent reminder"
    );
    let text = uber_text(
        &Ask {
            title: "test line".into(),
            project: Some("demo".into()),
            pane: Some("w1:p1".into()),
            cmd: Some("echo test".into()),
            ..n
        },
        Why::Escalate,
    );
    for part in [
        "origin=1000000",
        "demo",
        "w1:p1",
        "test line",
        "echo test",
        "no decision or approval",
    ] {
        assert!(text.contains(part));
    }
}
#[test]
fn uber_adapter_uses_plain_mail_without_a_typed_kind_or_decision() {
    const CHILD: &str = "VEGA_NOTIFY_UBER_TEST";
    if std::env::var_os(CHILD).is_none() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("vega");
        std::fs::write(
            &bin,
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$VEGA_NOTIFY_ARGS\"\n",
        )
        .unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        let out = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "routing_tests::uber_adapter_uses_plain_mail_without_a_typed_kind_or_decision",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .env("PATH", dir.path())
            .env("VEGA_NOTIFY_ARGS", dir.path().join("args"))
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stdout)
        );
        let args = std::fs::read_to_string(dir.path().join("args")).unwrap();
        assert!(args.starts_with("post\nsend\n--uber\n--from-tool\nvega-notify\nNotification only"));
        assert!(!args.contains("--kind"));
        return;
    }
    Uber.send(&ask("action"), Why::Escalate).unwrap();
}
