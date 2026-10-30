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
    let host = vega_herdr::state::post_host();
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
            format!("{host}.notify.sent"),
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
    let channels: [&dyn Channel; 2] = [&Toast, &Macos];
    let mut failures = vec![];
    for note in &notes {
        for channel in &channels {
            failures.extend(round.deliver(
                std::slice::from_ref(note),
                &[*channel],
                &mut sent,
                suppress,
                |s| s.save(&path),
            )?);
            if let Some(kv) = &broker {
                kv.put(
                    format!("{host}.notify.sent"),
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
    fn local_worker_checkpoints_and_restarts_without_a_second_toast() {
        const CHILD: &str = "VEGA_NOTIFY_LOCAL_TEST";
        if std::env::var_os(CHILD).is_some() {
            let store = vega_herdr::notes::Store::local().unwrap();
            store
                .add(
                    vega_herdr::notes::Note {
                        host: "test-notify".into(),
                        kind: "ask".into(),
                        level: Some("action".into()),
                        title: "demo: test -> pane".into(),
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
            rt.block_on(super::tick(1)).unwrap();
            let path = vega_notify::Sent::path().unwrap();
            assert_eq!(vega_notify::Sent::load(&path).unwrap().receipts.len(), 1);
            assert_eq!(store.list().len(), 1, "delivery reuses the ask note");
            let root = std::path::PathBuf::from(std::env::var_os("XDG_STATE_HOME").unwrap());
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
        std::fs::write(&herdr, "#!/bin/sh\necho toast >> \"$XDG_STATE_HOME/calls\"\nprintf '%s\\n' '{\"result\":{\"shown\":true,\"reason\":\"shown\"}}'\n").unwrap();
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
