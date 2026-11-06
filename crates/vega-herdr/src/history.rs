//! Bounded transient outbox for the central history stream, not a local archive.
//! CLI producers write one receipt; daemon producers use a nonblocking channel.
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

pub const MAX_EVENTS: usize = 256;
pub const MAX_EVENT_BYTES: usize = 256 * 1024;
static SERIAL: AtomicU64 = AtomicU64::new(0);

pub fn root() -> Option<PathBuf> {
    std::env::var_os("XDG_STATE_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/state")))
        .map(|p| {
            p.join("vega/history-outbox")
                .join(crate::state::post_host())
        })
}

pub fn hash(text: &str) -> String {
    format!("{:x}", Sha256::digest(text.as_bytes()))
}

pub fn event(host: &str, kind: &str, data: Value) -> Value {
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let id = hash(&format!(
        "{host}:{}:{}:{}",
        ts.as_nanos(),
        std::process::id(),
        SERIAL.fetch_add(1, Ordering::Relaxed)
    ));
    let caller = std::env::var("HERDR_PANE_ID").ok();
    let pane = data
        .get("pane")
        .cloned()
        .or_else(|| caller.clone().map(Value::String));
    // A daemon may inherit harness variables from whoever installed it. They
    // identify an event only when its pane is explicitly the caller's pane.
    let session = data.get("session").cloned().or_else(|| {
        let caller = caller.as_deref()?;
        if pane.as_ref().and_then(Value::as_str) != Some(caller) {
            return None;
        }
        std::env::var("CODEX_THREAD_ID")
            .or_else(|_| std::env::var("CLAUDE_CODE_SESSION_ID"))
            .ok()
            .map(Value::String)
    });
    json!({"schema":1,"id":id,"ts":ts.as_millis() as u64,"host":host,"kind":kind,
        "project":data.get("project"),"bead":data.get("bead"),"pane":pane,"session":session,"data":data})
}

/// Resolve owner-local project metadata outside the relay loop. Remote cwd
/// strings are descriptive only and must never be resolved on this machine.
pub fn enrich(event: &mut Value) {
    if !event["project"].is_null() || event["data"]["target_host"].as_str().is_some() {
        return;
    }
    let Some(cwd) = event["data"]["cwd"]
        .as_str()
        .filter(|p| p.starts_with('/') || p.starts_with("~/"))
    else {
        return;
    };
    let path = if let Some(rest) = cwd.strip_prefix("~/") {
        let Some(home) = std::env::var_os("HOME") else {
            return;
        };
        PathBuf::from(home).join(rest)
    } else {
        PathBuf::from(cwd)
    };
    if let Ok(Some(project)) =
        crate::project::resolve_optional(&path, event["host"].as_str().unwrap_or(""))
    {
        event["project"] = json!(project.id);
    }
}

/// CLI capture is best effort: history failure never changes command success.
pub fn emit(kind: &str, data: Value) {
    let host = crate::state::post_host();
    let mut event = event(&host, kind, data);
    enrich(&mut event);
    let result = root()
        .ok_or_else(|| io::Error::other("no state directory"))
        .and_then(|dir| enqueue(&dir, &event));
    if let Err(e) = result {
        eprintln!("history: event not queued: {e}");
    }
}

/// Cross-process lock, waiting at most ~100 ms. Count and bytes are bounded; a full queue
/// rejects new events rather than deleting already accepted events.
pub fn enqueue(dir: &Path, event: &Value) -> io::Result<()> {
    let bytes = serde_json::to_vec(event)?;
    if bytes.len() > MAX_EVENT_BYTES {
        return Err(io::Error::other("event exceeds 256 KiB outbox limit"));
    }
    fs::create_dir_all(dir)?;
    fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(dir.join("lock"))?;
    // A fork elsewhere in this process briefly shares the previous call's lock
    // until its child execs (CLOEXEC); a few short retries ride that out, a
    // real holder still fails within ~100 ms (vega-0ckc).
    let mut tries = 0;
    while unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        tries += 1;
        if tries == 20 {
            return Err(io::Error::other("outbox busy; event not queued"));
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let id = event["id"]
        .as_str()
        .filter(|id| id.len() == 64 && id.bytes().all(|b| b.is_ascii_hexdigit()))
        .ok_or_else(|| io::Error::other("invalid event id"))?;
    let target = dir.join(format!("{id}.json"));
    if target.exists() {
        return Ok(());
    }
    if pending(dir)?.len() >= MAX_EVENTS {
        return Err(io::Error::other(
            "outbox full (256 events); event not queued",
        ));
    }
    // A single lock protects this temporary path. Startup retries overwrite a
    // partial temp file; only .json receipts are eligible for publication.
    let tmp = dir.join("pending.tmp");
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&tmp)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    fs::rename(tmp, target)?;
    File::open(dir)?.sync_all()?;
    Ok(())
}

