//! README search (docs/design/janitor.md, README search): words through
//! FTS5, always offline; meaning through vectors from an OpenAI-compatible
//! endpoint (for example llama.cpp), an optional stage `vega janitor embed`
//! fills.

use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs;
use std::path::Path;
use std::time::Duration;

use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::remote;
use crate::scan::{Entry, Kind};

/// The part of a README that is indexed: its start says what a repo is for.
pub const README_BYTES: usize = 8 * 1024;
const DEFAULT_MODEL: &str = "qwen3-embedding-0.6b-q8-0";
/// The vega secret holding the endpoint's key, for tool `janitor`.
pub const SECRET: &str = "triton-embedding";
/// Qwen3-Embedding is instruction-tuned: the query carries the task, the
/// documents do not; both get the suffix (as brainz scripts/vault-embed.py).
const QUERY_INSTRUCT: &str = "Instruct: Given a description of what a software project does, retrieve the repositories whose README matches it\nQuery: ";
const INPUT_SUFFIX: &str = "<|endoftext|>";
const BATCH: usize = 16;
/// Reciprocal rank fusion constant (as brainz `hybrid`).
const RRF_K: f64 = 60.0;

pub fn create(db: &Connection) -> rusqlite::Result<()> {
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS readme (
           path TEXT PRIMARY KEY, sha TEXT NOT NULL, text TEXT NOT NULL, vec_key TEXT);
         CREATE VIRTUAL TABLE IF NOT EXISTS readme_fts USING fts5(path UNINDEXED, text);
         CREATE TABLE IF NOT EXISTS vector (key TEXT PRIMARY KEY, vec BLOB NOT NULL);",
    )
}

fn sha(parts: &[&str]) -> String {
    let mut h = Sha256::new();
    for p in parts {
        h.update(p.as_bytes());
        h.update([0]);
    }
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// The README of a checkout (`README.md`, else any `README*` file), its
/// first `README_BYTES`; empty when there is none.
pub fn readme(dir: &Path) -> String {
    let named = dir.join("README.md");
    let path = if named.is_file() {
        Some(named)
    } else {
        fs::read_dir(dir).ok().and_then(|rd| {
            let mut found: Vec<_> = rd
                .flatten()
                .map(|e| e.path())
                .filter(|p| {
                    p.is_file()
                        && p.file_name()
                            .and_then(|n| n.to_str())
                            .is_some_and(|n| n.to_ascii_lowercase().starts_with("readme"))
                })
                .collect();
            found.sort();
            found.into_iter().next()
        })
    };
    let Some(bytes) = path.and_then(|p| fs::read(p).ok()) else {
        return String::new();
    };
    let mut text = String::from_utf8_lossy(&bytes[..bytes.len().min(README_BYTES)]).into_owned();
    // A cut inside a multi-byte character leaves a replacement mark.
    if text.ends_with('\u{FFFD}') {
        text.pop();
    }
    text
}

/// What is indexed for a clone: its name (remote or directory) and README.
fn document(e: &Entry) -> Option<String> {
    let text = readme(Path::new(&e.path));
    if text.trim().is_empty() {
        return None;
    }
    let name = e
        .remote
        .as_deref()
        .map(remote::display)
        .unwrap_or_else(|| e.path.rsplit('/').next().unwrap_or_default());
    Some(format!("{name}\n\n{text}"))
}

/// Re-reads the READMEs of this host's clones that `changed` names or that
/// have no row yet, and drops the rows of clones that are gone.
pub fn sync(db: &Connection, entries: &[Entry], changed: &BTreeSet<&str>) -> rusqlite::Result<()> {
    let known: BTreeMap<String, String> = {
        let mut stmt = db.prepare("SELECT path, sha FROM readme")?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    let clones: Vec<&Entry> = entries
        .iter()
        .filter(|e| e.kind == Kind::Clone && !e.prunable && e.host.is_none())
        .collect();
    let live: BTreeSet<&str> = clones.iter().map(|e| e.path.as_str()).collect();
    for p in known.keys().filter(|p| !live.contains(p.as_str())) {
        db.execute("DELETE FROM readme WHERE path = ?1", [p])?;
        db.execute("DELETE FROM readme_fts WHERE path = ?1", [p])?;
    }
    for e in clones {
        if known.contains_key(&e.path) && !changed.contains(e.path.as_str()) {
            continue;
        }
        // No README: an empty row, so it is not read again until the clone moves.
        let text = document(e).unwrap_or_default();
        let sha = sha(&[&text]);
        if known.get(&e.path) == Some(&sha) {
            continue;
        }
        db.execute(
            "INSERT OR REPLACE INTO readme (path, sha, text, vec_key) VALUES (?1, ?2, ?3, NULL)",
            params![e.path, sha, text],
        )?;
        db.execute("DELETE FROM readme_fts WHERE path = ?1", [&e.path])?;
        if !text.is_empty() {
            db.execute(
                "INSERT INTO readme_fts (path, text) VALUES (?1, ?2)",
                params![e.path, text],
            )?;
        }
    }
    Ok(())
}

/// The words of `q` as an FTS5 query: each quoted (no operators), any may match.
fn fts_query(q: &str) -> String {
    q.split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(|w| format!("\"{w}\""))
        .collect::<Vec<_>>()
        .join(" OR ")
}

/// Paths whose README matches the words of `q`, best first (bm25).
pub fn words(db: &Connection, q: &str, limit: usize) -> rusqlite::Result<Vec<String>> {
    let fq = fts_query(q);
    if fq.is_empty() {
        return Ok(Vec::new());
    }
    let mut stmt =
        db.prepare("SELECT path FROM readme_fts WHERE readme_fts MATCH ?1 ORDER BY rank LIMIT ?2")?;
    let rows = stmt.query_map(params![fq, limit as i64], |r| r.get(0))?;
    rows.collect()
}

fn to_blob(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

fn from_blob(b: &[u8]) -> Vec<f32> {
    b.chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}

fn unit(mut v: Vec<f32>) -> Vec<f32> {
    let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if n > 0.0 {
        v.iter_mut().for_each(|x| *x /= n);
    }
    v
}

/// Paths by cosine similarity to `q` (a unit vector), best first.
pub fn meaning(db: &Connection, q: &[f32], limit: usize) -> rusqlite::Result<Vec<(String, f32)>> {
    let mut stmt =
        db.prepare("SELECT r.path, v.vec FROM readme r JOIN vector v ON v.key = r.vec_key")?;
    let rows = stmt.query_map([], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, Vec<u8>>(1)?))
    })?;
    let mut out: Vec<(String, f32)> = rows
        .filter_map(Result::ok)
        .map(|(p, b)| {
            let v = from_blob(&b);
            (p, v.iter().zip(q).map(|(a, b)| a * b).sum())
        })
        .collect();
    out.sort_by(|a, b| b.1.total_cmp(&a.1));
    out.truncate(limit);
    Ok(out)
}

