//! vegad's catalog thread (docs/design/lanista-catalog.md). On the builder
//! host (ganymede): `lanista fetch` once a day with the lanista secrets, then
//! a put of `model_index.json` into the broker's object store. On every other
//! host: a look at the object's metadata every 15 min and, when the bucket's
//! index is newer than the local one, a download into lanista's cache.
//!
//! Readers use direct gets only (`$JS.API.DIRECT.GET.OBJ_VEGA_LANISTA...`),
//! never a consumer: a consumer create right would let a host point a push
//! consumer's `deliver_subject` at another host's inbox (broker spike), and
//! the inbox-only host deimos reads the catalog too. The state file
//! (`vega_herdr::catalog::State`) is what `vega lanista` shows.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::Duration;

use async_nats::jetstream::{self, object_store};
use base64::engine::general_purpose::URL_SAFE;
use base64::Engine;
use sha2::{Digest, Sha256};
use vega_herdr::catalog::{self as cat, State};

use crate::Seed;

/// How often the thread looks; fetch and check have their own clocks.
const EVERY: Duration = Duration::from_secs(60);

/// `lanista fetch` downloads eight sources; a hung one is killed.
const FETCH_TIMEOUT: Duration = Duration::from_secs(15 * 60);

pub fn spawn() {
    thread::spawn(|| {
        let mut said = String::new();
        loop {
            let line = tick(
                &crate::host(),
                &cat::builder(),
                &cat::state_path(),
                &cat::index_path(),
                vega_bd_sync::unix_now(),
            );
            if !line.is_empty() && line != said {
                crate::log(&line);
                said = line;
            }
            thread::sleep(EVERY);
        }
    });
}

/// One step; returns a log line (logged when it changes).
pub fn tick(host: &str, builder: &str, state: &Path, index: &Path, now: i64) -> String {
    let mut st = State::load(state).unwrap_or_default();
    let line = if host == builder {
        build(&mut st, host, index, now)
    } else {
        receive(&mut st, index, now)
    };
    if let Err(e) = st.save(state) {
        return format!("catalog: state {}: {e}", state.display());
    }
    line
}

/// The builder: fetch when due, publish an index the bucket lacks. The
/// index (23 MB) is read after a fetch and on the check clock, so a failed
/// put retries every 15 min, not every tick.
fn build(st: &mut State, host: &str, index: &Path, now: i64) -> String {
    let mut line = String::new();
    let fetched = cat::fetch_due(st, now);
    if fetched {
        st.fetched_at = now;
        st.fetch = match fetch() {
            Ok(note) => {
                line = format!("catalog: fetched{note}");
                "ok".into()
            }
            Err(e) => {
                line = format!("catalog: fetch failed: {e}");
                format!("error: {e}")
            }
        };
    }
    if !fetched && now - st.checked_at < cat::CHECK_EVERY {
        return line;
    }
    st.checked_at = now;
    let Ok(bytes) = std::fs::read(index) else {
        return format!("catalog: no index at {} to publish", index.display());
    };
    let Some(generated) = cat::generated_at(&bytes) else {
        return format!("catalog: {} has no generated_at", index.display());
    };
    if st.published_at > 0 && st.generated_at == generated {
        return line;
    }
    match block_on(async {
        let client = crate::connect_as("catalog", Seed::Host, None)
            .await
            .map_err(|f| f.message)?;
        put(&client, &bytes, &generated, host).await
    }) {
        Ok(()) => {
            st.check = "ok".into();
            st.source = host.into();
            st.generated_at = generated.clone();
            st.published_at = now;
            format!("catalog: published {generated}")
        }
        Err(e) => {
            st.check = format!("error: {e}");
            format!("catalog: publish failed: {e}")
        }
    }
}

/// Every other host: copy the bucket's index when it is newer.
fn receive(st: &mut State, index: &Path, now: i64) -> String {
    if now - st.checked_at < cat::CHECK_EVERY {
        return String::new();
    }
    st.checked_at = now;
    let local = std::fs::read(index)
        .ok()
        .and_then(|b| cat::generated_at(&b));
    let got = block_on(async {
        let client = crate::connect_as("catalog", Seed::Host, None)
            .await
            .map_err(|f| f.message)?;
        let Some(info) = info(&client).await? else {
            return Ok(None);
        };
        let remote = info.metadata.get(cat::META_GENERATED).cloned();
        if !cat::newer(remote.as_deref().unwrap_or(""), local.as_deref()) {
            return Ok(Some((info, None)));
        }
        let bytes = download(&client, &info).await?;
        Ok(Some((info, Some(bytes))))
    });
    match got {
        Err(e) => {
            st.check = format!("error: {e}");
            format!("catalog: check failed: {e}")
        }
        Ok(None) => {
            st.check = "error: no catalog in the bucket".into();
            "catalog: no catalog in the bucket yet".into()
        }
        Ok(Some((_, None))) => {
            st.check = "ok".into();
            "catalog: current".into()
        }
        Ok(Some((info, Some(bytes)))) => match write_index(index, &bytes) {
            Ok(generated) => {
                st.check = "ok".into();
                st.source = info
                    .metadata
                    .get(cat::META_SOURCE)
                    .cloned()
                    .unwrap_or_default();
                st.generated_at = generated.clone();
                st.received_at = now;
                format!("catalog: received {generated} from {}", st.source)
            }
            Err(e) => {
                st.check = format!("error: {e}");
                format!("catalog: write failed: {e}")
            }
        },
    }
}

