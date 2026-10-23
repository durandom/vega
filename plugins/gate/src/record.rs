//! Review records: one bead per segment review, label `gate`, external ref
//! `gate:<key>` (gate.md section 6).

use std::path::Path;
use std::time::Duration;

use serde_json::{json, Value};

use crate::{run_ok, tool};

pub const LABEL: &str = "gate";
/// bd slower than this fails the gate open (gate.md section 1).
pub const BD_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone)]
pub struct Bead {
    pub id: String,
    pub title: String,
    pub status: String,
    pub meta: Value,
    pub created_at: String,
    pub close_reason: String,
}

impl Bead {
    pub fn verdict(&self) -> &str {
        self.meta
            .get("verdict")
            .and_then(Value::as_str)
            .unwrap_or("-")
    }
    pub fn meta_str(&self, k: &str) -> &str {
        self.meta.get(k).and_then(Value::as_str).unwrap_or("")
    }
    pub fn is_pass(&self) -> bool {
        self.status == "closed" && self.verdict() == "pass"
    }
    pub fn is_running(&self) -> bool {
        self.status != "closed" && self.verdict() == "running"
    }
}

fn bd(dir: &Path, args: &[&str]) -> Result<String, String> {
    run_ok(&tool("bd"), args, Some(dir), Some(BD_TIMEOUT))
}

fn parse_list(out: &str) -> Result<Vec<Bead>, String> {
    let v: Value = serde_json::from_str(&vega_plugin::unwrap_bd_envelope(out))
        .map_err(|e| format!("bd list: not JSON ({e})"))?;
    let s = |i: &Value, k: &str| i.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    Ok(v.as_array()
        .map(|a| {
            a.iter()
                .map(|i| Bead {
                    id: s(i, "id"),
                    title: s(i, "title"),
                    status: s(i, "status"),
                    meta: i.get("metadata").cloned().unwrap_or(Value::Null),
                    created_at: s(i, "created_at"),
                    close_reason: s(i, "close_reason"),
                })
                .collect()
        })
        .unwrap_or_default())
}

/// Every gate bead with this key.
pub fn lookup(dir: &Path, key: &str) -> Result<Vec<Bead>, String> {
    let r = format!("gate:{key}");
    parse_list(&bd(
        dir,
        &[
            "list",
            "--external-ref",
            &r,
            "--all",
            "--json",
            "--limit",
            "0",
        ],
    )?)
}

/// Every gate bead of the repo.
pub fn all(dir: &Path) -> Result<Vec<Bead>, String> {
    parse_list(&bd(
        dir,
        &["list", "--label", LABEL, "--all", "--json", "--limit", "0"],
    )?)
}

/// What the records say about one key.
#[derive(Debug)]
pub enum Status {
    Pass(Bead),
    Running(Bead),
    Missing { abandoned: usize },
}

pub fn status(beads: Vec<Bead>) -> Status {
    let abandoned = beads.iter().filter(|b| b.verdict() == "abandoned").count();
    if let Some(b) = beads.iter().find(|b| b.is_pass()) {
        return Status::Pass(b.clone());
    }
    match beads.into_iter().find(Bead::is_running) {
        Some(b) => Status::Running(b),
        None => Status::Missing { abandoned },
    }
}

/// Creates a gate bead; `closed` with a reason closes it at once (skip,
/// unreviewed, no-reviewer). Returns its id.
pub fn create(
    dir: &Path,
    title: &str,
    key: &str,
    meta: &Value,
    closed: Option<&str>,
) -> Result<String, String> {
    let r = format!("gate:{key}");
    let meta = meta.to_string();
    let status = if closed.is_some() {
        "open"
    } else {
        "in_progress"
    };
    let id = bd(
        dir,
        &[
            "create",
            "--title",
            title,
            "--type",
            "task",
            "--priority",
            "4",
            "--labels",
            LABEL,
            "--external-ref",
            &r,
            "--metadata",
            &meta,
            "--status",
            status,
            "--silent",
        ],
    )?
    .trim()
    .to_string();
    if id.is_empty() {
        return Err("bd create: no id".into());
    }
    if let Some(reason) = closed {
        close(dir, &id, reason)?;
    }
    Ok(id)
}

pub fn set_verdict(dir: &Path, id: &str, verdict: &str, notes: Option<&str>) -> Result<(), String> {
    let v = format!("verdict={verdict}");
    let mut args = vec!["update", id, "--set-metadata", &v];
    if let Some(n) = notes.filter(|n| !n.is_empty()) {
        args.extend(["--append-notes", n]);
    }
    bd(dir, &args).map(|_| ())
}

pub fn close(dir: &Path, id: &str, reason: &str) -> Result<(), String> {
    bd(dir, &["close", id, "--reason", reason]).map(|_| ())
}

/// Sets the verdict and closes the bead.
pub fn finish(
    dir: &Path,
    id: &str,
    verdict: &str,
    reason: &str,
    notes: Option<&str>,
) -> Result<(), String> {
    set_verdict(dir, id, verdict, notes)?;
    close(dir, id, reason)
}

/// Common metadata of a gate bead.
pub fn meta(fields: &[(&str, String)]) -> Value {
    let mut m = json!({});
    for (k, v) in fields {
        m[*k] = Value::String(v.clone());
    }
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_prefers_a_pass_then_a_running_review() {
        let list = r#"{"schema_version":1,"data":[
          {"id":"g-1","status":"closed","metadata":{"verdict":"fail"}},
          {"id":"g-2","status":"in_progress","metadata":{"verdict":"running"}},
          {"id":"g-3","status":"closed","metadata":{"verdict":"abandoned"}}]}"#;
        let beads = parse_list(list).unwrap();
        assert!(matches!(status(beads.clone()), Status::Running(b) if b.id == "g-2"));
        let mut with_pass = beads;
        with_pass[0].meta = json!({"verdict":"pass"});
        assert!(matches!(status(with_pass), Status::Pass(b) if b.id == "g-1"));
        let beads =
            parse_list(r#"[{"id":"g-3","status":"closed","metadata":{"verdict":"abandoned"}}]"#)
                .unwrap();
        assert!(matches!(status(beads), Status::Missing { abandoned: 1 }));
        // A pass verdict on a bead not yet closed is no pass.
        let open =
            parse_list(r#"[{"id":"g-4","status":"open","metadata":{"verdict":"pass"}}]"#).unwrap();
        assert!(matches!(status(open), Status::Missing { abandoned: 0 }));
    }
}