/// Reciprocal rank fusion of ranked path lists, normalised so a path ranked
/// first by every list scores 1.0.
pub fn fuse(legs: &[Vec<String>]) -> Vec<(String, f64)> {
    let mut score: BTreeMap<&str, f64> = BTreeMap::new();
    let used = legs.iter().filter(|l| !l.is_empty()).count();
    for leg in legs {
        for (rank, p) in leg.iter().enumerate() {
            *score.entry(p).or_default() += 1.0 / (RRF_K + rank as f64 + 1.0);
        }
    }
    let best = used.max(1) as f64 / (RRF_K + 1.0);
    let mut out: Vec<(String, f64)> = score
        .into_iter()
        .map(|(p, s)| (p.to_string(), s / best))
        .collect();
    out.sort_by(|a, b| b.1.total_cmp(&a.1));
    out
}

/// The embedding endpoint: URL, model and key.
pub struct Endpoint {
    pub url: String,
    pub model: String,
    key: vega_secrets::Secret,
}

fn var(k: &str, default: &str) -> String {
    env::var(k)
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or(default.into())
}

/// `VEGA_JANITOR_EMBED_MODEL`, default qwen3-embedding.
pub fn model() -> String {
    var("VEGA_JANITOR_EMBED_MODEL", DEFAULT_MODEL)
}

impl Endpoint {
    /// `VEGA_JANITOR_EMBED_ENDPOINT` or `janitor-embed.url`, `model()`, the key
    /// from vega secrets.
    pub fn from_env() -> Result<Self, vega_secrets::Error> {
        Ok(Self {
            url: vega_herdr::service_config::get(
                "VEGA_JANITOR_EMBED_ENDPOINT",
                "janitor-embed.url",
            )
            .ok_or_else(|| {
                vega_secrets::Error::new(
                    "endpoint_unconfigured",
                    "no embedding endpoint configured",
                    "set VEGA_JANITOR_EMBED_ENDPOINT or ~/.config/vega/janitor-embed.url",
                )
            })?,
            model: model(),
            key: vega_secrets::Store::from_env()?.get(SECRET, "janitor")?,
        })
    }

