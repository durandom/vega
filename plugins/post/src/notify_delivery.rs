//! A worker separate from the mail reactor; one round every 30 seconds.
use std::sync::{
    atomic::{AtomicU8, Ordering},
    Arc,
};
use std::time::Duration;
use vega_herdr::{asks, notes::Store};
use vega_notify::{Channel, Config, Macos, Round, Sent, Toast, Uber};

/// 0: startup/quiesced, 1: local mode, 2: broker mode. Inbox-only stays toast-only.
pub fn spawn() -> Arc<AtomicU8> {
    let mode = Arc::new(AtomicU8::new(0));
    let current = mode.clone();
    std::thread::spawn(move || {
        let rt = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(rt) => rt,
            Err(e) => {
                crate::log(&format!("notify: {e}"));
                return;
            }
        };
        let mut last_error = None;
        loop {
            let mode = current.load(Ordering::Relaxed);
            if mode != 0 {
                let result = rt.block_on(async {
                    tokio::time::timeout(Duration::from_secs(25), tick(mode)).await
                });
                let error = match result {
                    Ok(Ok(())) => None,
                    Ok(Err(e)) => Some(e),
                    Err(_) => Some("delivery round timed out".into()),
                };
                if error != last_error {
                    if let Some(e) = &error {
                        crate::log(&format!("notify: {e}"));
                    }
                    last_error = error;
                }
            }
            std::thread::sleep(Duration::from_secs(30));
        }
    });
    mode
}

async fn tick(mode: u8) -> Result<(), String> {
    tick_channels(mode, &[&Toast, &Macos, &Uber]).await
}

async fn tick_channels(mode: u8, channels: &[&dyn Channel]) -> Result<(), String> {
    tick_channels_at(mode, channels, (vega_herdr::notes::now_ms() / 1000) as i64).await
}

async fn tick_channels_at(mode: u8, channels: &[&dyn Channel], now: i64) -> Result<(), String> {
    let host = vega_herdr::state::post_host();
    let sent_key = format!("{}.notify.sent", vega_registry::token(&host));
    let inbox_only = vega_herdr::state::is_inbox_only(&host);
    let local = mode == 1 || inbox_only;
    let config = Config::load()?;
    let path = Sent::path().ok_or("no receipt directory")?;
    let mut sent = Sent::load(&path)?;
    let store = Store::local().ok_or("no notes directory")?;
    let mut view = vega_registry::View::default();
    let mut remote = vec![];
    let mut broker = None;
    if !local {
        let client = vega_registry::connect("notify").await?;
        let kv = vega_registry::bucket(&client).await?;
        view = vega_registry::read(&client, &kv).await?;
        // Election must fail closed too: read() deliberately tolerates failed direct gets.
        for key in view
            .keys
            .clone()
            .into_iter()
            .filter(|k| k.ends_with(".beat"))
        {
            match kv.get(&key).await.map_err(|e| e.to_string())? {
                Some(bytes) => {
                    view.entries.insert(
                        key,
                        serde_json::from_slice(&bytes).map_err(|e| e.to_string())?,
                    );
                }
                None => {
                    view.entries.remove(&key);
                }
            }
        }
        crate::identity::guard(&view.entries)?;
        // Re-read receipts strictly: a partial registry read must never turn into a resend.
        // Beats identify hosts even before their first notify.sent key was published.
        let hosts: std::collections::BTreeSet<_> = view
            .keys
            .iter()
            .filter_map(|k| k.split_once('.').map(|(h, _)| h.to_string()))
            .collect();
        for h in hosts {
            if let Some(bytes) = kv
                .get(format!("{h}.notify.sent"))
                .await
                .map_err(|e| e.to_string())?
            {
                sent.merge(
                    serde_json::from_slice(&bytes)
                        .map_err(|e| format!("invalid {h} receipts: {e}"))?,
                );
            }
        }
        remote = vega_registry::read_notes(&kv, &view).await;
        // Publish imported and local-mode receipts before any new delivery.
        sent.save(&path)?;
        kv.put(
            sent_key.clone(),
            serde_json::to_vec(&sent).map_err(|e| e.to_string())?.into(),
        )
        .await
        .map_err(|e| e.to_string())?;
        broker = Some(kv);
    }
    let notes = asks::open(store.list(), remote, &host);
    let focus = vega_herdr::waiting::read().focus;
    // Local input can be newer than the last heartbeat; suppression must not
    // wait for metadata publication after the Slayer just focused the pane.
    let human_at = vega_registry::uber_engaged::human_file()
        .and_then(|p| vega_registry::uber_engaged::read_at(&p))
        .max(
            vega_notify::seat(&view)
                .filter(|(h, _)| *h == host)
                .map(|(_, at)| at),
        );
    let suppress = |n: &vega_herdr::notes::Note| {
        let draft = n.host == host
            && n.pane.as_deref().is_some_and(|p| {
                screen(p).is_some_and(|s| {
                    matches!(
                        crate::delivery::draft(&s),
                        crate::delivery::Draft::Text(_) | crate::delivery::Draft::Opaque
                    )
                })
            });
        vega_notify::suppressed(
            n,
            &host,
            focus.as_deref(),
            human_at,
            now,
            config.seat_idle,
            draft,
        )
    };
    let round = Round {
        host: &host,
        local,
        toast_only: inbox_only,
        view: &view,
        config: &config,
        now,
    };
    // Each successful send is durable locally and on the broker before the next send.
    let mut failures = vec![];
    for note in &notes {
        for channel in channels {
            let before = sent.clone();
            failures.extend(round.deliver(
                std::slice::from_ref(note),
                &[*channel],
                &mut sent,
                suppress,
                |s| s.save(&path),
            )?);
            if let Some(kv) = broker.as_ref().filter(|_| sent != before) {
                kv.put(
                    sent_key.clone(),
                    serde_json::to_vec(&sent).map_err(|e| e.to_string())?.into(),
                )
                .await
                .map_err(|e| e.to_string())?;
            }
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures.join("; "))
    }
}