pub fn pending(dir: &Path) -> io::Result<Vec<PathBuf>> {
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut files = Vec::new();
    for file in fs::read_dir(dir)? {
        let file = file?;
        if file.path().extension().is_some_and(|x| x == "json") {
            files.push(file.path());
        }
    }
    files.sort();
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;
    /// A process forked by a parallel test holds a just-closed lock fd until it execs, so
    /// the nonblocking lock can report busy for a moment; retry that only.
    fn retry_busy(mut op: impl FnMut() -> io::Result<()>) -> io::Result<()> {
        for _ in 0..100 {
            match op() {
                Err(e) if e.to_string().contains("outbox busy") => {
                    std::thread::sleep(std::time::Duration::from_millis(10))
                }
                r => return r,
            }
        }
        op()
    }
    #[test]
    fn bounded_durable_and_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        let e = event("h", "mail.sent", json!({"body":"full text"}));
        retry_busy(|| enqueue(tmp.path(), &e)).unwrap();
        retry_busy(|| enqueue(tmp.path(), &e)).unwrap();
        let files = pending(tmp.path()).unwrap();
        assert_eq!(files.len(), 1);
        let saved: Value = serde_json::from_slice(&fs::read(&files[0]).unwrap()).unwrap();
        assert_eq!(saved, e);
        // Fill the rest as receipts: 255 more enqueues cost two fsyncs each, seconds in all.
        for i in 1..MAX_EVENTS {
            let id = hash(&i.to_string());
            fs::write(tmp.path().join(format!("{id}.json")), "{}").unwrap();
        }
        let full = retry_busy(|| enqueue(tmp.path(), &event("h", "test", Value::Null)));
        assert!(full.unwrap_err().to_string().contains("outbox full"));
        assert_eq!(pending(tmp.path()).unwrap().len(), MAX_EVENTS);
        let huge = event("h", "test", json!({"body":"x".repeat(MAX_EVENT_BYTES)}));
        assert!(enqueue(tmp.path(), &huge).is_err());
    }
    #[test]
    fn lock_contention_does_not_wait() {
        let tmp = tempfile::tempdir().unwrap();
        let lock = File::create(tmp.path().join("lock")).unwrap();
        assert_eq!(unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX) }, 0);
        assert!(enqueue(tmp.path(), &event("h", "test", Value::Null)).is_err());
        assert!(pending(tmp.path()).unwrap().is_empty());
    }
}

/// Host-local evidence only. Unknown acknowledgements are never synthesized
/// from retries rejected by the subject compare-and-set or from local receipts.
#[derive(Default, Debug, serde::Serialize, serde::Deserialize)]
pub struct PublishStatus {
    pub last_acked_ms: Option<u64>,
    pub last_acked_seq: Option<u64>,
    pub last_error: Option<String>,
}

pub fn read_status(dir: &Path) -> io::Result<PublishStatus> {
    match fs::read(dir.join("publish.state")) {
        Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(PublishStatus::default()),
        Err(e) => Err(e),
    }
}

/// Written only by the history publisher thread, atomically outside live loops.
pub fn publish_status(dir: &Path, sequence: Option<u64>, error: Option<&str>) -> io::Result<()> {
    let mut state = read_status(dir)?;
    if let Some(seq) = sequence {
        state.last_acked_seq = Some(seq);
        state.last_acked_ms = Some(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64,
        );
    }
    state.last_error = error.map(str::to_owned);
    fs::create_dir_all(dir)?;
    let tmp = dir.join("publish.tmp");
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&tmp)?;
    file.write_all(&serde_json::to_vec(&state)?)?;
    file.sync_all()?;
    fs::rename(tmp, dir.join("publish.state"))?;
    File::open(dir)?.sync_all()
}

#[cfg(test)]
mod status_tests {
    use super::*;
    #[test]
    fn evidence_survives_errors_and_has_no_receipt_extension() {
        let dir = tempfile::tempdir().unwrap();
        assert!(read_status(dir.path()).unwrap().last_acked_seq.is_none());
        publish_status(dir.path(), None, Some("broker unreachable")).unwrap();
        assert!(read_status(dir.path()).unwrap().last_acked_seq.is_none());
        publish_status(dir.path(), Some(7), None).unwrap();
        let status = read_status(dir.path()).unwrap();
        assert_eq!(status.last_acked_seq, Some(7));
        assert!(status.last_acked_ms.is_some());
        publish_status(dir.path(), None, Some("publish failed")).unwrap();
        let failed = read_status(dir.path()).unwrap();
        assert_eq!(failed.last_acked_ms, status.last_acked_ms);
        assert_eq!(failed.last_acked_seq, Some(7));
        assert_eq!(failed.last_error.as_deref(), Some("publish failed"));
        assert!(pending(dir.path()).unwrap().is_empty());
        std::fs::write(dir.path().join("publish.state"), b"bad").unwrap();
        assert!(read_status(dir.path()).is_err());
    }
}
