//! `auth-config --registry-read` against a real broker (vega-lhc.90): a local
//! nats-server in a temp dir with the rendered authorization block and
//! throwaway keys. The inbox-only host reads its one key and nothing else,
//! and cannot write; vegad's theme path resolves the control host's mode.

use std::net::TcpListener;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use async_nats::jetstream;
use serde_json::{json, Value};
use vega_theme_sync::{state, Env, Mode, Watcher};

use crate::auth::{self, Entry};
use crate::theme;

pub struct Server(Child);

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// A local nats-server with JetStream in `dir` and the rendered
/// authorization block; returns it (killed on drop) and its URL.
pub fn start(dir: &std::path::Path, authz: &str) -> (Server, String) {
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let conf = dir.join("nats.conf");
    std::fs::write(
        &conf,
        format!(
            "listen: 127.0.0.1:{port}\njetstream {{ store_dir: \"{}\" }}\n{authz}",
            dir.join("js").display()
        ),
    )
    .unwrap();
    let child = Command::new("nats-server")
        .arg("-c")
        .arg(&conf)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("nats-server not on PATH: run the tests via `mise run check`");
    (Server(child), format!("nats://127.0.0.1:{port}"))
}

pub fn user() -> (String, String) {
    let kp = nkeys::KeyPair::new_user();
    (kp.seed().unwrap(), kp.public_key())
}