fn screen(pane: &str) -> Option<String> {
    if !crate::valid_pane_id(pane) {
        return None;
    }
    let out = vega_herdr::output_within(
        std::process::Command::new("herdr").args([
            "pane", "read", pane, "--source", "visible", "--format", "ansi",
        ]),
        Duration::from_secs(3),
    )
    .ok()??;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    #[test]
    fn broker_worker_imports_before_send_checkpoints_each_send_and_fails_closed() {
        const CHILD: &str = "VEGA_NOTIFY_BROKER_TEST";
        if std::env::var_os(CHILD).is_none() {
            let dir = tempfile::tempdir().unwrap();
            let out = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "notify_delivery::tests::broker_worker_imports_before_send_checkpoints_each_send_and_fails_closed", "--nocapture"])
                .env(CHILD, "1").env("XDG_STATE_HOME", dir.path())
                .env("XDG_CONFIG_HOME", dir.path().join("config"))
                .env("VEGA_POST_HOST", "test-notify").env_remove("VEGA_NATS_NKEY")
                .output().unwrap();
            assert!(
                out.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
            return;
        }
        use serde_json::json;
        use vega_herdr::{asks, notes::Store};
        use vega_notify::{Ask, Channel, Class, Config, Receipt, Sent, Why};
        let dir = tempfile::tempdir().unwrap();
        let (_server, url) = crate::registry_read_test::start(dir.path(), "");
        std::env::set_var("VEGA_NATS_URL", &url);
        let root = crate::service::state_dir()
            .unwrap()
            .join("identity-cutover");
        std::fs::create_dir_all(&root).unwrap();
        let plans: Vec<_> = ["test-notify", "other"].into_iter().map(|host| json!({
            "schema":2,"host":host,"inbox_only":false,"mappings":[],"original":{},"writes":{},"remove":[],
            "registry":{"host":host,"backup":{},"projection":{}},"registry_reads":[],"owned":{}
        })).collect();
        std::fs::write(
            root.join("approved.json"),
            json!({"schema":2,"hosts":plans,"grants":{}}).to_string(),
        )
        .unwrap();
        struct Probe {
            url: String,
            calls: std::cell::Cell<usize>,
        }
        impl Channel for Probe {
            fn name(&self) -> &str {
                "toast"
            }
            fn class(&self) -> Class {
                Class::Seat
            }
            fn send(&self, _: &Ask, _: Why) -> Result<Receipt, vega_herdr::Failure> {
                let url = self.url.clone();
                let count = self.calls.get();
                // A separate connection/runtime observes the real broker while the
                // worker is inside its synchronous Channel callback.
                std::thread::spawn(move || {
                    tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .unwrap()
                        .block_on(async {
                            let c = async_nats::connect(url).await.unwrap();
                            let v = vega_registry::get_one(&c, "test-notify.notify.sent")
                                .await
                                .unwrap()
                                .unwrap();
                            assert!(
                                v["receipts"]["test-notify=1000/toast/raise"].is_object(),
                                "foreign receipt must publish before the first send"
                            );
                            if count > 0 {
                                let previous = format!("other={}/toast/raise", (count + 1) * 1000);
                                assert!(
                                    v["receipts"][previous].is_object(),
                                    "previous send must publish before the next send"
                                );
                            }
                        });
                })
                .join()
                .unwrap();
                self.calls.set(count + 1);
                Ok(Receipt {
                    at: 0,
                    level: "action".into(),
                })
            }
        }
        let probe = Probe {
            url: url.clone(),
            calls: std::cell::Cell::new(0),
        };
        tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
            let mut client = None;
            for _ in 0..50 {
                if let Ok(c) = async_nats::connect(&url).await { client = Some(c); break; }
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
            let client = client.unwrap();
            let kv = async_nats::jetstream::new(client.clone()).create_key_value(async_nats::jetstream::kv::Config {
                bucket:vega_registry::BUCKET.into(), history:1, ..Default::default()
            }).await.unwrap();
            let now = (vega_herdr::notes::now_ms()/1000) as i64;
            for (h, input) in [("test-notify", now), ("other", now-10)] {
                kv.put(format!("{h}.beat"), json!({"schema":2,"at":now,"human_at":input}).to_string().into()).await.unwrap();
            }
            let own = Ask { host:"test-notify".into(),kind:"ask".into(),level:Some("action".into()),..Default::default() };
            vega_herdr::notes::Store::local().unwrap().add(own, 1000).unwrap();
            for id in [2000, 3000] {
                let ask = Ask { host:"other".into(), id,kind:"ask".into(),level:Some("action".into()),..Default::default() };
                kv.put(format!("other.note.{id}"), ask.to_json().to_string().into()).await.unwrap();
            }
            let imported = json!({"receipts":{"test-notify=1000/toast/raise":{"at":now-10,"level":"action"}},"acked":{}});
            kv.put("other.notify.sent", imported.to_string().into()).await.unwrap();
            super::tick_channels(2, &[&probe]).await.unwrap();
            assert_eq!(probe.calls.get(), 2, "imported ask is never sent again");
            let published = vega_registry::get_one(&client, "test-notify.notify.sent").await.unwrap().unwrap();
            assert_eq!(published["receipts"].as_object().unwrap().len(), 3);
            assert_eq!(Sent::load(&Sent::path().unwrap()).unwrap().receipts.len(), 3);
            assert_eq!(vega_registry::get_one(&client, "other.notify.sent").await.unwrap(), Some(imported.clone()));
            // A fresh worker invocation loads the checkpoint; default adapters have nothing to send.
            super::tick(2).await.unwrap();
            kv.put("other.notify.sent", "corrupt".into()).await.unwrap();
            assert!(super::tick_channels(2, &[&probe]).await.unwrap_err().contains("invalid other receipts"));
            assert_eq!(probe.calls.get(), 2);
            kv.put("other.notify.sent", imported.to_string().into()).await.unwrap();
            kv.put("test-notify.beat", "corrupt".into()).await.unwrap();
            assert!(super::tick_channels(2, &[&probe]).await.is_err(), "a tolerant beat read could elect the wrong host");
            assert_eq!(probe.calls.get(), 2);
            // Exercise routing through the production broker worker, not only Round fixtures.
            kv.put("test-notify.beat", json!({"schema":2,"at":now,"human_at":now}).to_string().into()).await.unwrap();
            let config_root = std::path::PathBuf::from(std::env::var_os("XDG_CONFIG_HOME").unwrap()).join("vega");
            std::fs::create_dir_all(&config_root).unwrap();
            let reminder_now = now + 2700;
            let quiet = ["00:00-01:00","06:00-07:00","12:00-13:00","18:00-19:00"].into_iter().find(|q| {
                let c = Config::parse(&format!("quiet='{q}'")).unwrap();
                !c.is_quiet(now) && !c.is_quiet(reminder_now)
            }).unwrap();
            asks::change(&Store::local().unwrap(), "test-notify", now, true, |_| true).unwrap();
            for id in [2000, 3000] { kv.delete(format!("other.note.{id}")).await.unwrap(); }
            let base = ((now-900)*1000) as u64;
            Store::local().unwrap().add(Ask { host:"test-notify".into(),kind:"ask".into(),level:Some("action".into()),..Default::default() },base).unwrap();
            for id in [base+1,base+2] {
                let ask = Ask { host:"other".into(),id,kind:"ask".into(),level:Some("action".into()),..Default::default() };
                kv.put(format!("other.note.{id}"),ask.to_json().to_string().into()).await.unwrap();
            }
            std::fs::write(config_root.join("notify.toml"),format!("notifier='test-notify'\nquiet='{quiet}'\n[channels.ntfy]" )).unwrap();
            struct RouteProbe(&'static str, std::cell::RefCell<Vec<Why>>);
            impl Channel for RouteProbe {
                fn name(&self) -> &str { self.0 }
                fn class(&self) -> Class { Class::Remote }
                fn send(&self, a: &Ask, why: Why) -> Result<Receipt, vega_herdr::Failure> {
                    self.1.borrow_mut().push(why);
                    Ok(Receipt { at:0,level:a.level.clone().unwrap() })
                }
            }
            let phone = RouteProbe("ntfy",Default::default());
            let uber = RouteProbe("uber",Default::default());
            super::tick_channels_at(2,&[&phone,&uber],now).await.unwrap();
            assert_eq!(*phone.1.borrow(),vec![Why::Escalate;3]);
            let mut claim = vega_registry::uber_state::value("w9:p1","test-notify",0,20);
            claim["engaged_at"] = json!(reminder_now);
            kv.put("test-notify.beat", json!({"schema":2,"at":reminder_now,"human_at":now}).to_string().into()).await.unwrap();
            kv.put(vega_registry::uber_key("test-notify"),claim.to_string().into()).await.unwrap();
            kv.put(vega_registry::agent_key("test-notify","w9:p1"),json!({"status":"idle"}).to_string().into()).await.unwrap();
            super::tick_channels_at(2,&[&phone,&uber],reminder_now).await.unwrap();
            assert_eq!(*uber.1.borrow(),vec![Why::Remind;3]);
            assert_eq!(phone.1.borrow().len(),3,"engaged phone steps go to Uber");
            claim["engaged_at"] = json!(0);
            kv.put(vega_registry::uber_key("test-notify"),claim.to_string().into()).await.unwrap();
            super::tick_channels_at(2,&[&phone,&uber],reminder_now).await.unwrap();
            assert_eq!(phone.1.borrow().len(),3,"Uber receipts prevent a later phone repeat");
        });
    }

    #[test]
    fn local_worker_checkpoints_and_restarts_without_a_second_toast() {
        const CHILD: &str = "VEGA_NOTIFY_LOCAL_TEST";
        if std::env::var_os(CHILD).is_some() {
            let store = vega_herdr::notes::Store::local().unwrap();
            let root = std::path::PathBuf::from(std::env::var_os("XDG_STATE_HOME").unwrap());
            std::fs::write(root.join("calls"), "").unwrap();
            std::fs::write(
                root.join("screen"),
                "answer\n────────\n❯ typing a draft\n────────\n  status",
            )
            .unwrap();
            store
                .add(
                    vega_herdr::notes::Note {
                        host: "test-notify".into(),
                        kind: "ask".into(),
                        level: Some("action".into()),
                        title: "demo: test -> pane".into(),
                        pane: Some("w1:p1".into()),
                        ..Default::default()
                    },
                    1000,
                )
                .unwrap();
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            rt.block_on(super::tick(1)).unwrap();
            assert_eq!(
                std::fs::read_to_string(root.join("calls")).unwrap(),
                "",
                "a recognized draft suppresses delivery"
            );
            let path = vega_notify::Sent::path().unwrap();
            assert!(vega_notify::Sent::load(&path).unwrap().receipts.is_empty());
            std::fs::write(
                root.join("screen"),
                "answer\n────────\n❯ \n────────\n  status",
            )
            .unwrap();
            let focus = vega_herdr::waiting::Ledger {
                focus: Some("w1:p1".into()),
                ..Default::default()
            };
            vega_herdr::waiting::save(&vega_herdr::waiting::path().unwrap(), &focus).unwrap();
            let human = vega_registry::uber_engaged::human_file().unwrap();
            let now = (vega_herdr::notes::now_ms() / 1000) as i64;
            vega_registry::uber_engaged::write_at(&human, now).unwrap();
            rt.block_on(super::tick(1)).unwrap();
            assert_eq!(
                std::fs::read_to_string(root.join("calls")).unwrap(),
                "",
                "recent focus suppresses an empty input box"
            );
            assert!(vega_notify::Sent::load(&path).unwrap().receipts.is_empty());
            vega_registry::uber_engaged::write_at(&human, now - 301).unwrap();
            rt.block_on(super::tick(1)).unwrap();
            rt.block_on(super::tick(1)).unwrap();
            assert_eq!(vega_notify::Sent::load(&path).unwrap().receipts.len(), 1);
            assert_eq!(store.list().len(), 1, "delivery reuses the ask note");
            assert_eq!(
                std::fs::read_to_string(root.join("calls")).unwrap(),
                "toast\n"
            );
            std::fs::write(&path, "corrupt").unwrap();
            assert!(rt.block_on(super::tick(1)).is_err());
            assert_eq!(
                std::fs::read_to_string(root.join("calls")).unwrap(),
                "toast\n"
            );
            return;
        }
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let herdr = bin.join("herdr");
        std::fs::write(&herdr, "#!/bin/sh\nif [ \"$1\" = pane ]; then\n  /bin/cat \"$XDG_STATE_HOME/screen\"\n  exit 0\nfi\necho toast >> \"$XDG_STATE_HOME/calls\"\nprintf '%s\\n' '{\"result\":{\"shown\":true,\"reason\":\"shown\"}}'\n").unwrap();
        std::fs::set_permissions(&herdr, std::fs::Permissions::from_mode(0o755)).unwrap();
        let out = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "notify_delivery::tests::local_worker_checkpoints_and_restarts_without_a_second_toast", "--nocapture"])
            .env(CHILD, "1").env("XDG_STATE_HOME", dir.path())
            .env("XDG_CONFIG_HOME", dir.path().join("config"))
            .env("VEGA_POST_HOST", "test-notify").env("PATH", bin)
            .output().unwrap();
        assert!(
            out.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

#[cfg(test)]
mod routing_worker_tests {
    use super::*;
    #[test]
    fn configured_worker_routes_phone_retries_quiet_and_honors_ack() {
        const CHILD: &str = "VEGA_NOTIFY_ROUTE_TEST";
        if std::env::var_os(CHILD).is_none() {
            let dir = tempfile::tempdir().unwrap();
            let out = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "notify_delivery::routing_worker_tests::configured_worker_routes_phone_retries_quiet_and_honors_ack", "--nocapture"])
                .env(CHILD,"1").env("XDG_STATE_HOME",dir.path())
                .env("XDG_CONFIG_HOME",dir.path().join("config")).env("VEGA_POST_HOST","route-test")
                .output().unwrap();
            assert!(
                out.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
            return;
        }
        use vega_notify::{Ask, Class, Receipt, Why};
        struct Phone(std::cell::RefCell<Vec<Why>>);
        impl Channel for Phone {
            fn name(&self) -> &str {
                "ntfy"
            }
            fn class(&self) -> Class {
                Class::Remote
            }
            fn send(&self, ask: &Ask, why: Why) -> Result<Receipt, vega_herdr::Failure> {
                self.0.borrow_mut().push(why);
                Ok(Receipt {
                    at: 0,
                    level: ask.level.clone().unwrap(),
                })
            }
        }
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let phone = Phone(Default::default());
        let store = Store::local().unwrap();
        let n = store
            .add(
                Ask {
                    host: "route-test".into(),
                    kind: "ask".into(),
                    level: Some("action".into()),
                    ..Default::default()
                },
                1000000,
            )
            .unwrap();
        let root =
            std::path::PathBuf::from(std::env::var_os("XDG_CONFIG_HOME").unwrap()).join("vega");
        std::fs::create_dir_all(&root).unwrap();
        let file = root.join("notify.toml");
        // Obtain a quiet interval covering fake time 1900 independently of the machine TZ.
        let covering = ["00:00-12:00", "12:00-00:00"]
            .into_iter()
            .find(|q| {
                Config::parse(&format!("quiet='{q}'"))
                    .unwrap()
                    .is_quiet(1900)
            })
            .unwrap();
        std::fs::write(&file, format!("quiet='{covering}'\n[channels.ntfy]")).unwrap();
        rt.block_on(super::tick_channels_at(1, &[&phone], 1899))
            .unwrap();
        assert!(phone.0.borrow().is_empty());
        rt.block_on(super::tick_channels_at(1, &[&phone], 1900))
            .unwrap();
        assert!(phone.0.borrow().is_empty());
        assert_eq!(
            Sent::load(&Sent::path().unwrap()).unwrap().deferred.len(),
            1
        );
        // Reload config on the next tick, then verify the successful receipt survives restart.
        let other = if covering == "00:00-12:00" {
            "12:00-00:00"
        } else {
            "00:00-12:00"
        };
        std::fs::write(&file, format!("quiet='{other}'\n[channels.ntfy]")).unwrap();
        rt.block_on(super::tick_channels_at(1, &[&phone], 1900))
            .unwrap();
        rt.block_on(super::tick_channels_at(1, &[&phone], 1900))
            .unwrap();
        assert_eq!(*phone.0.borrow(), vec![Why::Escalate]);
        vega_herdr::asks::change(&store, "route-test", 2000, true, |a| a.id == n.id).unwrap();
        rt.block_on(super::tick_channels_at(1, &[&phone], 4600))
            .unwrap();
        assert_eq!(phone.0.borrow().len(), 1);
        std::fs::write(&file, format!("quiet='{covering}'\n[channels.ntfy]")).unwrap();
        for (id, urgency) in [(1001000, "low"), (1002000, "high")] {
            store
                .add(
                    Ask {
                        host: "route-test".into(),
                        kind: "ask".into(),
                        level: Some("urgent".into()),
                        source: Some("pagerduty".into()),
                        urgency: Some(urgency.into()),
                        ..Default::default()
                    },
                    id,
                )
                .unwrap();
        }
        rt.block_on(super::tick_channels_at(1, &[&phone], 1900))
            .unwrap();
        assert_eq!(
            *phone.0.borrow(),
            vec![Why::Escalate, Why::Raise],
            "only persisted PagerDuty high breaks quiet"
        );
    }
}
