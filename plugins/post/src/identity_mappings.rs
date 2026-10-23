//! Read-only discovery for the flag-day preview, never a steady-state reader.
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde_json::Value;
use vega_herdr::cutover::{Mapping, Snapshot};
use vega_herdr::Failure;

fn collect(snapshot: &Snapshot) -> Result<BTreeSet<String>, String> {
    let mut keys = BTreeSet::new();
    for (path, bytes) in snapshot {
        let parts = path.components().count();
        // Lanes have no project in old payloads. Their lossy filenames (and
        // orphan status/sync files) are checked by the pure planner below.
        let project_record = parts == 2
            && ["uac", "uac-status", "badges"]
                .iter()
                .any(|kind| path.starts_with(kind));
        let note = parts == 2
            && path.starts_with("notes")
            && path.extension().is_some_and(|ext| ext == "json")
            && path != Path::new("notes/seen.json");
        let bd_project = parts == 3
            && path.starts_with("bd/projects")
            && path.extension().is_some_and(|ext| ext == "json");
        if !project_record
            && !note
            && !bd_project
            && path != Path::new("hygiene.json")
            && path != Path::new("pagerduty/state.json")
        {
            continue;
        }
        let v: Value = serde_json::from_slice(bytes)
            .map_err(|_| format!("invalid project snapshot at {}", path.display()))?;
        if v["schema"] == 2 {
            continue;
        }
        if path == Path::new("hygiene.json") {
            keys.extend(
                v["daily"]
                    .as_object()
                    .ok_or("invalid hygiene daily records")?
                    .keys()
                    .cloned(),
            );
        } else if path == Path::new("pagerduty/state.json") {
            if v["version"] == 1 {
                for record in v["incidents"]
                    .as_object()
                    .ok_or("invalid PagerDuty state")?
                    .values()
                {
                    for variant in ["Mail", "Board"] {
                        if let Some(key) = record["delivered"]["to"][variant]["project"].as_str() {
                            keys.insert(key.to_owned());
                        }
                    }
                }
            }
        } else if let Some(key) = v[if path.starts_with("bd/projects") {
            "key"
        } else {
            "project"
        }]
        .as_str()
        {
            keys.insert(key.to_owned());
        }
    }
    Ok(keys)
}