fn block_on<T>(f: impl std::future::Future<Output = Result<T, String>>) -> Result<T, String> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?
        .block_on(f)
}

/// `lanista fetch` with the secrets `vega secrets run --for lanista` would
/// give it. A secret this host cannot decrypt is skipped (lanista then skips
/// that source); the note names it.
fn fetch() -> Result<String, String> {
    let store = vega_secrets::Store::from_env().map_err(|e| e.message)?;
    let (vars, skipped) = store.env_for("lanista").map_err(|e| e.message)?;
    let mut c = Command::new(lanista_bin());
    c.arg("fetch");
    for (k, v) in &vars {
        c.env(k, v.expose());
    }
    let out = vega_herdr::output_within(&mut c, FETCH_TIMEOUT)
        .map_err(|e| format!("lanista: {e}"))?
        .ok_or("lanista fetch timed out")?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        let last = err.lines().rev().find(|l| !l.trim().is_empty());
        return Err(format!(
            "lanista fetch: {} {}",
            out.status,
            last.unwrap_or("").trim()
        ));
    }
    Ok(if skipped.is_empty() {
        String::new()
    } else {
        let names: Vec<&str> = skipped.iter().map(|e| e.message.as_str()).collect();
        format!(" (secrets skipped: {})", names.join("; "))
    })
}

/// vegad's PATH comes from herdr; uv puts lanista into `~/.local/bin`.
fn lanista_bin() -> PathBuf {
    std::env::var_os("PATH")
        .and_then(|p| {
            std::env::split_paths(&p)
                .map(|d| d.join("lanista"))
                .find(|f| f.is_file())
        })
        .unwrap_or_else(|| {
            Path::new(&std::env::var("HOME").unwrap_or_default()).join(".local/bin/lanista")
        })
}

/// Puts the index with its `generated_at` and source host as object
/// metadata; the object store replaces the previous one.
pub async fn put(
    client: &async_nats::Client,
    bytes: &[u8],
    generated: &str,
    source: &str,
) -> Result<(), String> {
    let js = jetstream::new(client.clone());
    let store = js
        .get_object_store(cat::BUCKET)
        .await
        .map_err(|e| format!("bucket {}: {e}", cat::BUCKET))?;
    let meta = object_store::ObjectMetadata {
        name: cat::OBJECT.into(),
        metadata: HashMap::from([
            (cat::META_GENERATED.to_string(), generated.to_string()),
            (cat::META_SOURCE.to_string(), source.to_string()),
        ]),
        ..Default::default()
    };
    store
        .put(meta, &mut &bytes[..])
        .await
        .map_err(|e| format!("put: {e}"))?;
    Ok(())
}

fn stream() -> String {
    format!("OBJ_{}", cat::BUCKET)
}

/// The direct-get subjects a reader needs (auth.rs grants exactly these).
pub fn read_subjects() -> [String; 2] {
    let api = format!("$JS.API.DIRECT.GET.{}", stream());
    [api.clone(), format!("{api}.>")]
}

/// What the builder needs for `put`: open the bucket (stream info), read the
/// previous info (message get), publish chunks and info, purge old chunks.
pub fn write_subjects() -> [String; 4] {
    let s = stream();
    [
        format!("$JS.API.STREAM.INFO.{s}"),
        format!("$JS.API.STREAM.MSG.GET.{s}"),
        format!("$JS.API.STREAM.PURGE.{s}"),
        format!("$O.{}.>", cat::BUCKET),
    ]
}

/// The admin's create right (`vega post provision lanista`, while
/// `auth-config --create lanista`) and its steady stream-info right.
pub fn create_subject() -> String {
    format!("$JS.API.STREAM.CREATE.{}", stream())
}

pub fn info_subject() -> String {
    format!("$JS.API.STREAM.INFO.{}", stream())
}

/// The bucket as `vega post provision lanista` creates it: twice the index
/// fits (a put writes the new chunks before it purges the old ones).
pub fn bucket_config() -> object_store::Config {
    object_store::Config {
        bucket: cat::BUCKET.into(),
        description: Some("lanista model catalog (vega, docs/design/lanista-catalog.md)".into()),
        max_bytes: 128 << 20,
        compression: true,
        ..Default::default()
    }
}