    /// Unit vectors for `texts`; the server gets `timeout` per batch.
    pub fn embed(&self, texts: &[String], timeout: Duration) -> Result<Vec<Vec<f32>>, String> {
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_secs(2))
            .timeout(timeout)
            .build();
        let url = format!("{}/embeddings", self.url.trim_end_matches('/'));
        let resp: Value = agent
            .post(&url)
            .set("Authorization", &format!("Bearer {}", self.key.expose()))
            .send_json(json!({"model": self.model, "input": texts}))
            .map_err(|e| match e {
                ureq::Error::Status(code, _) => format!("{url}: HTTP {code}"),
                ureq::Error::Transport(t) => format!("{url}: {}", t.kind()),
            })?
            .into_json()
            .map_err(|e| format!("{url}: {e}"))?;
        let data = resp["data"]
            .as_array()
            .ok_or_else(|| format!("{url}: no data in the answer"))?;
        let mut out = vec![Vec::new(); texts.len()];
        for d in data {
            let i = d["index"].as_u64().unwrap_or(0) as usize;
            let v: Vec<f32> = d["embedding"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|x| x.as_f64())
                        .map(|x| x as f32)
                        .collect()
                })
                .unwrap_or_default();
            if let Some(slot) = out.get_mut(i) {
                *slot = unit(v);
            }
        }
        if out.iter().any(Vec::is_empty) {
            return Err(format!("{url}: fewer vectors than texts"));
        }
        Ok(out)
    }

    /// The query's unit vector (with the instruction).
    pub fn query(&self, q: &str) -> Result<Vec<f32>, String> {
        let text = format!("{QUERY_INSTRUCT}{q}{INPUT_SUFFIX}");
        self.embed(&[text], Duration::from_secs(10))?
            .pop()
            .ok_or_else(|| "no vector".into())
    }
}

/// The content address of a README's vector: same model and text, same key,
/// so a moved or duplicated clone costs no call.
fn vec_key(model: &str, text: &str) -> String {
    sha(&[model, INPUT_SUFFIX, text])
}

/// What `embed` did.
#[derive(Debug, Default, PartialEq)]
pub struct Embedded {
    /// Embedding calls' texts (one per distinct README).
    pub new: usize,
    pub reused: usize,
    pub total: usize,
}

/// Gives every README a vector of `model`: reuses one with the same key,
/// sends the rest to `embed` in batches, committing each.
pub fn embed_all(
    db: &mut Connection,
    model: &str,
    mut embed: impl FnMut(&[String]) -> Result<Vec<Vec<f32>>, String>,
) -> Result<Embedded, String> {
    let e = |e: rusqlite::Error| e.to_string();
    let rows: Vec<(String, String, Option<String>)> = {
        let mut stmt = db
            .prepare("SELECT path, text, vec_key FROM readme WHERE text != ''")
            .map_err(e)?;
        let rows = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .map_err(e)?;
        rows.collect::<rusqlite::Result<_>>().map_err(e)?
    };
    let mut done = Embedded {
        total: rows.len(),
        ..Default::default()
    };
    // key -> (text, paths): clones of one repo share a README and a call.
    let mut todo: BTreeMap<String, (String, Vec<String>)> = BTreeMap::new();
    for (path, text, old) in rows {
        let key = vec_key(model, &text);
        let have: Option<i64> = db
            .query_row("SELECT 1 FROM vector WHERE key = ?1", [&key], |r| r.get(0))
            .optional()
            .map_err(e)?;
        if have.is_some() {
            if old.as_deref() != Some(&key) {
                db.execute(
                    "UPDATE readme SET vec_key = ?1 WHERE path = ?2",
                    [&key, &path],
                )
                .map_err(e)?;
                done.reused += 1;
            }
        } else {
            todo.entry(key).or_insert((text, Vec::new())).1.push(path);
        }
    }
    let todo: Vec<(String, (String, Vec<String>))> = todo.into_iter().collect();
    for batch in todo.chunks(BATCH) {
        let texts: Vec<String> = batch
            .iter()
            .map(|(_, (t, _))| format!("{t}{INPUT_SUFFIX}"))
            .collect();
        let vecs = embed(&texts)?;
        let tx = db.transaction().map_err(e)?;
        for ((key, (_, paths)), v) in batch.iter().zip(vecs) {
            tx.execute(
                "INSERT OR REPLACE INTO vector (key, vec) VALUES (?1, ?2)",
                params![key, to_blob(&v)],
            )
            .map_err(e)?;
            for path in paths {
                tx.execute(
                    "UPDATE readme SET vec_key = ?1 WHERE path = ?2",
                    [key, path],
                )
                .map_err(e)?;
            }
        }
        tx.commit().map_err(e)?;
        done.new += batch.len();
    }
    // Vectors no README points to (moved on, other model) are dropped.
    db.execute(
        "DELETE FROM vector WHERE key NOT IN (SELECT vec_key FROM readme WHERE vec_key IS NOT NULL)",
        [],
    )
    .map_err(e)?;
    Ok(done)
}

