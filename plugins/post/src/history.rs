//! Off-loop publishing and lineage persistence for the central history stream.
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{mpsc, OnceLock};
use std::time::Duration;
use vega_herdr::history as outbox;

pub const STREAM: &str = "VEGA_HISTORY";
static TX: OnceLock<mpsc::SyncSender<Work>> = OnceLock::new();
enum Work {
    Event(Value),
    Lineage(BTreeMap<String, Value>),
}

pub fn config() -> async_nats::jetstream::stream::Config {
    async_nats::jetstream::stream::Config {
        name: STREAM.into(),
        subjects: vec!["vega.history.>".into()],
        max_bytes: 10 << 30,
        max_age: Duration::ZERO,
        retention: async_nats::jetstream::stream::RetentionPolicy::Limits,
        // Retain existing history at capacity; report/retry new publications.
        discard: async_nats::jetstream::stream::DiscardPolicy::New,
        storage: async_nats::jetstream::stream::StorageType::File,
        ..Default::default()
    }
}

/// Existing history must satisfy the retention contract too; never call an
/// incompatible stream "ready", and never update its configuration implicitly.
pub async fn provision(js: &async_nats::jetstream::Context) -> Result<(), crate::Failure> {
    let stream = js
        .get_or_create_stream(config())
        .await
        .map_err(crate::broker_err)?;
    validate_config(&stream.cached_info().config)
}

fn validate_config(actual: &async_nats::jetstream::stream::Config) -> Result<(), crate::Failure> {
    let expected = config();
    if actual.name != expected.name
        || actual.subjects != expected.subjects
        || actual.retention != expected.retention
        || actual.max_age != expected.max_age
        || actual.max_bytes != expected.max_bytes
        || actual.storage != expected.storage
        || actual.discard != expected.discard
        || actual.max_messages > 0
        || actual.max_messages_per_subject > 0
        || actual.allow_rollup
        || actual.allow_message_ttl
    {
        return Err(crate::Failure::new("history_config", "VEGA_HISTORY exists with incompatible retention or subjects", "inspect VEGA_HISTORY with the admin/viewer principal and reconcile its configuration with docs/design/history.md; vega will not change or delete existing history"));
    }
    Ok(())
}

fn submit(work: Work) {
    if let Some(tx) = TX.get() {
        if tx.try_send(work).is_err() {
            crate::log("history: worker queue full or unavailable; event not queued");
        }
    } else {
        crate::log("history: worker not started; event not queued");
    }
}

pub fn record(kind: &str, data: Value) {
    submit(Work::Event(outbox::event(&crate::host(), kind, data)));
}

pub fn lineage(desired: &BTreeMap<String, Value>) {
    submit(Work::Lineage(
        desired
            .iter()
            .filter(|(k, _)| tracked(k))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect(),
    ));
}
fn tracked(key: &str) -> bool {
    matches!(key.split('.').nth(1), Some("agent" | "link" | "uac"))
}

#[derive(Clone, Default, serde::Serialize, serde::Deserialize)]
struct Lineage {
    values: BTreeMap<String, Value>,
    chains: BTreeMap<String, String>,
}
impl Lineage {
    fn reconcile(
        &mut self,
        dir: &Path,
        host: &str,
        mut desired: BTreeMap<String, Value>,
    ) -> std::io::Result<()> {
        enrich_lineage(&mut desired);
        let keys: std::collections::BTreeSet<_> =
            self.values.keys().chain(desired.keys()).cloned().collect();
        for key in keys {
            let old = self.values.get(&key);
            let new = desired.get(&key);
            if old == new {
                continue;
            }
            let action = if new.is_none() {
                "end"
            } else if old.is_none() {
                "start"
            } else {
                "change"
            };
            let id = outbox::hash(&format!(
                "{host}:{key}:{}:{}",
                self.chains.get(&key).map(String::as_str).unwrap_or(""),
                new.map(Value::to_string).unwrap_or_default()
            ));
            let value = new.or(old).cloned().unwrap_or(Value::Null);
            let mut e = outbox::event(
                host,
                &format!("lineage.{action}"),
                json!({"key":key,"value":value,"previous":old,
                "project":value["project"],"bead":value["bead"],"pane":value["pane"],"session":value["session"]}),
            );
            e["id"] = json!(id);
            outbox::enqueue(dir, &e)?;
            let mut next = self.clone();
            next.chains.insert(key.clone(), id);
            if let Some(v) = new {
                next.values.insert(key, v.clone());
            } else {
                next.values.remove(&key);
            }
            // Persist before publishing receipts. Repeating a write after a crash
            // has the same event ID and cannot duplicate the broker event.
            let bytes = serde_json::to_vec(&next)?;
            let temp = dir.join("lineage.tmp");
            std::fs::write(&temp, bytes)?;
            std::fs::File::open(&temp)?.sync_all()?;
            std::fs::rename(temp, dir.join("lineage.state"))?;
            std::fs::File::open(dir)?.sync_all()?;
            *self = next;
        }
        Ok(())
    }
}