/// The object's info by one last-by-subject direct get; `None` when the
/// bucket has no (or a deleted) catalog.
pub async fn info(client: &async_nats::Client) -> Result<Option<object_store::ObjectInfo>, String> {
    let subject = format!(
        "$JS.API.DIRECT.GET.{}.$O.{}.M.{}",
        stream(),
        cat::BUCKET,
        URL_SAFE.encode(cat::OBJECT)
    );
    let msg = client
        .request(subject, "".into())
        .await
        .map_err(|e| format!("catalog info: {e}"))?;
    if msg.status == Some(async_nats::StatusCode::NOT_FOUND) {
        return Ok(None);
    }
    if let Some(s) = msg.status.filter(|s| !s.is_success()) {
        return Err(format!("catalog info: status {s}"));
    }
    let info: object_store::ObjectInfo =
        serde_json::from_slice(&msg.payload).map_err(|e| format!("catalog info: {e}"))?;
    Ok((!info.deleted).then_some(info))
}

/// The object's chunks, one `next_by_subj` direct get each, checked against
/// the info's size and SHA-256 digest.
pub async fn download(
    client: &async_nats::Client,
    info: &object_store::ObjectInfo,
) -> Result<Vec<u8>, String> {
    let api = format!("$JS.API.DIRECT.GET.{}", stream());
    let chunks = format!("$O.{}.C.{}", cat::BUCKET, info.nuid);
    let mut out = Vec::with_capacity(info.size);
    let mut seq = 1u64;
    for n in 0..info.chunks {
        let req = serde_json::json!({"seq": seq, "next_by_subj": chunks}).to_string();
        let msg = client
            .request(api.clone(), req.into())
            .await
            .map_err(|e| format!("chunk {n}: {e}"))?;
        if let Some(s) = msg.status.filter(|s| !s.is_success()) {
            return Err(format!("chunk {n}: status {s} (replaced during download?)"));
        }
        let got = msg
            .headers
            .as_ref()
            .and_then(|h| h.get("Nats-Sequence"))
            .and_then(|v| v.as_str().parse::<u64>().ok())
            .ok_or(format!("chunk {n}: no Nats-Sequence"))?;
        out.extend_from_slice(&msg.payload);
        seq = got + 1;
    }
    if out.len() != info.size {
        return Err(format!("size {} != {}", out.len(), info.size));
    }
    let digest = format!("SHA-256={}", URL_SAFE.encode(Sha256::digest(&out)));
    if info.digest.as_deref() != Some(digest.as_str()) {
        return Err("digest mismatch".into());
    }
    Ok(out)
}

/// Writes the index atomically next to lanista's (a running lanista never
/// reads half a file); returns its `generated_at`.
fn write_index(index: &Path, bytes: &[u8]) -> Result<String, String> {
    let generated = cat::generated_at(bytes).ok_or("not a lanista index")?;
    let dir = index.parent().ok_or("no cache dir")?;
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let tmp = index.with_extension("json.vega-tmp");
    std::fs::write(&tmp, bytes).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, index).map_err(|e| e.to_string())?;
    Ok(generated)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn receive_waits_for_the_check_clock() {
        let dir = tempfile::tempdir().unwrap();
        let mut st = State {
            checked_at: 1000,
            check: "ok".into(),
            ..Default::default()
        };
        let line = receive(
            &mut st,
            &dir.path().join("model_index.json"),
            1000 + cat::CHECK_EVERY - 1,
        );
        assert_eq!(line, "");
        assert_eq!(st.checked_at, 1000);
    }

    #[test]
    fn build_publishes_nothing_already_published() {
        let dir = tempfile::tempdir().unwrap();
        let index = dir.path().join("model_index.json");
        std::fs::write(&index, br#"{"generated_at": "g1", "models": {}}"#).unwrap();
        let mut st = State {
            fetched_at: 1000,
            fetch: "ok".into(),
            source: "ganymede".into(),
            generated_at: "g1".into(),
            published_at: 1000,
            ..Default::default()
        };
        // Within the check clock: the index is not even read.
        st.checked_at = 1500;
        let before = st.clone();
        assert_eq!(build(&mut st, "ganymede", &index, 2000), "");
        assert_eq!(st, before);
        // On the clock: read, already published, no put (no broker here).
        let now = 1500 + cat::CHECK_EVERY;
        assert_eq!(build(&mut st, "ganymede", &index, now), "");
        assert_eq!(st.checked_at, now);
        assert_eq!(st.published_at, 1000);
    }

    #[test]
    fn write_index_refuses_what_is_not_an_index() {
        let dir = tempfile::tempdir().unwrap();
        let index = dir.path().join("lanista/model_index.json");
        assert!(write_index(&index, b"<html>").is_err());
        assert!(!index.exists());
        assert_eq!(
            write_index(&index, br#"{"generated_at": "g2"}"#).unwrap(),
            "g2"
        );
        assert!(index.exists());
    }
}
