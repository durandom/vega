//! A worker separate from the mail reactor; one round every 30 seconds.
use std::sync::{
    atomic::{AtomicU8, Ordering},
    Arc,
};
use std::time::Duration;
use vega_herdr::{asks, notes::Store};
use vega_notify::{Channel, Config, Macos, Round, Sent, Toast};

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
    tick_channels(mode, &[&Toast, &Macos]).await
}

async fn tick_channels(mode: u8, channels: &[&dyn Channel]) -> Result<(), String> {
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
    let now = (vega_herdr::notes::now_ms() / 1000) as i64;
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
        use vega_notify::{Ask, Channel, Class, Receipt, Sent, Why};
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