// Disk reads and project probes belong to the worker, never the relay loop.
fn enrich_lineage(desired: &mut BTreeMap<String, Value>) {
    for (key, value) in desired.iter_mut() {
        if key.split('.').nth(1) != Some("agent") {
            continue;
        }
        let (Some(pane), Some(name), Some(kind)) = (
            value["pane"].as_str(),
            value["name"].as_str(),
            value["kind"].as_str(),
        ) else {
            continue;
        };
        if let Some(spawn) = crate::st::spawn_of_on(None, pane, name, kind) {
            value["parent"] = json!(spawn.parent);
            value["tier"] = json!(spawn.tier);
            value["model"] = json!(spawn.model);
            value["session"] = json!(spawn.session);
            value["task"] = json!(spawn.task);
        }
    }
}

pub fn spawn() {
    let host = crate::host();
    let Some(dir) = outbox::root() else {
        return;
    };
    let (tx, rx) = mpsc::sync_channel(64);
    if TX.set(tx).is_err() {
        return;
    }
    std::thread::spawn(move || {
        let Ok(rt) = crate::runtime() else {
            crate::log("history: cannot start runtime; worker stopped");
            return;
        };
        let mut lineage: Lineage = match std::fs::read(dir.join("lineage.state")) {
            Ok(bytes) => match serde_json::from_slice(&bytes) {
                Ok(v) => v,
                Err(e) => {
                    crate::log(&format!(
                        "history: invalid lineage checkpoint: {e}; worker stopped"
                    ));
                    return;
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Lineage::default(),
            Err(e) => {
                crate::log(&format!(
                    "history: cannot read lineage checkpoint: {e}; worker stopped"
                ));
                return;
            }
        };
        let mut client = None;
        let mut last_failure = None;
        loop {
            // Waits are on this dedicated worker, never the relay runtime.
            let first = rx.recv_timeout(Duration::from_secs(5)).ok();
            for work in first.into_iter().chain(rx.try_iter()).take(64) {
                let result = match work {
                    Work::Event(mut e) => {
                        outbox::enrich(&mut e);
                        outbox::enqueue(&dir, &e)
                    }
                    Work::Lineage(v) => lineage.reconcile(&dir, &host, v),
                };
                if let Err(e) = result {
                    crate::log(&format!("history: capture failed: {e}"));
                }
            }
            if !crate::broker_configured() {
                continue;
            }
            let result = rt.block_on(async {
                if client.is_none() {
                    client = Some(
                        crate::connect("history", None)
                            .await
                            .map_err(|e| e.message)?,
                    );
                }
                let js = async_nats::jetstream::new(client.as_ref().unwrap().clone());
                drain(&js, &dir, &host).await
            });
            if let Err(e) = result {
                if last_failure.as_ref() != Some(&e) {
                    crate::log(&format!("history: publish deferred: {e}"));
                    last_failure = Some(e);
                }
                client = None;
                std::thread::sleep(Duration::from_secs(5));
            } else if last_failure.take().is_some() {
                crate::log("history: publication recovered");
            }
        }
    });
}

async fn publish(
    js: &async_nats::jetstream::Context,
    host: &str,
    event: &Value,
) -> Result<(), String> {
    let id = event["id"]
        .as_str()
        .filter(|s| s.len() == 64 && s.bytes().all(|c| c.is_ascii_hexdigit()))
        .ok_or("invalid event ID")?;
    if event["host"] != host {
        return Err("outbox event belongs to another host".into());
    }
    let subject = format!("vega.history.{host}.{id}");
    let mut headers = async_nats::HeaderMap::new();
    headers.insert("Nats-Msg-Id", id);
    // Each event owns a unique subject: the compare-and-set makes retries
    // idempotent even after JetStream's finite deduplication window expires.
    headers.insert("Nats-Expected-Last-Subject-Sequence", "0");
    headers.insert("Nats-Expected-Stream", STREAM);
    let ack = js
        .publish_with_headers(subject, headers, event.to_string().into())
        .await
        .map_err(|e| e.to_string())?;
    match ack.await {
        Ok(_) => Ok(()),
        Err(e)
            if e.kind() == async_nats::jetstream::context::PublishErrorKind::WrongLastSequence =>
        {
            Ok(())
        }
        Err(e) => Err(e.to_string()),
    }
}

async fn drain(js: &async_nats::jetstream::Context, dir: &Path, host: &str) -> Result<(), String> {
    let mut published = 0;
    for path in outbox::pending(dir).map_err(|e| e.to_string())? {
        let event = match read_receipt(&path, host) {
            Ok(event) => event,
            Err(error) => {
                quarantine_receipt(&path, &error);
                continue;
            }
        };
        // Bad receipts do not consume the publish budget: even when their
        // quarantine rename fails, they cannot starve later valid receipts.
        if published == 32 {
            break;
        }
        tokio::time::timeout(Duration::from_secs(5), publish(js, host, &event))
            .await
            .map_err(|_| "publish timeout".to_string())??;
        std::fs::remove_file(path).map_err(|e| e.to_string())?;
        published += 1;
    }
    Ok(())
}

fn read_receipt(path: &Path, host: &str) -> Result<Value, String> {
    let event: Value = serde_json::from_slice(&std::fs::read(path).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    if event["schema"] != 1
        || !event["kind"].is_string()
        || event["host"] != host
        || !event["id"]
            .as_str()
            .is_some_and(|id| id.len() == 64 && id.bytes().all(|b| b.is_ascii_hexdigit()))
    {
        return Err("invalid history receipt envelope (schema, kind, host or id)".into());
    }
    Ok(event)
}

/// Preserve damaged bytes for inspection without letting one receipt block
/// the queue. A fresh suffix never overwrites an earlier quarantined receipt.
fn quarantine_receipt(path: &Path, reason: &str) {
    static FAILED: OnceLock<std::sync::Mutex<std::collections::BTreeSet<std::path::PathBuf>>> =
        OnceLock::new();
    let target = path.with_extension(format!("corrupt-{}", nuid::next()));
    match std::fs::rename(path, &target) {
        Ok(()) => {
            crate::log(&format!(
                "history: quarantined {} as {}: {reason}; inspect the preserved receipt",
                path.display(),
                target.display()
            ));
            if let Some(dir) = path.parent() {
                if let Err(error) = std::fs::File::open(dir).and_then(|f| f.sync_all()) {
                    crate::log(&format!(
                        "history: could not sync quarantine directory {}: {error}",
                        dir.display()
                    ));
                }
            }
            if let Ok(mut failed) = FAILED.get_or_init(Default::default).lock() {
                failed.remove(path);
            }
        }
        Err(error) => {
            // A permissions problem must not block later receipts either.
            // Retry the rename next round, but report a given path only once.
            if let Ok(mut failed) = FAILED.get_or_init(Default::default).lock() {
                if failed.insert(path.into()) {
                    crate::log(&format!("history: cannot quarantine {}: {error}; receipt skipped ({reason}); inspect its permissions", path.display()));
                }
            }
        }
    }
}

/// Wrap the existing acknowledged mail publish, retaining its error contract.
/// Every successful send path (CLI, tool, bounce, ask, Uber, quarantine grant, PagerDuty)
/// uses this; a history failure never changes mail delivery's result.
pub async fn mail(
    js: &async_nats::jetstream::Context,
    subject: String,
    headers: async_nats::HeaderMap,
    body: String,
) -> Result<
    impl std::future::Future<
        Output = Result<
            async_nats::jetstream::publish::PublishAck,
            async_nats::jetstream::context::PublishError,
        >,
    >,
    async_nats::jetstream::context::PublishError,
> {
    let envelope: BTreeMap<String, Vec<String>> = headers
        .iter()
        .map(|(k, v)| (k.to_string(), v.iter().map(ToString::to_string).collect()))
        .collect();
    let mut event = outbox::event(
        &crate::host(),
        "mail.sent",
        json!({"subject":subject,"headers":envelope,"body":body,
        "pane":headers.get(crate::h::FROM_PANE).map(|v|v.as_str()),
        "cwd":headers.get(crate::h::FROM_CWD).map(|v|v.as_str())}),
    );
    if let Some(id) = headers.get("Nats-Msg-Id") {
        event["id"] = json!(outbox::hash(&format!(
            "mail:{}:{subject}:{}",
            crate::host(),
            id.as_str()
        )));
    }
    let ack = js
        .publish_with_headers(subject, headers, body.into())
        .await?;
    Ok(async move {
        let result = ack.await?;
        if TX.get().is_some() {
            submit(Work::Event(event));
        } else if let Some(dir) = outbox::root() {
            outbox::enrich(&mut event);
            if let Err(e) = outbox::enqueue(&dir, &event) {
                eprintln!("history: mail event not queued: {e}");
            }
        }
        Ok(result)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::{self, Entry};
    use crate::registry_read_test::{connect, denied, start, user};

    #[test]
    fn failed_herdr_snapshot_preserves_live_registry_and_lineage() {
        use std::os::unix::fs::PermissionsExt;
        const FIXTURE: &str = "VEGA_HISTORY_SNAPSHOT_FIXTURE";
        if std::env::var_os(FIXTURE).is_none() {
            let root = tempfile::tempdir().unwrap();
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "history::tests::failed_herdr_snapshot_preserves_live_registry_and_lineage",
                    "--nocapture",
                ])
                .env(FIXTURE, "1")
                .env("HOME", root.path())
                .env("XDG_STATE_HOME", root.path().join("state"))
                .env("XDG_CONFIG_HOME", root.path().join("config"))
                .env("VEGA_POST_HOST", "proteus")
                .env("VEGA_POST_INBOX_ONLY", "")
                .current_dir(root.path())
                .output()
                .unwrap();
            let stdout = String::from_utf8_lossy(&output.stdout);
            assert!(
                output.status.success(),
                "{stdout}{}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(
                stdout.contains("registry: off (fixture snapshot failure); local state only"),
                "{stdout}"
            );
            return;
        }
        let root = std::path::PathBuf::from(std::env::var_os("HOME").unwrap());
        let bin = root.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let herdr = bin.join("herdr");
        std::fs::write(&herdr, include_str!("../tests/fixtures/history-herdr.sh")).unwrap();
        std::fs::set_permissions(&herdr, std::fs::Permissions::from_mode(0o755)).unwrap();
        let mut paths = vec![bin];
        paths.extend(std::env::split_paths(&std::env::var_os("PATH").unwrap()));
        std::env::set_var("PATH", std::env::join_paths(paths).unwrap());
        let cutover = crate::service::state_dir()
            .unwrap()
            .join("identity-cutover");
        std::fs::create_dir_all(&cutover).unwrap();
        let plan = json!({"schema":2,"host":"proteus","inbox_only":false,"mappings":[],"original":{},"writes":{},"remove":[],"registry":{"host":"proteus","backup":{},"projection":{}},"registry_reads":[],"owned":{}});
        std::fs::write(
            cutover.join("approved.json"),
            json!({"schema":2,"hosts":[plan],"grants":{}}).to_string(),
        )
        .unwrap();

        let (admin_seed, admin) = user();
        let (host_seed, host) = user();
        let entries = [Entry {
            host: "proteus".into(),
            key: host,
        }];
        let authz = auth::render(
            &entries,
            Some(&admin),
            &["registry".into()],
            &[],
            None,
            &[],
            &[],
            &[],
            None,
        );
        let (_server, url) = start(&root, &authz);
        // Intercept the real live-loop producer; persist every submitted
        // snapshot just as the worker does, without spawning unrelated jobs.
        let (tx, rx) = mpsc::sync_channel(64);
        assert!(TX.set(tx).is_ok());
        crate::runtime().unwrap().block_on(async {
            let admin = connect(&url, &admin_seed, crate::ADMIN_INBOX).await;
            async_nats::jetstream::new(admin).create_key_value(async_nats::jetstream::kv::Config {
                bucket: vega_registry::BUCKET.into(), ..Default::default()
            }).await.unwrap();
            let client = connect(&url, &host_seed, "_INBOX_proteus").await;
            let kv = vega_registry::bucket(&client).await.unwrap();
            let key = vega_registry::agent_key("proteus", "w1:p1");
            let value = json!({"schema":2,"pane":"w1:p1","name":"worker","kind":"codex","status":"working"});
            kv.put(&key, value.to_string().into()).await.unwrap();
            let desired = BTreeMap::from([(key.clone(), value.clone())]);
            let spool = outbox::root().unwrap();
            let mut history = Lineage::default();
            history.reconcile(&spool, "proteus", desired.clone()).unwrap();
            let before = outbox::pending(&spool).unwrap();
            for failed in ["agent", "workspace"] {
                std::env::set_var("VEGA_SNAPSHOT_FAIL", failed);
                let mut registry = crate::Registry {
                    kv: Some(kv.clone()), last: desired.clone(),
                    janitor_next: Some(std::time::Instant::now() + Duration::from_secs(3600)),
                    ..Default::default()
                };
                registry.run(&client, "proteus", &mut crate::Supervisor::default()).await;
                for work in rx.try_iter() {
                    if let Work::Lineage(values) = work {
                        history.reconcile(&spool, "proteus", values).unwrap();
                    }
                }
                assert_eq!(registry.state, "registry: off", "failed {failed} snapshot must stop reconciliation");
                let saved = kv.get(&key).await.unwrap().expect("live agent must not be deleted");
                assert_eq!(serde_json::from_slice::<Value>(&saved).unwrap(), value);
                assert_eq!(outbox::pending(&spool).unwrap(), before, "failed {failed} snapshot emitted lifecycle events");
                for path in outbox::pending(&spool).unwrap() {
                    let event: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
                    assert_ne!(event["kind"], "lineage.end");
                }
            }
        });
    }

    #[test]
    fn incompatible_history_is_not_reported_ready() {
        validate_config(&config()).unwrap();
        let mut old = config();
        old.max_age = Duration::from_secs(3600);
        assert!(validate_config(&old).is_err());
        let mut old = config();
        old.discard = async_nats::jetstream::stream::DiscardPolicy::Old;
        assert!(validate_config(&old).is_err());
    }

    #[test]
    fn lineage_restart_retains_last_value_and_deduplicates() {
        let tmp = tempfile::tempdir().unwrap();
        let a = BTreeMap::from([(
            "h.agent.p".into(),
            json!({"pane":"p","project":"x","model":"m"}),
        )]);
        let mut state = Lineage::default();
        state.reconcile(tmp.path(), "h", a.clone()).unwrap();
        let mut state: Lineage =
            serde_json::from_slice(&std::fs::read(tmp.path().join("lineage.state")).unwrap())
                .unwrap();
        state.reconcile(tmp.path(), "h", a.clone()).unwrap();
        assert_eq!(outbox::pending(tmp.path()).unwrap().len(), 1);
        state.reconcile(tmp.path(), "h", BTreeMap::new()).unwrap();
        let rows: Vec<Value> = outbox::pending(tmp.path())
            .unwrap()
            .iter()
            .map(|p| serde_json::from_slice(&std::fs::read(p).unwrap()).unwrap())
            .collect();
        assert_eq!(rows.len(), 2);
        let end = rows.iter().find(|v| v["kind"] == "lineage.end").unwrap();
        assert_eq!(end["data"]["value"]["model"], "m");
        state.reconcile(tmp.path(), "h", a).unwrap();
        assert_eq!(outbox::pending(tmp.path()).unwrap().len(), 3);
    }

    #[test]
    fn history_and_archive_permissions_and_retry_on_local_broker() {
        let dir = tempfile::tempdir().unwrap();
        let (admin_seed, admin) = user();
        let (host_seed, host) = user();
        let (denied_seed, deimos) = user();
        let (viewer_seed, viewer) = user();
        let entries = vec![
            Entry {
                host: "proteus".into(),
                key: host,
            },
            Entry {
                host: "deimos".into(),
                key: deimos,
            },
        ];
        let authz = auth::render(
            &entries,
            Some(&admin),
            &["history".into(), "archive".into(), "proteus".into()],
            &["deimos".into()],
            Some(&viewer),
            &[],
            &[],
            &[],
            None,
        );
        let (_server, url) = start(dir.path(), &authz);
        crate::runtime().unwrap().block_on(async {
            let admin = connect(&url, &admin_seed, crate::ADMIN_INBOX).await;
            let js = async_nats::jetstream::new(admin.clone());
            let mut cfg = config();
            cfg.duplicate_window = Duration::from_millis(100);
            let mut stream = js.create_stream(cfg).await.unwrap();
            provision(&js).await.unwrap();
            let mut old = crate::stream_config("proteus");
            old.max_age = Duration::from_secs(365 * 86400);
            old.max_bytes = 1 << 30;
            old.description = Some("keep this".into());
            let before = js
                .create_stream(old)
                .await
                .unwrap()
                .cached_info()
                .config
                .clone();
            assert!(crate::archive::update(&admin, "proteus").await.unwrap());
            assert!(!crate::archive::update(&admin, "proteus").await.unwrap());
            let after = js
                .get_stream(crate::stream_name("proteus"))
                .await
                .unwrap()
                .cached_info()
                .config
                .clone();
            let mut expected = before;
            expected.max_age = Duration::ZERO;
            expected.max_bytes = crate::ARCHIVE_MAX_BYTES;
            assert_eq!(
                serde_json::to_value(expected).unwrap(),
                serde_json::to_value(after).unwrap()
            );
            let host = connect(&url, &host_seed, "_INBOX_proteus").await;
            assert!(denied(&host, "$JS.API.STREAM.UPDATE.VEGA_INBOX_proteus", "{}").await);
            assert!(denied(&host, "$JS.API.STREAM.MSG.GET.VEGA_HISTORY", "{\"seq\":1}").await);
            assert!(denied(&host, "vega.history.other.fake", "{}").await);
            let deimos = connect(&url, &denied_seed, "_INBOX_deimos").await;
            assert!(denied(&deimos, "vega.history.proteus.fake", "{}").await);
            assert!(
                denied(
                    &deimos,
                    "$JS.API.STREAM.MSG.GET.VEGA_HISTORY",
                    "{\"seq\":1}"
                )
                .await
            );
            assert!(denied(&deimos, "vega.inbox.proteus.p.deimos", "{}").await);
            let host_js = async_nats::jetstream::new(host);
            let spool = dir.path().join("outbox");
            let e = outbox::event(
                "proteus",
                "spawn",
                json!({"brief":"full brief","tier":"balanced","model":"test"}),
            );
            outbox::enqueue(&spool, &e).unwrap();
            // These sort ahead of the valid receipt. Both parse failures and
            // unreadable receipt shapes must be quarantined, not poison drain.
            let corrupt = spool.join("000-corrupt.json");
            std::fs::write(&corrupt, b"{truncated").unwrap();
            let unreadable = spool.join("000-directory.json");
            std::fs::create_dir(&unreadable).unwrap();
            let invalid = spool.join("000-invalid-envelope.json");
            std::fs::write(&invalid, b"{}").unwrap();
            drain(&host_js, &spool, "proteus").await.unwrap();
            assert!(outbox::pending(&spool).unwrap().is_empty());
            assert!(!corrupt.exists());
            assert!(!unreadable.exists());
            assert!(!invalid.exists());
            let quarantined: Vec<_> = std::fs::read_dir(&spool)
                .unwrap()
                .map(|f| f.unwrap().path())
                .filter(|p| {
                    p.extension()
                        .is_some_and(|e| e.to_string_lossy().starts_with("corrupt-"))
                })
                .collect();
            assert_eq!(quarantined.len(), 3);
            let preserved = quarantined
                .iter()
                .find(|p| p.file_stem().unwrap() == "000-corrupt")
                .unwrap();
            assert_eq!(std::fs::read(preserved).unwrap(), b"{truncated");
            drain(&host_js, &spool, "proteus").await.unwrap();
            assert_eq!(
                std::fs::read_dir(&spool).unwrap().count(),
                quarantined.len() + 1
            ); // lock only
               // Simulate crash after broker ack but before local unlink, beyond
               // the server's duplicate window. Subject CAS still keeps one event.
            tokio::time::sleep(Duration::from_millis(150)).await;
            outbox::enqueue(&spool, &e).unwrap();
            drain(&host_js, &spool, "proteus").await.unwrap();
            assert_eq!(stream.info().await.unwrap().state.messages, 1);
            let viewer = connect(&url, &viewer_seed, crate::VIEWER_INBOX).await;
            let read = async_nats::jetstream::new(viewer.clone())
                .get_stream(STREAM)
                .await
                .unwrap()
                .get_raw_message(1)
                .await
                .unwrap();
            assert_eq!(
                serde_json::from_slice::<Value>(&read.payload).unwrap()["data"]["brief"],
                "full brief"
            );
            assert!(denied(&viewer, "vega.history.proteus.fake", "{}").await);
            assert!(denied(&viewer, "$JS.API.STREAM.UPDATE.VEGA_HISTORY", "{}").await);
            // Broker outage: accepted receipts survive a failed bounded drain
            // and publish once when a new worker connection reaches the store.
            drop(_server);
            let pending = outbox::event(
                "proteus",
                "mail.local",
                json!({"pane":"p","body":"during outage"}),
            );
            outbox::enqueue(&spool, &pending).unwrap();
            assert!(drain(&host_js, &spool, "proteus").await.is_err());
            assert_eq!(outbox::pending(&spool).unwrap().len(), 1);
            let (_restarted, url) = start(dir.path(), &authz);
            let recovered = connect(&url, &host_seed, "_INBOX_proteus").await;
            drain(&async_nats::jetstream::new(recovered), &spool, "proteus")
                .await
                .unwrap();
            assert!(outbox::pending(&spool).unwrap().is_empty());
            let admin = connect(&url, &admin_seed, crate::ADMIN_INBOX).await;
            let mut recovered = async_nats::jetstream::new(admin)
                .get_stream(STREAM)
                .await
                .unwrap();
            assert_eq!(recovered.info().await.unwrap().state.messages, 2);
            let deimos = connect(&url, &denied_seed, "_INBOX_deimos").await;
            let private = outbox::event("deimos", "mail.local", json!({"body":"local history"}));
            outbox::enqueue(&spool, &private).unwrap();
            drain(&async_nats::jetstream::new(deimos), &spool, "deimos")
                .await
                .unwrap();
            assert!(outbox::pending(&spool).unwrap().is_empty());
            assert_eq!(recovered.info().await.unwrap().state.messages, 3);
        });
    }
}
