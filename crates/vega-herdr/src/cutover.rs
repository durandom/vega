//! Pure host-local file projection for the single journaled startup cutover.
//! The coordinator owns snapshots, durable plans, application and rollback;
//! these helpers never access Git, another host's paths, or live state.
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::project::{encode_component, ProjectId, RepoLocation};

pub type Snapshot = BTreeMap<PathBuf, Vec<u8>>;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Mapping {
    pub old_key: String,
    pub project: ProjectId,
    pub location: RepoLocation,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Plan {
    /// Relative to this host's vega state root, including in-place projections.
    pub writes: Snapshot,
    /// Only old project files; unchanged pane state and watermarks are excluded.
    pub remove: BTreeSet<PathBuf>,
}

fn value(bytes: &[u8]) -> Result<Value, String> {
    serde_json::from_slice(bytes).map_err(|_| "invalid local project snapshot".into())
}

fn legacy(kind: &str, key: &str) -> PathBuf {
    if kind == "badges" {
        let name: String = key
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || "-.".contains(c) {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        Path::new(kind).join(format!("{name}.json"))
    } else {
        Path::new(kind).join(format!("local_{key}").replace([':', '/'], "_"))
    }
}

fn destination(kind: &str, id: &ProjectId) -> PathBuf {
    Path::new(kind)
        .join("v2")
        .join(format!("{}.json", encode_component(id.as_str())))
}

fn put(plan: &mut Plan, snapshot: &Snapshot, path: PathBuf, v: &Value) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(v).map_err(|_| "cannot encode local projection")?;
    if let Some(prior) = plan.writes.get(&path).or_else(|| snapshot.get(&path)) {
        if value(prior)? != *v {
            return Err(format!(
                "conflicting local project records at {}",
                path.display()
            ));
        }
    }
    plan.writes.insert(path, bytes);
    Ok(())
}

fn merge_claim(prior: &Value, next: &mut Value) -> Result<(), String> {
    if prior["pane"] != next["pane"] || prior["host"] != next["host"] {
        return Err("conflicting local UAC ownership for one project ID".into());
    }
    // Every other durable claim field (including tracker) must agree. Location
    // and alias metadata combine; claim time retains the earliest start.
    let excluded = [
        "common_dir",
        "checkout",
        "locations",
        "aliases",
        "folder_name",
        "origin_name",
        "at",
    ];
    for (key, val) in prior.as_object().ok_or("invalid UAC claim")? {
        if !excluded.contains(&key.as_str()) && !next[key].is_null() && &next[key] != val {
            return Err(format!("incompatible UAC claim field {key}"));
        }
        if next[key].is_null() {
            next[key] = val.clone();
        }
    }
    let mut aliases = BTreeSet::new();
    let mut locations = BTreeSet::<RepoLocation>::new();
    for v in [prior, &*next] {
        for field in ["folder_name", "origin_name"] {
            if let Some(alias) = v[field].as_str() {
                aliases.insert(alias.to_owned());
            }
        }
        if let Some(list) = v["aliases"].as_array() {
            aliases.extend(list.iter().filter_map(|a| a.as_str().map(str::to_owned)));
        }
        if let Some(list) = v["locations"].as_array() {
            for location in list {
                locations.insert(
                    serde_json::from_value(location.clone()).map_err(|_| "invalid UAC location")?,
                );
            }
        }
    }
    next["aliases"] = json!(aliases);
    next["locations"] = json!(locations);
    let a = prior["at"].as_i64().ok_or("claim missing timestamp")?;
    let b = next["at"].as_i64().ok_or("claim missing timestamp")?;
    next["at"] = a.min(b).into();
    // Preserve the primary execution location from the first claim; never
    // choose a different clone based on its timestamp or filesystem spelling.
    next["common_dir"] = prior["common_dir"].clone();
    next["checkout"] = prior["checkout"].clone();
    Ok(())
}

/// `mappings` must come from this owner's local resolver while quiesced. Files
/// are relative to vega's state directory, unlike bd's separate sub-plan root.
/// Missing mappings and incompatible collisions block before any writes.
pub fn prepare(
    snapshot: &Snapshot,
    mappings: &[Mapping],
    local_host: &str,
) -> Result<Plan, String> {
    prepare_with_aliases(snapshot, mappings, local_host, &[])
}

/// Explicit owning-host aliases allow a configured registry host spelling to
/// replace its old local hostname. The coordinator must validate these aliases
/// on this host; the helper never infers host ownership from a remote payload.
pub fn prepare_with_aliases(
    snapshot: &Snapshot,
    mappings: &[Mapping],
    local_host: &str,
    old_hosts: &[&str],
) -> Result<Plan, String> {
    let mut plan = Plan {
        writes: Snapshot::new(),
        remove: BTreeSet::new(),
    };
    let mut ids = BTreeMap::new();
    let mut locations = BTreeMap::new();
    let mut old_locations = BTreeMap::new();
    let mut sources = BTreeMap::new();
    let mut claims = BTreeMap::<PathBuf, Value>::new();
    let mut lanes = BTreeMap::<PathBuf, Vec<Value>>::new();
    for m in mappings {
        if old_locations
            .insert(&m.old_key, &m.location.common_dir)
            .is_some_and(|common| common != &m.location.common_dir)
        {
            return Err("one legacy key maps to different locations".into());
        }
        if !m.location.common_dir.is_absolute() || !m.location.checkout.is_absolute() {
            return Err("migration requires canonical absolute local locations".into());
        }
        if ids
            .insert(&m.old_key, &m.project)
            .is_some_and(|id| id != &m.project)
            || locations
                .insert(&m.location.common_dir, &m.project)
                .is_some_and(|id| id != &m.project)
        {
            return Err("conflicting project identity mapping".into());
        }
        for kind in ["uac", "uac-status", "lanes", "badges"] {
            let source = legacy(kind, &m.old_key);
            if sources
                .insert(source.clone(), &m.old_key)
                .is_some_and(|old| old != &m.old_key)
            {
                return Err("lossy local project filenames collide".into());
            }
            let Some(bytes) = snapshot.get(&source) else {
                continue;
            };
            let dest = destination(kind, &m.project);
            if kind == "lanes" {
                let text = std::str::from_utf8(bytes).map_err(|_| "invalid lane encoding")?;
                for line in text.lines().filter(|line| !line.trim().is_empty()) {
                    let mut v = value(line.as_bytes())?;
                    if !v["actor"].is_string() || !v["since"].is_i64() {
                        return Err("invalid lane record".into());
                    }
                    v["schema"] = 2.into();
                    v["project"] = json!(m.project);
                    let records = lanes.entry(dest.clone()).or_default();
                    if !records.contains(&v) {
                        records.push(v);
                    }
                }
            } else {
                let mut v = value(bytes)?;
                // Legacy uac-status writers persisted just the epoch seconds.
                if kind == "uac-status" && v.is_i64() {
                    v = json!({"at": v});
                }
                if !v.is_object() {
                    return Err("project record is not an object".into());
                }
                if let Some(old) = v["project"].as_str() {
                    if old != m.old_key {
                        return Err("legacy project payload/filename mismatch".into());
                    }
                }
                v["schema"] = 2.into();
                v["project"] = json!(m.project);
                if kind == "uac" {
                    let owner = v["host"].as_str().ok_or("UAC missing host")?;
                    let owner = crate::state::host_token(&owner.to_lowercase());
                    if owner != crate::state::host_token(&local_host.to_lowercase())
                        && !old_hosts
                            .iter()
                            .any(|host| crate::state::host_token(&host.to_lowercase()) == owner)
                    {
                        return Err("foreign UAC claim in local snapshot".into());
                    }
                    if !v["pane"].is_string() || !v["at"].is_i64() {
                        return Err("invalid UAC claim".into());
                    }
                    v["host"] = local_host.into();
                    v["common_dir"] = json!(m.location.common_dir);
                    v["checkout"] = json!(m.location.checkout);
                    v["locations"] = json!([m.location]);
                    if !v["folder_name"].is_string() {
                        let common = &m.location.common_dir;
                        let folder = if common
                            .file_name()
                            .is_some_and(|n| n == ".git" || n == ".bare")
                        {
                            common.parent()
                        } else {
                            Some(common.as_path())
                        };
                        v["folder_name"] = folder
                            .and_then(Path::file_name)
                            .and_then(|n| n.to_str())
                            .unwrap_or_default()
                            .into();
                    }
                    if let Some(prior) = claims.get(&dest) {
                        merge_claim(prior, &mut v)?;
                    }
                    claims.insert(dest, v);
                } else {
                    if !v["at"].is_i64() {
                        return Err("project snapshot missing timestamp".into());
                    }
                    put(&mut plan, snapshot, dest, &v)?;
                }
            }
            plan.remove.insert(source);
        }
    }
    for (path, claim) in claims {
        put(&mut plan, snapshot, path, &claim)?;
    }
    for (path, records) in lanes {
        // Keep every actor/pane record and its original since; the reader
        // computes the earliest start per actor across merged clone lanes.
        let mut text = String::new();
        for v in records {
            text.push_str(&v.to_string());
            text.push('\n');
        }
        let bytes = text.into_bytes();
        if snapshot.get(&path).is_some_and(|prior| prior != &bytes) {
            return Err("conflicting v2 lanes".into());
        }
        plan.writes.insert(path, bytes);
    }
    for (path, bytes) in snapshot {
        let parts: Vec<_> = path.components().collect();
        if parts.len() == 2
            && ["uac", "uac-status", "lanes", "badges"]
                .iter()
                .any(|kind| path.starts_with(kind))
            && !plan.remove.contains(path)
        {
            return Err(format!("unmapped local project file {}", path.display()));
        }
        if path.starts_with("notes")
            && parts.len() == 2
            && path.extension().is_some_and(|ext| ext == "json")
            && path.file_name().is_none_or(|name| name != "seen.json")
        {
            let mut v = value(bytes)?;
            if v["schema"] != 2 {
                if let Some(old) = v["project"].as_str() {
                    let id = ids.get(&old.to_owned()).ok_or("unmapped note project")?;
                    v["project"] = json!(id);
                }
                v["schema"] = 2.into();
            }
            if crate::notes::Note::from_json(&v).is_none() {
                return Err("invalid note snapshot".into());
            }
            plan.writes.insert(
                path.clone(),
                serde_json::to_vec_pretty(&v).map_err(|_| "cannot encode note")?,
            );
        }
        if path == Path::new("hygiene.json") {
            let mut v = value(bytes)?;
            if v["schema"] != 2 {
                let daily = v["daily"]
                    .as_object()
                    .ok_or("invalid hygiene daily records")?;
                let mut next = serde_json::Map::new();
                for (old, record) in daily {
                    let id = ids.get(old).ok_or("unmapped hygiene project")?.as_str();
                    if next
                        .insert(id.into(), record.clone())
                        .is_some_and(|prior| prior != *record)
                    {
                        return Err("conflicting hygiene records".into());
                    }
                }
                v["daily"] = next.into();
                v["schema"] = 2.into();
            }
            plan.writes.insert(
                path.clone(),
                serde_json::to_vec_pretty(&v).map_err(|_| "cannot encode hygiene")?,
            );
        }
        if path == Path::new("pagerduty/state.json") {
            let mut v = value(bytes)?;
            if v["version"] == 1 {
                for tracked in v["incidents"]
                    .as_object_mut()
                    .ok_or("invalid PagerDuty state")?
                    .values_mut()
                {
                    if tracked["delivered"].is_null() {
                        continue;
                    }
                    let to = &mut tracked["delivered"]["to"];
                    for variant in ["Mail", "Board"] {
                        if let Some(old) = to[variant]["project"].as_str() {
                            let id = ids
                                .get(&old.to_owned())
                                .ok_or("unmapped PagerDuty receipt project")?;
                            to[variant]["project"] = json!(id);
                        }
                    }
                }
                v["version"] = 2.into();
            } else if v["version"] != 2 {
                return Err("unsupported PagerDuty state version".into());
            }
            plan.writes.insert(
                path.clone(),
                serde_json::to_vec_pretty(&v).map_err(|_| "cannot encode PagerDuty state")?,
            );
        }
    }
    Ok(plan)
}

/// Explicit v2-to-v2 rekey of one ID's owner-local records after a remote
/// change (vega-lhc.14.7). `snapshot` holds this host's v2 files; the caller
/// verifies that every old location now resolves to `to`, journals the plan
/// and applies it while writers are stopped. Joins never drop durable data:
/// claims merge only for the same pane, lanes take the union, and the derived
/// caches (uac-status, badges, hygiene day marks) keep the new ID's record.
pub fn rekey(snapshot: &Snapshot, from: &str, to: &ProjectId) -> Result<Plan, String> {
    if from == to.as_str() {
        return Err("old and new project IDs are equal".into());
    }
    let mut plan = Plan {
        writes: Snapshot::new(),
        remove: BTreeSet::new(),
    };
    let conflict = |path: &Path| format!("conflicting join at {}", path.display());
    for kind in ["uac", "uac-status", "badges", "lanes"] {
        let source = Path::new(kind)
            .join("v2")
            .join(format!("{}.json", encode_component(from)));
        let Some(bytes) = snapshot.get(&source) else {
            continue;
        };
        let dest = destination(kind, to);
        let prior = snapshot.get(&dest);
        plan.remove.insert(source);
        if kind == "lanes" {
            let mut text = prior
                .map(|b| String::from_utf8(b.clone()))
                .transpose()
                .map_err(|_| "invalid lane encoding")?
                .unwrap_or_default();
            let known: Vec<Value> = text
                .lines()
                .filter(|l| !l.trim().is_empty())
                .map(|l| value(l.as_bytes()))
                .collect::<Result<_, _>>()?;
            if !text.is_empty() && !text.ends_with('\n') {
                text.push('\n');
            }
            let lines = std::str::from_utf8(bytes).map_err(|_| "invalid lane encoding")?;
            for line in lines.lines().filter(|l| !l.trim().is_empty()) {
                let mut v = value(line.as_bytes())?;
                if !v["actor"].is_string() || !v["since"].is_i64() {
                    return Err("invalid lane record".into());
                }
                v["project"] = json!(to);
                if !known.contains(&v) {
                    text.push_str(&v.to_string());
                    text.push('\n');
                }
            }
            plan.writes.insert(dest, text.into_bytes());
            continue;
        }
        let mut v = value(bytes)?;
        if v["project"] != from {
            return Err(format!(
                "project payload/filename mismatch at {}",
                dest.display()
            ));
        }
        v["project"] = json!(to);
        match (kind, prior) {
            // Derived caches: vegad recomputes the new ID's record.
            ("uac-status" | "badges", Some(_)) => continue,
            ("uac", Some(prior)) => merge_claim(&value(prior)?, &mut v)
                .map_err(|e| format!("{}: {e}", conflict(&dest)))?,
            _ => {}
        }
        plan.writes.insert(
            dest,
            serde_json::to_vec_pretty(&v).map_err(|_| "cannot encode local projection")?,
        );
    }
    for (path, bytes) in snapshot {
        let parts = path.components().count();
        if path.starts_with("notes")
            && parts == 2
            && path.extension().is_some_and(|ext| ext == "json")
            && path.file_name().is_none_or(|name| name != "seen.json")
        {
            let mut v = value(bytes)?;
            if v["project"] == from {
                v["project"] = json!(to);
                if crate::notes::Note::from_json(&v).is_none() {
                    return Err(format!("invalid note {}", path.display()));
                }
                plan.writes.insert(
                    path.clone(),
                    serde_json::to_vec_pretty(&v).map_err(|_| "cannot encode note")?,
                );
            }
        }
        if path == Path::new("hygiene.json") {
            let mut v = value(bytes)?;
            let daily = v["daily"]
                .as_object_mut()
                .ok_or("invalid hygiene daily records")?;
            if let Some(record) = daily.remove(from) {
                daily.entry(to.as_str()).or_insert(record);
                plan.writes.insert(
                    path.clone(),
                    serde_json::to_vec_pretty(&v).map_err(|_| "cannot encode hygiene")?,
                );
            }
        }
        if path == Path::new("pagerduty/state.json") {
            let mut v = value(bytes)?;
            if v["version"] != 2 {
                return Err("unsupported PagerDuty state version".into());
            }
            let mut changed = false;
            for tracked in v["incidents"]
                .as_object_mut()
                .ok_or("invalid PagerDuty state")?
                .values_mut()
            {
                for variant in ["Mail", "Board"] {
                    // pointer_mut: indexing would insert into undelivered records.
                    if let Some(project) =
                        tracked.pointer_mut(&format!("/delivered/to/{variant}/project"))
                    {
                        if *project == from {
                            *project = json!(to);
                            changed = true;
                        }
                    }
                }
            }
            if changed {
                plan.writes.insert(
                    path.clone(),
                    serde_json::to_vec_pretty(&v).map_err(|_| "cannot encode PagerDuty state")?,
                );
            }
        }
        // Schedule job state and run files move byte for byte.
        let old_dir = Path::new("schedule").join(encode_component(from));
        if let Ok(rest) = path.strip_prefix(&old_dir) {
            let dest = Path::new("schedule").join(to.encoded()).join(rest);
            if snapshot.get(&dest).is_some_and(|prior| prior != bytes) {
                return Err(conflict(&dest));
            }
            plan.writes.insert(dest, bytes.clone());
            plan.remove.insert(path.clone());
        }
    }
    Ok(plan)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mapping(old: &str, common: &str, checkout: &str) -> Mapping {
        Mapping {
            old_key: old.into(),
            project: serde_json::from_value(json!("github.com/team/repo")).unwrap(),
            location: RepoLocation {
                common_dir: common.into(),
                checkout: checkout.into(),
            },
        }
    }
    fn add(snapshot: &mut Snapshot, path: PathBuf, v: Value) {
        snapshot.insert(path, serde_json::to_vec(&v).unwrap());
    }
    fn claim(snapshot: &mut Snapshot, m: &Mapping, pane: &str, at: i64) {
        add(
            snapshot,
            legacy("uac", &m.old_key),
            json!({"project":m.old_key,"pane":pane,"host":"host","at":at,"tracker":"task-1","origin_name":"repo"}),
        );
    }

    #[test]
    fn clone_claims_lanes_notes_and_receipts_preserve_durable_fields() {
        let a = mapping("~/a/.git", "/a/.git", "/a");
        let b = mapping("~/b/.bare", "/b/.bare", "/b/main");
        let mut snapshot = Snapshot::new();
        claim(&mut snapshot, &a, "p1", 20);
        claim(&mut snapshot, &b, "p1", 10);
        snapshot.insert(
            legacy("lanes", &a.old_key),
            b"{\"actor\":\"host/fix\",\"since\":20,\"pane\":\"p1\"}\n".to_vec(),
        );
        snapshot.insert(
            legacy("lanes", &b.old_key),
            b"{\"actor\":\"host/fix\",\"since\":10,\"pane\":\"p1\"}\n".to_vec(),
        );
        add(
            &mut snapshot,
            legacy("uac-status", &a.old_key),
            json!({"at":50}),
        );
        add(
            &mut snapshot,
            legacy("badges", &a.old_key),
            json!({"project":a.old_key,"notes":1,"decisions":2,"waiting":1,"waiting_panes":[{"pane":"p2","attention":"asking","title":"question"}],"at":50}),
        );
        add(
            &mut snapshot,
            "notes/5000.json".into(),
            json!({"id":5000,"host":"host","project":b.old_key,"cause":"bd-sync:DB","resolved":null,"title":"stopped","body":"conflict"}),
        );
        add(
            &mut snapshot,
            "notes/seen.json".into(),
            json!({"read":{"host":4000},"cleared":{"host":3000}}),
        );
        add(
            &mut snapshot,
            "hygiene.json".into(),
            json!({"waiting":{"p2":{"since":2,"mailed":true}},"daily":{a.old_key.clone():{"at":4,"digest":"unchanged"}}}),
        );
        add(
            &mut snapshot,
            "pagerduty/state.json".into(),
            json!({"version":1,"attempted":42,"poll_error":null,"incidents":{"I":{"incident":{"id":"I"},"last_seen":45,"delivered":{"at":40,"status":"triggered","changed_at":"t","to":{"Mail":{"project":a.old_key,"host":"remote","pane":"p9"}}}}}}),
        );
        let original = snapshot.clone();
        let mappings = [a.clone(), b.clone()];
        let plan = prepare(&snapshot, &mappings, "host").unwrap();
        assert_eq!(snapshot, original);
        let uac = value(&plan.writes[&destination("uac", &a.project)]).unwrap();
        assert_eq!(uac["at"], 10);
        assert_eq!(uac["tracker"], "task-1");
        assert_eq!(uac["common_dir"], "/a/.git");
        assert_eq!(uac["locations"].as_array().unwrap().len(), 2);
        assert!(uac["aliases"].as_array().unwrap().contains(&json!("b")));
        let lines =
            String::from_utf8(plan.writes[&destination("lanes", &a.project)].clone()).unwrap();
        let lanes = crate::lanes::parse(&lines, a.project.as_str());
        assert_eq!(lanes[0].since, 10);
        assert_eq!(lanes[0].actor, "host/fix");
        let note = value(&plan.writes[Path::new("notes/5000.json")]).unwrap();
        assert_eq!(note["id"], 5000);
        assert_eq!(note["cause"], "bd-sync:DB");
        assert_eq!(note["project"], json!(a.project));
        assert!(!plan.writes.contains_key(Path::new("notes/seen.json")));
        let hygiene = value(&plan.writes[Path::new("hygiene.json")]).unwrap();
        assert_eq!(
            hygiene["waiting"],
            value(&snapshot[Path::new("hygiene.json")]).unwrap()["waiting"]
        );
        assert_eq!(hygiene["daily"][a.project.as_str()]["digest"], "unchanged");
        let receipt = value(&plan.writes[Path::new("pagerduty/state.json")]).unwrap();
        assert_eq!(receipt["incidents"]["I"]["delivered"]["at"], 40);
        assert_eq!(
            receipt["incidents"]["I"]["delivered"]["to"]["Mail"]["project"],
            json!(a.project)
        );
        assert_eq!(
            value(&plan.writes[&destination("uac-status", &a.project)]).unwrap()["at"],
            50
        );
        assert_eq!(
            value(&plan.writes[&destination("badges", &a.project)]).unwrap()["waiting_panes"][0]
                ["title"],
            "question"
        );
        let journal = serde_json::to_vec(&plan).unwrap();
        let replay: Plan = serde_json::from_slice(&journal).unwrap();
        snapshot.extend(replay.writes);
        assert_eq!(
            prepare(&snapshot, &mappings, "host").unwrap().writes,
            plan.writes
        );
        // Exact rollback is owned by .14.6: the original bytes include watermarks
        // and all timestamps and are unchanged by this pure projection.
        assert_eq!(
            original[Path::new("notes/seen.json")],
            snapshot[Path::new("notes/seen.json")]
        );
    }

    #[test]
    fn bare_integer_status_preserves_timestamp_and_rejects_other_scalars() {
        let m = mapping("~/repo/.git", "/repo/.git", "/repo");
        // Shape captured on both owner hosts: uac-status/local_* has no object.
        let mut files =
            Snapshot::from([(legacy("uac-status", &m.old_key), b"1792573200".to_vec())]);
        let plan = prepare(&files, std::slice::from_ref(&m), "host").unwrap();
        let status = value(&plan.writes[&destination("uac-status", &m.project)]).unwrap();
        assert_eq!(
            status,
            json!({"schema":2,"project":m.project,"at":1792573200_i64})
        );
        for invalid in [json!(true), json!("1792573200"), json!(1.5), json!([])] {
            add(&mut files, legacy("uac-status", &m.old_key), invalid);
            assert!(prepare(&files, std::slice::from_ref(&m), "host").is_err());
        }
    }

    #[test]
    fn conflicting_claims_trackers_badges_and_missing_mappings_block() {
        let a = mapping("~/a/.git", "/a/.git", "/a");
        let b = mapping("~/b/.git", "/b/.git", "/b");
        let mut snapshot = Snapshot::new();
        claim(&mut snapshot, &a, "p1", 10);
        claim(&mut snapshot, &b, "p2", 20);
        assert!(prepare(&snapshot, &[a.clone(), b.clone()], "host")
            .unwrap_err()
            .contains("ownership"));
        claim(&mut snapshot, &b, "p1", 20);
        let mut v = value(&snapshot[&legacy("uac", &b.old_key)]).unwrap();
        v["tracker"] = "task-other".into();
        add(&mut snapshot, legacy("uac", &b.old_key), v);
        assert!(prepare(&snapshot, &[a.clone(), b.clone()], "host")
            .unwrap_err()
            .contains("tracker"));
        assert!(prepare(&snapshot, std::slice::from_ref(&a), "host")
            .unwrap_err()
            .contains("unmapped"));
        assert!(prepare(&snapshot, &[a.clone(), b.clone()], "other-host")
            .unwrap_err()
            .contains("foreign"));
        snapshot.clear();
        for (m, notes) in [(&a, 1), (&b, 2)] {
            add(
                &mut snapshot,
                legacy("badges", &m.old_key),
                json!({"project":m.old_key,"notes":notes,"at":10}),
            );
        }
        assert!(prepare(&snapshot, &[a, b], "host")
            .unwrap_err()
            .contains("conflicting"));
    }

    #[test]
    fn lossy_tokens_unmapped_notes_and_forks_do_not_coalesce() {
        let a = mapping("a/b", "/a/.git", "/a");
        let b = mapping("a_b", "/b/.git", "/b");
        assert!(prepare(&Snapshot::new(), &[a.clone(), b], "host")
            .unwrap_err()
            .contains("collide"));
        let mut files = Snapshot::new();
        add(
            &mut files,
            "notes/1.json".into(),
            json!({"id":1,"host":"host","project":"missing"}),
        );
        assert!(prepare(&files, std::slice::from_ref(&a), "host")
            .unwrap_err()
            .contains("unmapped note"));
        files.clear();
        let mut fork = mapping("fork", "/fork/.git", "/fork");
        fork.project = serde_json::from_value(json!("github.com/other/fork")).unwrap();
        claim(&mut files, &a, "p1", 10);
        claim(&mut files, &fork, "p2", 10);
        assert_eq!(prepare(&files, &[a, fork], "host").unwrap().writes.len(), 2);
    }
    #[test]
    fn owning_host_aliases_are_explicit_and_never_inferred() {
        let m = mapping("~/a/.git", "/a/.git", "/a");
        let mut snapshot = Snapshot::new();
        claim(&mut snapshot, &m, "p1", 10);
        assert!(prepare(&snapshot, std::slice::from_ref(&m), "registry-alias").is_err());
        let plan = prepare_with_aliases(
            &snapshot,
            std::slice::from_ref(&m),
            "registry-alias",
            &["host"],
        )
        .unwrap();
        assert_eq!(
            value(&plan.writes[&destination("uac", &m.project)]).unwrap()["host"],
            "registry-alias"
        );
        assert!(
            prepare_with_aliases(&snapshot, &[m], "registry-alias", &["foreign-host"]).is_err()
        );
    }
    #[test]
    fn one_common_directory_can_retain_several_local_checkouts() {
        let a = mapping("~/a/.git", "/a/.git", "/a");
        let linked = Mapping {
            location: RepoLocation {
                common_dir: a.location.common_dir.clone(),
                checkout: "/a-linked".into(),
            },
            ..a.clone()
        };
        let mut files = Snapshot::new();
        claim(&mut files, &a, "p1", 10);
        let plan = prepare(&files, &[a.clone(), linked], "host").unwrap();
        let projected = value(&plan.writes[&destination("uac", &a.project)]).unwrap();
        assert_eq!(projected["locations"].as_array().unwrap().len(), 2);
    }

    const OLD: &str = "local:host:/r/.git";
    fn v2(kind: &str, id: &str) -> PathBuf {
        Path::new(kind)
            .join("v2")
            .join(format!("{}.json", encode_component(id)))
    }
    fn rekeyed() -> (Snapshot, ProjectId) {
        let to: ProjectId = serde_json::from_value(json!("git.example/r")).unwrap();
        let mut s = Snapshot::new();
        add(
            &mut s,
            v2("uac", OLD),
            json!({"schema":2,"project":OLD,"host":"host","pane":"p1","at":20,"tracker":"t-1","common_dir":"/r/.git","checkout":"/r","locations":[{"common_dir":"/r/.git","checkout":"/r"}],"folder_name":"r"}),
        );
        add(
            &mut s,
            v2("uac-status", OLD),
            json!({"schema":2,"project":OLD,"at":5}),
        );
        add(
            &mut s,
            v2("badges", OLD),
            json!({"schema":2,"project":OLD,"at":5,"notes":1}),
        );
        s.insert(
            v2("lanes", OLD),
            format!(
                "{}\n",
                json!({"actor":"host/r","since":3,"pane":"p1","schema":2,"project":OLD})
            )
            .into_bytes(),
        );
        add(
            &mut s,
            "notes/7.json".into(),
            json!({"schema":2,"id":7,"host":"host","project":OLD,"cause":format!("pagerduty-config:{OLD}"),"resolved":null,"title":"t","body":"b"}),
        );
        add(
            &mut s,
            "notes/8.json".into(),
            json!({"schema":2,"id":8,"host":"host","project":"other","title":"t"}),
        );
        add(
            &mut s,
            "hygiene.json".into(),
            json!({"schema":2,"daily":{OLD:{"day":"d"}}}),
        );
        add(
            &mut s,
            "pagerduty/state.json".into(),
            json!({"version":2,"incidents":{"a":{"delivered":{"to":{"Board":{"project":OLD}}}},"b":{"delivered":null}}}),
        );
        s.insert(
            Path::new("schedule")
                .join(encode_component(OLD))
                .join("job/state.json"),
            b"{}".to_vec(),
        );
        (s, to)
    }

    #[test]
    fn rekey_moves_every_record_and_keeps_durable_fields() {
        let (s, to) = rekeyed();
        let plan = rekey(&s, OLD, &to).unwrap();
        let claim = value(&plan.writes[&v2("uac", &to)]).unwrap();
        assert_eq!(
            (
                claim["project"].as_str(),
                claim["tracker"].as_str(),
                claim["at"].as_i64()
            ),
            (Some(to.as_str()), Some("t-1"), Some(20))
        );
        assert_eq!(value(&plan.writes[&v2("badges", &to)]).unwrap()["notes"], 1);
        let lanes = String::from_utf8(plan.writes[&v2("lanes", &to)].clone()).unwrap();
        assert!(lanes.contains("\"since\":3") && lanes.contains(to.as_str()));
        let note = value(&plan.writes[Path::new("notes/7.json")]).unwrap();
        assert_eq!(note["project"], to.as_str());
        assert_eq!(
            note["cause"],
            format!("pagerduty-config:{OLD}"),
            "cause kept"
        );
        assert!(!plan.writes.contains_key(Path::new("notes/8.json")));
        assert_eq!(
            value(&plan.writes[Path::new("hygiene.json")]).unwrap()["daily"][to.as_str()]["day"],
            "d"
        );
        let pd = value(&plan.writes[Path::new("pagerduty/state.json")]).unwrap();
        assert_eq!(
            pd["incidents"]["a"]["delivered"]["to"]["Board"]["project"],
            to.as_str()
        );
        assert!(
            pd["incidents"]["b"]["delivered"].is_null(),
            "undelivered untouched"
        );
        assert!(plan.writes.contains_key(
            &Path::new("schedule")
                .join(to.encoded())
                .join("job/state.json")
        ));
        for kind in ["uac", "uac-status", "badges", "lanes"] {
            assert!(plan.remove.contains(&v2(kind, OLD)), "{kind}");
        }
        assert!(rekey(&s, OLD, &serde_json::from_value(json!(OLD)).unwrap()).is_err());
    }

    #[test]
    fn rekey_joins_only_without_data_loss() {
        let (mut s, to) = rekeyed();
        // A manual re-claim by the same pane merges; caches keep the new record.
        add(
            &mut s,
            v2("uac", to.as_str()),
            json!({"schema":2,"project":to,"host":"host","pane":"p1","at":30,"tracker":"t-1","common_dir":"/r/.git","checkout":"/r","folder_name":"r","aliases":["r2"]}),
        );
        add(
            &mut s,
            v2("badges", to.as_str()),
            json!({"schema":2,"project":to,"at":9,"notes":0}),
        );
        s.insert(
            v2("lanes", to.as_str()),
            format!(
                "{}\n",
                json!({"actor":"host/x","since":1,"pane":"p1","schema":2,"project":to})
            )
            .into_bytes(),
        );
        let plan = rekey(&s, OLD, &to).unwrap();
        let claim = value(&plan.writes[&v2("uac", &to)]).unwrap();
        assert_eq!(claim["at"], 20, "earliest claim time");
        assert!(claim["aliases"].as_array().unwrap().contains(&json!("r2")));
        assert!(
            !plan.writes.contains_key(&v2("badges", &to)),
            "new cache kept"
        );
        assert!(plan.remove.contains(&v2("badges", OLD)));
        assert_eq!(
            String::from_utf8(plan.writes[&v2("lanes", &to)].clone())
                .unwrap()
                .lines()
                .count(),
            2
        );
        // Another pane, another tracker or differing schedule bytes block.
        add(
            &mut s,
            v2("uac", to.as_str()),
            json!({"schema":2,"project":to,"host":"host","pane":"p9","at":30}),
        );
        assert!(rekey(&s, OLD, &to)
            .unwrap_err()
            .contains("conflicting join"));
        add(
            &mut s,
            v2("uac", to.as_str()),
            json!({"schema":2,"project":to,"host":"host","pane":"p1","at":30,"tracker":"t-2"}),
        );
        assert!(rekey(&s, OLD, &to).unwrap_err().contains("tracker"));
        s.remove(&v2("uac", to.as_str()));
        s.insert(
            Path::new("schedule")
                .join(to.encoded())
                .join("job/state.json"),
            b"[]".to_vec(),
        );
        assert!(rekey(&s, OLD, &to).unwrap_err().contains("schedule"));
    }
}