pub async fn connect(url: &str, seed: &str, inbox: &str) -> async_nats::Client {
    for _ in 0..50 {
        if let Ok(c) = async_nats::ConnectOptions::with_nkey(seed.into())
            .custom_inbox_prefix(inbox)
            .request_timeout(Some(Duration::from_millis(500)))
            .connect(url)
            .await
        {
            return c;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("local nats-server did not come up at {url}");
}

/// Denied publishes are dropped by the server: the request times out.
pub async fn denied(c: &async_nats::Client, subject: &str, payload: &str) -> bool {
    c.request(subject.to_string(), payload.to_string().into())
        .await
        .is_err()
}

#[test]
fn registry_read_is_one_key_read_only() {
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
    let reads = vec![auth::parse_registry_read("deimos=proteus.theme").unwrap()];
    let authz = auth::render(
        &entries,
        Some(&admin),
        &["registry".into()],
        &["deimos".into()],
        None,
        &[],
        &[],
        &reads,
        None,
    );
    let (_server, url) = start(dir.path(), &authz);

    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            // The admin creates the bucket as `vega post provision registry` does.
            let a = connect(&url, &admin_seed, crate::ADMIN_INBOX).await;
            jetstream::new(a)
                .create_key_value(jetstream::kv::Config {
                    bucket: vega_registry::BUCKET.into(),
                    history: 1,
                    max_bytes: 64 << 20,
                    ..Default::default()
                })
                .await
                .unwrap();
            let p = connect(&url, &proteus_seed, "_INBOX_proteus").await;
            let pkv = jetstream::new(p.clone())
                .get_key_value(vega_registry::BUCKET)
                .await
                .unwrap();
            pkv.put("proteus.theme", json!({"mode": "dark"}).to_string().into())
                .await
                .unwrap();
            pkv.put("proteus.beat", json!({"at": 1}).to_string().into())
                .await
                .unwrap();
            let g = connect(&url, &ganymede_seed, "_INBOX_ganymede").await;
            jetstream::new(g)
                .get_key_value(vega_registry::BUCKET)
                .await
                .unwrap()
                .put(
                    "ganymede.theme",
                    json!({"mode": "light"}).to_string().into(),
                )
                .await
                .unwrap();

            // Controls: the same calls succeed with a full host's grant, so a
            // failure below is the permission, not the probe.
            assert!(vega_registry::get_one(&p, "ganymede.theme")
                .await
                .unwrap()
                .is_some());
            assert!(!denied(&p, "$KV.VEGA_REGISTRY.proteus.x", "{}").await);
            assert!(
                !denied(
                    &p,
                    "$JS.API.STREAM.INFO.KV_VEGA_REGISTRY",
                    r#"{"subjects_filter":">"}"#
                )
                .await
            );

            let d = connect(&url, &deimos_seed, "_INBOX_deimos").await;
            // The granted key.
            assert_eq!(
                vega_registry::get_one(&d, "proteus.theme").await.unwrap(),
                Some(json!({"mode": "dark"}))
            );
            // No other key, also not through the bare direct get or stream info.
            assert!(vega_registry::get_one(&d, "ganymede.theme").await.is_err());
            assert!(vega_registry::get_one(&d, "proteus.beat").await.is_err());
            assert!(
                denied(
                    &d,
                    "$JS.API.DIRECT.GET.KV_VEGA_REGISTRY",
                    r#"{"last_by_subj":"$KV.VEGA_REGISTRY.ganymede.theme"}"#
                )
                .await
            );
            assert!(
                denied(
                    &d,
                    "$JS.API.STREAM.INFO.KV_VEGA_REGISTRY",
                    r#"{"subjects_filter":">"}"#
                )
                .await
            );
            assert!(jetstream::new(d.clone())
                .get_key_value(vega_registry::BUCKET)
                .await
                .is_err());
            // A payload on the granted subject never yields another key.
            for body in [
                r#"{"seq":1}"#,
                r#"{"last_by_subj":"$KV.VEGA_REGISTRY.ganymede.theme"}"#,
            ] {
                if let Ok(m) = d
                    .request(
                        vega_registry::direct_get_subject("proteus.theme"),
                        body.into(),
                    )
                    .await
                {
                    let subj = m
                        .headers
                        .as_ref()
                        .and_then(|h| h.get("Nats-Subject"))
                        .map(|v| v.as_str().to_string());
                    assert!(
                        subj.is_none()
                            || subj.as_deref() == Some("$KV.VEGA_REGISTRY.proteus.theme"),
                        "{body}: {subj:?}"
                    );
                }
            }
            // No put, delete or purge, of the granted key or its own.
            assert!(denied(&d, "$KV.VEGA_REGISTRY.proteus.theme", r#"{"mode":"light"}"#).await);
            assert!(denied(&d, "$KV.VEGA_REGISTRY.deimos.theme", r#"{"mode":"light"}"#).await);
            let mut del = async_nats::HeaderMap::new();
            del.insert("KV-Operation", "DEL");
            assert!(d
                .request_with_headers("$KV.VEGA_REGISTRY.proteus.theme", del, "".into())
                .await
                .is_err());
            assert!(denied(&d, "$JS.API.STREAM.PURGE.KV_VEGA_REGISTRY", "{}").await);
            // No watch: neither a consumer nor a plain subscription.
            assert!(denied(&d, "$JS.API.CONSUMER.CREATE.KV_VEGA_REGISTRY", "{}").await);
            assert!(
                denied(
                    &d,
                    "$JS.API.CONSUMER.CREATE.KV_VEGA_REGISTRY.w.$KV.VEGA_REGISTRY.>",
                    "{}"
                )
                .await
            );
            let mut sub = d.subscribe("$KV.VEGA_REGISTRY.>").await.unwrap();
            d.flush().await.unwrap();
            pkv.put("proteus.beat", json!({"at": 2}).to_string().into())
                .await
                .unwrap();
            use futures_util::StreamExt;
            // Silent or closed by the server; never a message.
            if let Ok(Some(m)) = tokio::time::timeout(Duration::from_millis(500), sub.next()).await
            {
                panic!("deimos watched {}", m.subject);
            }
            assert_eq!(
                vega_registry::get_one(&d, "proteus.theme").await.unwrap(),
                Some(json!({"mode": "dark"}))
            );

            // vegad's path on the inbox-only host: the control mode, applied.
            let modes = theme::mode_of(&d, "proteus").await.unwrap();
            assert_eq!(modes.get("proteus"), Some(&Mode::Dark));
            let home = tempfile::tempdir().unwrap();
            let env = Env::at(home.path());
            let remote: theme::Remote = Default::default();
            *remote.lock().unwrap() = modes;
            let mut w = Watcher::default();
            let mut err = String::new();
            assert!(theme::step(&env, None, &mut w, &remote, &mut err).is_some());
            let applied = state::read(&env).unwrap();
            assert_eq!(
                (applied.mode, applied.source.as_str()),
                (Mode::Dark, "registry:proteus")
            );

            // A deleted key reads as nothing, not as an error.
            pkv.delete("proteus.theme").await.unwrap();
            assert_eq!(
                vega_registry::get_one(&d, "proteus.theme").await.unwrap(),
                None::<Value>
            );
            assert!(theme::mode_of(&d, "proteus").await.unwrap().is_empty());
        });
}

#[test]
fn registry_read_needs_an_inbox_only_host() {
    let (_, p) = user();
    let (_, d) = user();
    let entries = [format!("proteus={p}"), format!("deimos={d}")];
    let err = crate::auth_config(
        &entries,
        None,
        None,
        &[],
        &[],
        None,
        &[],
        &[],
        &["deimos=proteus.theme".into()],
        None,
    )
    .unwrap_err();
    assert!(
        err.message.contains("only inbox-only hosts"),
        "{}",
        err.message
    );
    assert!(crate::auth_config(
        &entries,
        None,
        None,
        &[],
        &["deimos".into()],
        None,
        &[],
        &[],
        &["deimos=proteus.*".into()],
        None,
    )
    .is_err());
}

#[test]
fn project_pins_cross_hosts_under_rendered_grants() {
    use vega_registry::pins::{self, Pin};
    let dir = tempfile::tempdir().unwrap();
    let (admin_seed, admin) = user();
    let (pseed, pkey) = user();
    let (gseed, gkey) = user();
    let (dseed, dkey) = user();
    let entries = vec![
        Entry {
            host: "proteus".into(),
            key: pkey,
        },
        Entry {
            host: "ganymede".into(),
            key: gkey,
        },
        Entry {
            host: "deimos".into(),
            key: dkey,
        },
    ];
    let project = "github.com/durandom/vega";
    let k = pins::key("proteus", project, "agents", "codex", "budget");
    let uk = vega_registry::uac_key("proteus", project);
    let reads = vec![
        auth::parse_registry_read(&format!("deimos={k}")).unwrap(),
        auth::parse_registry_read(&format!("deimos={uk}")).unwrap(),
    ];
    let authz = auth::render(
        &entries,
        Some(&admin),
        &["registry".into()],
        &["deimos".into()],
        None,
        &[],
        &[],
        &reads,
        None,
    );
    let (_server, url) = start(dir.path(), &authz);
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            let a = connect(&url, &admin_seed, crate::ADMIN_INBOX).await;
            jetstream::new(a)
                .create_key_value(jetstream::kv::Config {
                    bucket: vega_registry::BUCKET.into(),
                    history: 1,
                    ..Default::default()
                })
                .await
                .unwrap();
            let p = connect(&url, &pseed, "_INBOX_proteus").await;
            let g = connect(&url, &gseed, "_INBOX_ganymede").await;
            let d = connect(&url, &dseed, "_INBOX_deimos").await;
            // Cutover uses only an owner's snapshotted keys. Foreign legacy
            // keys survive this owner's apply; replay uses the same backup.
            let legacy = "proteus.lanista.old";
            let old_project = "~/src/vega/.git";
            let old_pin = json!({"project":old_project,"seat":"agents","lane":"codex","tier":"budget","model":null,"at":7,"expires":1});
            let kv = vega_registry::bucket(&p).await.unwrap();
            kv.put(legacy, old_pin.to_string().into()).await.unwrap();
            let gkv = vega_registry::bucket(&g).await.unwrap();
            gkv.put("ganymede.uac.old", json!({"project":old_project,"pane":"old"}).to_string().into()).await.unwrap();
            let snapshot = std::collections::BTreeMap::from([(legacy.into(), old_pin), ("ganymede.uac.old".into(), json!({"project":old_project,"pane":"old"}))]);
            let ids = std::collections::BTreeMap::from([(old_project.into(), serde_json::from_value(json!(project)).unwrap())]);
            let migrated = std::collections::BTreeMap::from([(uk.clone(), json!({"schema":2,"host":"proteus","pane":"w1:p1","project":project,"at":1,"folder_name":"vega"}))]);
            let owned_old_uac = "proteus.uac.old";
            kv.put(owned_old_uac, json!({"project":old_project,"pane":"old"}).to_string().into()).await.unwrap();
            vega_registry::reconcile(&kv,"proteus",Default::default(),&Default::default(),&[legacy.into(),owned_old_uac.into()]).await.unwrap();
            assert!(vega_registry::get_one(&p,legacy).await.unwrap().is_some());
            assert!(vega_registry::get_one(&p,owned_old_uac).await.unwrap().is_some());
            let plan = vega_registry::cutover::Plan::prepare("proteus", &snapshot, &ids, migrated).unwrap();
            vega_registry::cutover::validate(std::slice::from_ref(&plan)).unwrap();
            plan.apply(&kv).await.unwrap();
            plan.apply(&kv).await.unwrap();
            assert!(vega_registry::get_one(&p, legacy).await.unwrap().is_none());
            assert!(vega_registry::get_one(&p, "ganymede.uac.old").await.unwrap().is_some());
            assert_eq!(vega_registry::get_one(&d, &uk).await.unwrap().unwrap()["project"], project);
            assert!(vega_registry::get_one(&d, "ganymede.uac.old").await.is_err());
            assert!(denied(&d, &format!("$KV.VEGA_REGISTRY.{uk}"), "{}").await);
            assert!(denied(&d, &format!("$JS.API.STREAM.INFO.KV_{}", vega_registry::BUCKET), "{}").await);
            let record = Pin {
                schema: 2,
                project: serde_json::from_value(json!(project)).unwrap(),
                seat: "agents".into(),
                lane: "codex".into(),
                tier: "budget".into(),
                model: None,
                at: 0,
                expires: Some(100),
            };
            let resolve = |v: &vega_registry::View, now| {
                pins::latest(v, project, "agents", "codex", "budget")
                    .and_then(|(h, p)| p.active(now).map(|m| (h, m.to_string())))
            };
            pins::access_with(
                &p,
                "proteus",
                record.clone(),
                Some(Some("test-model".into())),
                None,
                10,
            )
            .await
            .unwrap();
            let view = pins::access_with(&g, "ganymede", record.clone(), None, None, 10)
                .await
                .unwrap();
            assert_eq!(
                resolve(&view, 10),
                Some(("proteus".into(), "test-model".into()))
            );
            assert!(pins::latest(&view, "~/other/.git", "agents", "codex", "budget").is_none());
            assert!(pins::latest(&view, project, "agents", "codex", "flagship").is_none());
            // vegad must not delete independently managed pins on restart/reconcile.
            let kv = vega_registry::bucket(&p).await.unwrap();
            vega_registry::reconcile(
                &kv,
                "proteus",
                Default::default(),
                &Default::default(),
                std::slice::from_ref(&k),
            )
            .await
            .unwrap();
            assert!(vega_registry::get_one(&g, &k).await.unwrap().is_some());
            // An inbox-only host reads precisely its granted key, without stream info.
            let hosts = ["proteus".to_string()];
            let view = pins::access_with(&d, "deimos", record.clone(), None, Some(&hosts), 10)
                .await
                .unwrap();
            assert_eq!(resolve(&view, 10).unwrap().1, "test-model");
            assert!(
                pins::access_with(&d, "deimos", record.clone(), None, None, 10)
                    .await
                    .is_err()
            );
            assert!(denied(&g, &format!("$KV.VEGA_REGISTRY.{k}"), "{}").await);
            // At the reset, owner prunes the payload but preserves a tombstone.
            let view = pins::access_with(&p, "proteus", record.clone(), None, None, 100)
                .await
                .unwrap();
            assert_eq!(resolve(&view, 100), None);
            let pruned = vega_registry::get_one(&p, &k).await.unwrap().unwrap();
            assert!(pruned["model"].is_null());
            assert!(pruned["at"].as_u64().unwrap() > 0);
            // A new set from another host wins; a clear on the first host wins next.
            let mut never = record.clone();
            never.expires = None;
            pins::access_with(
                &g,
                "ganymede",
                never.clone(),
                Some(Some("other-model".into())),
                None,
                101,
            )
            .await
            .unwrap();
            let view = pins::access_with(&p, "proteus", never, Some(None), None, 102)
                .await
                .unwrap();
            assert_eq!(resolve(&view, 102), None);
            assert_eq!(
                pins::latest(&view, project, "agents", "codex", "budget")
                    .unwrap()
                    .0,
                "proteus"
            );
            let view = pins::access_with(&g, "ganymede", record, None, None, 102)
                .await
                .unwrap();
            assert_eq!(resolve(&view, 102), None);
        });
}