pub(super) fn generate(
    snapshot: &Snapshot,
    registry: &BTreeMap<String, Value>,
    host: &str,
    home: &Path,
) -> Result<Vec<Mapping>, Failure> {
    let mut keys = collect(snapshot).map_err(super::identity::failure)?;
    for (key, value) in registry {
        if key.starts_with(&format!("{}.", vega_registry::token(host)))
            && vega_registry::is_legacy_project_key(key)
        {
            keys.insert(
                value["project"]
                    .as_str()
                    .ok_or_else(|| {
                        super::identity::failure(format!(
                            "legacy registry payload missing project: {key}"
                        ))
                    })?
                    .to_owned(),
            );
        }
    }
    let mut mappings = Vec::new();
    let mut failures = Vec::new();
    for old_key in keys {
        let path = if old_key == "~" {
            home.to_path_buf()
        } else if let Some(rest) = old_key.strip_prefix("~/") {
            home.join(rest)
        } else {
            PathBuf::from(&old_key)
        };
        let resolved = if path.is_absolute() {
            vega_herdr::project::resolve(&path, host)
        } else {
            Err(Failure::new(
                "project_path",
                "legacy key is not an absolute or home-relative path",
                "restore the original local project path before generating mappings",
            ))
        };
        match resolved {
            Ok(p) => mappings.push(Mapping {
                old_key,
                project: p.id,
                location: p.location,
            }),
            Err(e) => failures.push(Failure::new(
                &e.code,
                format!("{old_key}: {}", e.message),
                &e.hint,
            )),
        }
    }
    if !failures.is_empty() {
        // Report every key, preserving each resolver's actionable hint, and let
        // the normal CLI boundary render the last error and return exit 1.
        let last = failures.pop().unwrap();
        for error in failures {
            vega_herdr::print_failure(&error);
        }
        return Err(last);
    }
    super::identity::local_plans(snapshot, &mappings, host).map_err(|e| {
        if e.starts_with("unmapped ") {
            Failure::new(
                "identity_unmapped_state", e,
                "restore missing legacy project metadata from backup; lossy filenames alone cannot recover paths; rerun `vega post identity mappings --output mappings.json`",
            )
        } else {
            super::identity::failure(e)
        }
    })?;
    Ok(mappings)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn collects_distinct_references_and_ignores_v2_and_watermarks() {
        let snapshot = [
            ("uac/old", json!({"project":"~/claim/.git"})),
            ("uac-status/old", json!({"project":"~/status/.git"})),
            ("badges/old.json", json!({"project":"~/badge/.git"})),
            ("notes/1.json", json!({"project":"~/note/.git"})),
            ("notes/seen.json", json!({"project":"not-a-project"})),
            (
                "notes/2.json",
                json!({"schema":2,"project":"github.com/team/repo"}),
            ),
            ("hygiene.json", json!({"daily":{"~/hygiene/.git":{}}})),
            (
                "pagerduty/state.json",
                json!({"version":1,"incidents":{
                "A":{"delivered":{"to":{"Mail":{"project":"~/mail/.git"}}}},
                "B":{"delivered":{"to":{"Board":{"project":"~/board/.git"}}}},
                "C":{"delivered":null}}}),
            ),
            ("bd/projects/old.json", json!({"key":"~/bd/.git"})),
            (
                "bd/projects/v2/a/b.json",
                json!({"schema":2,"project":"github.com/team/repo"}),
            ),
        ]
        .into_iter()
        .map(|(path, value)| (path.into(), value.to_string().into_bytes()))
        .collect();
        let expected = [
            "claim", "status", "badge", "note", "hygiene", "mail", "board", "bd",
        ]
        .into_iter()
        .map(|name| format!("~/{name}/.git"))
        .collect();
        assert_eq!(collect(&snapshot).unwrap(), expected);
    }

    #[test]
    fn registry_only_keys_resolve_locally_and_foreign_paths_are_ignored() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().canonicalize().unwrap();
        assert!(std::process::Command::new("git")
            .args(["init", "-q"])
            .arg(home.join("repo"))
            .status()
            .unwrap()
            .success());
        let entries = BTreeMap::from([
            ("a.lanista.old".into(), json!({"project":"~/repo/.git"})),
            (
                "foreign.lanista.old".into(),
                json!({"project":"/missing/foreign"}),
            ),
            (
                "a.lanista.v2.abc".into(),
                json!({"project":"github.com/team/repo"}),
            ),
        ]);
        let mappings = generate(&Snapshot::new(), &entries, "a", &home).unwrap();
        assert_eq!(mappings.len(), 1);
        assert_eq!(mappings[0].old_key, "~/repo/.git");
        assert_eq!(mappings[0].location.common_dir, home.join("repo/.git"));
    }

    #[test]
    fn invalid_records_keep_the_cutover_hint() {
        let snapshot = Snapshot::from([("notes/1.json".into(), b"{}".to_vec())]);
        let error = generate(&snapshot, &BTreeMap::new(), "a", Path::new("/tmp")).unwrap_err();
        assert_eq!(error.message, "invalid note snapshot");
        assert!(!error.hint.contains("lossy"));
        assert!(error.hint.contains("project-registry-cutover.md"));
    }

    #[test]
    fn orphan_lossy_filenames_fail_with_recovery_hint() {
        let snapshot = Snapshot::from([("lanes/local_~_lost_.git".into(), b"{}\n".to_vec())]);
        let error = generate(&snapshot, &BTreeMap::new(), "a", Path::new("/tmp")).unwrap_err();
        assert_eq!(error.code, "identity_unmapped_state");
        assert!(error.message.contains("lanes/local_~_lost_.git"));
        assert!(error.hint.contains("backup"));
    }
}
