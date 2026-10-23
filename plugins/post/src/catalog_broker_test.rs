//! The lanista catalog against a real broker (vega-lhc.136): the admin
//! creates the bucket, the writer puts the index, every host (the inbox-only
//! one too) reads it by direct get alone, and nobody else can write or make
//! a consumer.

use async_nats::jetstream;
use futures_util::StreamExt;
use vega_herdr::catalog::{self as cat, State};

use crate::auth::{self, Entry};
use crate::catalog;
use crate::registry_read_test::{connect, denied, start, user};

fn index(generated: &str, models: usize) -> Vec<u8> {
    // Several 128 KiB chunks, so the next_by_subj walk is exercised.
    let models: serde_json::Map<String, serde_json::Value> = (0..models)
        .map(|i| {
            (
                format!("model-{i}"),
                serde_json::json!({"note": "x".repeat(200)}),
            )
        })
        .collect();
    serde_json::to_vec(&serde_json::json!({"generated_at": generated, "models": models})).unwrap()
}

#[test]
fn catalog_is_put_by_the_writer_and_read_by_direct_get() {
    let dir = tempfile::tempdir().unwrap();
    let (admin_seed, admin) = user();
    let (proteus_seed, proteus) = user();
    let (ganymede_seed, ganymede) = user();
    let (deimos_seed, deimos) = user();
    let entries = vec![
        Entry {
            host: "proteus".into(),
            key: proteus,
        },
        Entry {
            host: "ganymede".into(),
            key: ganymede,
        },
        Entry {
            host: "deimos".into(),
            key: deimos,
        },
    ];
    let authz = auth::render(
        &entries,
        Some(&admin),
        &["lanista".into()],
        &["deimos".into()],
        None,
        &[],
        &[],
        &[],
        Some("ganymede"),
    );
    let (_server, url) = start(dir.path(), &authz);

    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            // As `vega post provision lanista` does.
            let a = connect(&url, &admin_seed, crate::ADMIN_INBOX).await;
            jetstream::new(a)
                .create_object_store(catalog::bucket_config())
                .await
                .unwrap();

            let p = connect(&url, &proteus_seed, "_INBOX_proteus").await;
            let d = connect(&url, &deimos_seed, "_INBOX_deimos").await;
            assert!(catalog::info(&p).await.unwrap().is_none());

            let g = connect(&url, &ganymede_seed, "_INBOX_ganymede").await;
            let first = index("2026-10-19T02:00:00+00:00", 2000);
            assert!(first.len() > 3 * 128 * 1024);
            catalog::put(&g, &first, "2026-10-19T02:00:00+00:00", "ganymede")
                .await
                .unwrap();

            for c in [&p, &d] {
                let info = catalog::info(c).await.unwrap().unwrap();
                assert_eq!(info.metadata["source"], "ganymede");
                assert_eq!(info.metadata["generated_at"], "2026-10-19T02:00:00+00:00");
                assert_eq!(catalog::download(c, &info).await.unwrap(), first);
            }

            // A second put replaces the first; the old chunks are purged.
            let second = index("2026-10-20T02:00:00+00:00", 1500);
            catalog::put(&g, &second, "2026-10-20T02:00:00+00:00", "ganymede")
                .await
                .unwrap();
            let info = catalog::info(&d).await.unwrap().unwrap();
            assert_eq!(info.metadata["generated_at"], "2026-10-20T02:00:00+00:00");
            assert_eq!(catalog::download(&d, &info).await.unwrap(), second);

            // Readers cannot write, open the bucket, or create a consumer
            // (a push consumer could deliver into another host's inbox).
            for c in [&p, &d] {
                assert!(catalog::put(c, b"{}", "x", "evil").await.is_err());
                assert!(denied(c, "$O.VEGA_LANISTA.C.evil", "x").await);
                assert!(denied(c, "$JS.API.STREAM.PURGE.OBJ_VEGA_LANISTA", "{}").await);
                assert!(
                    denied(
                        c,
                        "$JS.API.CONSUMER.CREATE.OBJ_VEGA_LANISTA",
                        r#"{"stream_name":"OBJ_VEGA_LANISTA","config":{"deliver_subject":"vega.inbox.proteus.x.titan"}}"#
                    )
                    .await
                );
            }
            // Nor can the writer: put rights are the bucket's only.
            assert!(
                denied(
                    &g,
                    "$JS.API.CONSUMER.CREATE.OBJ_VEGA_LANISTA",
                    r#"{"stream_name":"OBJ_VEGA_LANISTA","config":{}}"#
                )
                .await
            );
        });
}

