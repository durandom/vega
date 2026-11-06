//! Bounded, value-free config write history. Never serialize live/source content.
use std::collections::BTreeSet;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use crate::{Ctx, Env, FilePlan, Plan, Unit};

const LIMIT: usize = 256;

pub fn path(env: &Env) -> PathBuf {
    env.state.join("vega/config-evidence.json")
}

pub fn event(unit: Unit, result: &str, changes: Vec<Value>) -> Value {
    let phase = match std::env::var("VEGA_CONFIG_PHASE").as_deref() {
        Ok("install") => "install",
        Ok("deploy") => "deploy",
        _ => "apply",
    };
    json!({"at_unix_ms": SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as u64,
        "phase": phase, "unit": unit.name(), "result": result, "writes": changes})
}

/// JSON pointers identify changed managed keys; arrays are opaque (hook commands
/// and arbitrary settings never become evidence). Whole files have no key diff.
pub fn change(ctx: &Ctx, unit: Unit, plan: &Plan, f: &FilePlan) -> Value {
    let mut keys = Vec::new();
    if f.follow {
        let old: Value = f
            .live
            .as_deref()
            .and_then(|s| serde_json::from_str(s).ok())
            .unwrap_or(json!({}));
        let new: Value = serde_json::from_str(&f.text).unwrap_or(json!({}));
        if matches!(unit, Unit::ClaudeSettings | Unit::HerdrAppSettings) {
            let names: BTreeSet<_> = old
                .as_object()
                .into_iter()
                .flat_map(|m| m.keys())
                .chain(new.as_object().into_iter().flat_map(|m| m.keys()))
                .collect();
            for name in names {
                leaf(
                    &format!("/{}", name.replace('~', "~0").replace('/', "~1")),
                    old.get(name),
                    new.get(name),
                    &mut keys,
                );
            }
        } else {
            diff("", Some(&old), Some(&new), &mut keys);
        }
        if matches!(unit, Unit::ClaudeSettings | Unit::HerdrAppSettings) {
            let last = crate::last_seeds(&ctx.env, unit);
            for key in &mut keys {
                // Seeded keys are compared as top-level keys; values stay private.
                let name = key["key"]
                    .as_str()
                    .unwrap_or("")
                    .trim_start_matches('/')
                    .replace("~1", "/")
                    .replace("~0", "~");
                key["source_changed"] = json!(plan
                    .seeds
                    .as_ref()
                    .is_some_and(|s| last.get(&name) != s.get(&name)));
                key["reason"] = json!(if old.get(&name).is_none() {
                    "missing_key"
                } else {
                    "source_changed"
                });
            }
        }
    }
    let target = f
        .path
        .strip_prefix(&ctx.env.home)
        .map(|p| format!("~/{}", p.display()))
        .unwrap_or_else(|_| f.path.display().to_string());
    let backup = f.live.as_ref().map(|_| {
        let path = if f.follow {
            fs::canonicalize(&f.path).unwrap_or_else(|_| f.path.clone())
        } else {
            f.path.clone()
        };
        let path = crate::backup_path(&path);
        path.strip_prefix(&ctx.env.home)
            .map(|p| format!("~/{}", p.display()))
            .unwrap_or_else(|_| path.display().to_string())
    });
    json!({"target": target, "operation": if f.live.is_none() {"added"} else {"changed"},
        "before": if f.live.is_none() {"absent"} else {"redacted"}, "after": "redacted", "backup_refreshed": backup, "keys": keys})
}

fn diff(prefix: &str, old: Option<&Value>, new: Option<&Value>, out: &mut Vec<Value>) {
    if old == new {
        return;
    }
    // Descend objects, including newly created objects, but never array entries.
    let a = old.and_then(Value::as_object);
    let b = new.and_then(Value::as_object);
    if (a.is_some() || old.is_none())
        && (b.is_some() || new.is_none())
        && (a.is_some() || b.is_some())
    {
        let names: BTreeSet<_> = a
            .into_iter()
            .flat_map(|m| m.keys())
            .chain(b.into_iter().flat_map(|m| m.keys()))
            .collect();
        for k in names {
            let key = k.replace('~', "~0").replace('/', "~1");
            diff(
                &format!("{prefix}/{key}"),
                a.and_then(|m| m.get(k)),
                b.and_then(|m| m.get(k)),
                out,
            );
        }
    } else {
        leaf(prefix, old, new, out);
    }
}

fn leaf(prefix: &str, old: Option<&Value>, new: Option<&Value>, out: &mut Vec<Value>) {
    if old != new {
        out.push(json!({"key": prefix, "operation": if old.is_none() {"added"} else if new.is_none() {"removed"} else {"changed"},
            "before": if old.is_none() {"absent"} else {"redacted"}, "after": if new.is_none() {"absent"} else {"redacted"}}));
    }
}

/// Serialize concurrent applies, atomically replace history, retain the last 256
/// unit results. A damaged history fails loudly instead of discarding evidence.
pub fn retain(env: &Env, event: &Value) -> Result<(), String> {
    let p = path(env);
    let dir = p.parent().unwrap();
    fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .mode(0o600)
        .open(dir.join("config-evidence.lock"))
        .map_err(|e| e.to_string())?;
    lock.lock().map_err(|e| e.to_string())?;
    let mut events: Vec<Value> = match fs::read(&p) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|e| format!("{}: {e}", p.display()))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(e) => return Err(e.to_string()),
    };
    events.push(event.clone());
    if events.len() > LIMIT {
        events.drain(..events.len() - LIMIT);
    }
    let tmp = dir.join(format!(".config-evidence.{}", std::process::id()));
    let result = (|| {
        let mut f = OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(&tmp)?;
        f.write_all(
            serde_json::to_string(&events)
                .expect("JSON values serialize")
                .as_bytes(),
        )?;
        f.sync_all()?;
        fs::rename(&tmp, &p)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result.map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removed_keys_and_opaque_arrays_have_no_values() {
        let mut out = Vec::new();
        diff(
            "",
            Some(&json!({"hooks":{"PreToolUse":["SECRET"]},"removed":"SECRET"})),
            Some(&json!({"hooks":{"PreToolUse":["NEXT_SECRET"]}})),
            &mut out,
        );
        assert_eq!(out[0]["key"], "/hooks/PreToolUse");
        assert_eq!(out[0]["operation"], "changed");
        assert_eq!(out[1]["key"], "/removed");
        assert_eq!(out[1]["operation"], "removed");
        assert_eq!(out[1]["after"], "absent");
        assert!(!serde_json::to_string(&out).unwrap().contains("SECRET"));
    }

    #[test]
    fn concurrent_history_updates_keep_all_events() {
        let d = tempfile::tempdir().unwrap();
        let env = Env::at(d.path());
        std::thread::scope(|s| {
            for _ in 0..8 {
                let env = &env;
                s.spawn(move || {
                    for _ in 0..8 {
                        retain(env, &event(Unit::ClaudeMd, "unchanged", vec![])).unwrap();
                    }
                });
            }
        });
        let events: Vec<Value> = serde_json::from_slice(&fs::read(path(&env)).unwrap()).unwrap();
        assert_eq!(events.len(), 64);
    }
}
