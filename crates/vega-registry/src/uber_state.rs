//! The Uber-UAC claim on its own host (docs/design/uber-uac.md §2,
//! vega-lhc.209.2): `$XDG_STATE_HOME/vega/uber/claim.json` is the truth,
//! `<host>.uber.v1` its registry copy. `vega uber claim|release` write both;
//! vegad keeps the key equal to the file (`local_entries`).

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

/// `$XDG_STATE_HOME/vega/uber/claim.json` (else `~/.local/state/...`).
pub fn file() -> Option<PathBuf> {
    let any = vega_herdr::state::state_file_on("uber", None, "x")?;
    Some(any.parent()?.join("claim.json"))
}

/// The registry value of a claim: `{schema: 1, pane, host, at, epoch}`.
pub fn value(pane: &str, host: &str, at: i64, epoch: u64) -> Value {
    json!({"schema": 1, "pane": pane, "host": host, "at": at, "epoch": epoch})
}

/// The claim in `file`, if it is a schema-1 claim of `host`.
pub fn local(file: &Path, host: &str) -> Option<Value> {
    let text = std::fs::read_to_string(file).ok()?;
    let v: Value = serde_json::from_str(&text).ok()?;
    let valid = v["schema"] == 1
        && v["pane"].is_string()
        && v["at"].is_i64()
        && v["epoch"].is_u64()
        && v["host"].as_str().map(crate::token) == Some(crate::token(host));
    valid.then_some(v)
}

/// Writes the claim file atomically.
pub fn save(file: &Path, claim: &Value) -> std::io::Result<()> {
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = file.with_extension(format!("tmp{}", std::process::id()));
    std::fs::write(&tmp, claim.to_string())?;
    std::fs::rename(tmp, file)
}

/// Removes the claim file; a missing file is fine.
pub fn remove(file: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(file) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

/// Puts this host's key now, so other hosts see the claim before vegad's
/// next reconcile.
pub async fn publish(
    kv: &async_nats::jetstream::kv::Store,
    host: &str,
    claim: &Value,
) -> Result<(), String> {
    kv.put(crate::uber_key(host), claim.to_string().into())
        .await
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// Deletes this host's key.
pub async fn unpublish(kv: &async_nats::jetstream::kv::Store, host: &str) -> Result<(), String> {
    kv.delete(crate::uber_key(host))
        .await
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claim_file_round_trip_accepts_only_this_hosts_schema_1_claim() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("uber/claim.json");
        assert_eq!(local(&file, "ganymede"), None);
        let claim = value("w1:p1", "ganymede", 1, 1000);
        save(&file, &claim).unwrap();
        assert_eq!(local(&file, "ganymede"), Some(claim));
        assert_eq!(local(&file, "proteus"), None);
        save(
            &file,
            &json!({"schema": 2, "pane": "w1:p1", "host": "ganymede", "at": 1, "epoch": 1}),
        )
        .unwrap();
        assert_eq!(local(&file, "ganymede"), None);
        remove(&file).unwrap();
        remove(&file).unwrap();
        assert!(!file.exists());
    }
}