/// vega-lhc.140: before the broker grants the bucket, the catalog's direct
/// get (receiver) and stream info (builder) are denied. That ended vegad: the
/// shared connect's event callback ran `exit(1)` on any permissions
/// violation. The child below runs the real catalog ticks and the relay's
/// connect as vegad in a process of its own, so an exit shows here.
#[test]
fn a_denied_catalog_does_not_end_vegad() {
    let dir = tempfile::tempdir().unwrap();
    let (seed, key) = user();
    // Mail rights only: no VEGA_LANISTA grant at all.
    let authz = format!(
        "authorization {{ users = [ {{ nkey: {key}, permissions: {{ \
         publish: {{ allow: [\"vega.inbox.>\"] }}, \
         subscribe: {{ allow: [\"vega.inbox.>\", \"_INBOX_proteus.>\"] }} }} }} ] }}"
    );
    let (_server, url) = start(dir.path(), &authz);
    // Up before the child connects (its connect does not retry).
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(connect(&url, &seed, "_INBOX_proteus"));
    let nk = dir.path().join("proteus.nk");
    std::fs::write(&nk, &seed).unwrap();
    let out = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "catalog_broker_test::denied_catalog_child",
            "--exact",
            "--ignored",
            "--nocapture",
        ])
        .env("VEGA_NATS_URL", &url)
        .env("VEGA_NATS_NKEY", &nk)
        .env("VEGA_POST_HOST", "proteus")
        .env("CATALOG_CHILD_DIR", dir.path())
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "vegad's catalog ended the process ({}):\n{stdout}\n{}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    // One line per denied subject, not one per tick.
    let denied = |subject: &str| {
        stdout
            .lines()
            .filter(|l| l.contains("broker right missing") && l.contains(subject))
            .count()
    };
    assert_eq!(denied("$JS.API.DIRECT.GET.OBJ_VEGA_LANISTA"), 1, "{stdout}");
    assert_eq!(
        denied("$JS.API.STREAM.INFO.OBJ_VEGA_LANISTA"),
        1,
        "{stdout}"
    );
}

#[test]
#[ignore = "run by a_denied_catalog_does_not_end_vegad in a process of its own"]
fn denied_catalog_child() {
    let dir = std::path::PathBuf::from(std::env::var("CATALOG_CHILD_DIR").unwrap());
    let (state, index) = (dir.join("lanista.json"), dir.join("model_index.json"));
    crate::enter_daemon();
    let now = vega_bd_sync::unix_now();

    // Receiver: twice, the check clock apart.
    for n in 0..2 {
        let line = catalog::tick(
            "proteus",
            "ganymede",
            &state,
            &index,
            now + n * cat::CHECK_EVERY,
        );
        assert!(line.starts_with("catalog: check failed"), "{line}");
    }
    // Builder: an index to publish, the fetch not due.
    std::fs::write(&index, br#"{"generated_at": "g1", "models": {}}"#).unwrap();
    State {
        fetched_at: now,
        fetch: "ok".into(),
        ..Default::default()
    }
    .save(&state)
    .unwrap();
    let line = catalog::tick("proteus", "proteus", &state, &index, now);
    assert!(line.starts_with("catalog: publish failed"), "{line}");

    // Mail still flows on the relay's connect.
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            let c = crate::connect("relay", None).await.unwrap();
            let mut sub = c.subscribe("vega.inbox.proteus.>").await.unwrap();
            c.publish("vega.inbox.proteus.w3V:p1.titan", "hi".into())
                .await
                .unwrap();
            let msg = tokio::time::timeout(std::time::Duration::from_secs(5), sub.next())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(&msg.payload[..], b"hi");
        });
}