/// READMEs with a vector of `model`.
pub fn vectors_of(db: &Connection, model: &str) -> usize {
    let Ok(mut stmt) = db.prepare("SELECT text, vec_key FROM readme WHERE vec_key IS NOT NULL")
    else {
        return 0;
    };
    stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
        .map(|rows| {
            rows.filter_map(Result::ok)
                .filter(|(t, k)| vec_key(model, t) == *k)
                .count()
        })
        .unwrap_or(0)
}

/// The first README line that says something (no heading, badge, HTML,
/// code fence, table or rule), at most 80 characters.
pub fn about(db: &Connection, path: &str) -> String {
    let text: String = db
        .query_row("SELECT text FROM readme WHERE path = ?1", [path], |r| {
            r.get(0)
        })
        .unwrap_or_default();
    text.lines()
        .skip(2) // the name line and the blank after it
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.starts_with(['#', '[', '<', '!', '`', '|', '-', '=']))
        .map(|l| l.chars().take(80).collect())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedding_requires_explicit_endpoint() {
        const FIXTURE: &str = "VEGA_ENDPOINT_CONFIG_FIXTURE";
        if std::env::var_os(FIXTURE).is_none() {
            let root = tempfile::tempdir().unwrap();
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "search::tests::embedding_requires_explicit_endpoint",
                    "--nocapture",
                ])
                .env(FIXTURE, "1")
                .env("HOME", root.path())
                .env("XDG_CONFIG_HOME", root.path().join("config"))
                .env("XDG_STATE_HOME", root.path().join("state"))
                .env_remove("VEGA_JANITOR_EMBED_ENDPOINT")
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            return;
        }
        let error = Endpoint::from_env()
            .err()
            .expect("endpoint must be required");
        assert_eq!(error.code, "endpoint_unconfigured");
        let store = vega_secrets::Store::from_env().unwrap();
        let (public, _) = store.host_init().unwrap();
        let mut manifest = vega_secrets::Manifest::default();
        manifest.set_host(&store.host, &public);
        let mut entry = vega_secrets::Entry {
            name: SECRET.into(),
            env: "EMBED_TEST_KEY".into(),
            tools: vec!["janitor".into()],
            hosts: vec![store.host.clone()],
            ..Default::default()
        };
        entry.encrypted_to = store
            .encrypt(
                &manifest,
                &entry,
                &vega_secrets::Secret::new("test-key".into()),
            )
            .unwrap();
        manifest.set_entry(&entry);
        store.save_manifest(&manifest).unwrap();
        std::fs::write(
            store.config.join("janitor-embed.url"),
            "https://embedding.example/v1\nignored\n",
        )
        .unwrap();
        assert_eq!(
            Endpoint::from_env().unwrap().url,
            "https://embedding.example/v1"
        );
        std::env::set_var("VEGA_JANITOR_EMBED_ENDPOINT", "https://override.example/v1");
        assert_eq!(
            Endpoint::from_env().unwrap().url,
            "https://override.example/v1"
        );
    }

    #[test]
    fn words_become_a_safe_any_query() {
        assert_eq!(
            fts_query("git map, cleanup!"),
            "\"git\" OR \"map\" OR \"cleanup\""
        );
        assert_eq!(fts_query("-- *"), "");
    }

    #[test]
    fn fusion_rewards_agreement() {
        let a = vec!["x".to_string(), "y".to_string()];
        let b = vec!["x".to_string(), "z".to_string()];
        let f = fuse(&[a, b]);
        assert_eq!(f[0].0, "x");
        assert!((f[0].1 - 1.0).abs() < 1e-9);
        assert!(f[1].1 < 0.6);
    }

    #[test]
    fn blobs_round_trip() {
        let v = unit(vec![3.0, 4.0]);
        assert_eq!(from_blob(&to_blob(&v)), vec![0.6, 0.8]);
    }
}
