//! Pure projection for the journaled startup cutover (.14.6). The caller
//! snapshots files, resolves old keys locally, persists this plan and backups,
//! and applies/removes files while all writers are quiesced. No Dolt data is read
//! or merged here; historical backup directories are outside this projection.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use vega_herdr::project::{ProjectId, RepoLocation};

use crate::git::GitState;
use crate::store::{self, Project, Record, State};

/// Paths are relative to the local vega/bd state directory. Preserve the exact
/// snapshot in the startup journal, including absent files, before applying.
pub type Snapshot = BTreeMap<PathBuf, Vec<u8>>;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Mapping {
    pub old_key: String,
    pub project: ProjectId,
    pub location: RepoLocation,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Plan {
    /// Full v2 file bytes, safe to replay from a durable journal.
    pub writes: Snapshot,
    /// Old files to remove only after the backed-up projection is durable.
    pub remove: BTreeSet<PathBuf>,
}

/// This lossy spelling is used ONLY to locate a legacy snapshot file. Runtime
/// readers never use it, and the old key remains only in the migration journal.
fn legacy_file(kind: &str, key: &str) -> PathBuf {
    let safe: String = key
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || "-_.".contains(c) {
                c
            } else {
                '_'
            }
        })
        .collect();
    Path::new(kind).join(format!("{safe}.json"))
}

fn decode<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T, String> {
    serde_json::from_slice(bytes).map_err(|_| "invalid local sync snapshot record".into())
}

/// All old records must have a host-local mapping. Reject lossy-source
/// collisions, identity changes at one common directory, and incompatible
/// destination records before the caller changes any files.
pub fn prepare(snapshot: &Snapshot, mappings: &[Mapping]) -> Result<Plan, String> {
    let mut plan = Plan {
        writes: Snapshot::new(),
        remove: BTreeSet::new(),
    };
    let mut sources = BTreeMap::new();
    let mut locations = BTreeMap::new();
    let mut keys = BTreeMap::new();
    for m in mappings {
        if keys
            .insert(&m.old_key, (&m.project, &m.location.common_dir))
            .is_some_and(|pair| pair != (&m.project, &m.location.common_dir))
        {
            return Err("one legacy key maps to conflicting local records".into());
        }
        if !m.location.common_dir.is_absolute() || !m.location.checkout.is_absolute() {
            return Err("cutover requires canonical absolute local locations".into());
        }
        if locations
            .insert(&m.location.common_dir, &m.project)
            .is_some_and(|id| id != &m.project)
        {
            return Err("one local location maps to conflicting project IDs".into());
        }
        for kind in ["projects", "state", "git"] {
            let source = legacy_file(kind, &m.old_key);
            if sources
                .insert(source.clone(), &m.old_key)
                .is_some_and(|old| old != &m.old_key)
            {
                return Err(
                    "legacy sync filenames collide; resolve ownership before cutover".into(),
                );
            }
            let Some(bytes) = snapshot.get(&source) else {
                continue;
            };
            let value = match kind {
                "projects" => {
                    let mut v: serde_json::Value = decode(bytes)?;
                    if v["key"] != m.old_key {
                        return Err("legacy project key/payload mismatch".into());
                    }
                    v["key"] = m.project.as_str().into();
                    v["common"] = m
                        .location
                        .common_dir
                        .to_str()
                        .ok_or("non-UTF-8 location")?
                        .into();
                    // Validate without altering the database, data, execution
                    // checkout, remote, stop/resume or provisioning timestamps.
                    let p: Project =
                        serde_json::from_value(v.clone()).map_err(|_| "invalid legacy project")?;
                    if !p.dir.is_absolute() || !p.data.is_absolute() {
                        return Err("legacy project paths must be absolute".into());
                    }
                    v
                }
                "state" => {
                    serde_json::to_value(decode::<State>(bytes)?).map_err(|_| "invalid state")?
                }
                _ => serde_json::to_value(decode::<GitState>(bytes)?)
                    .map_err(|_| "invalid git state")?,
            };
            let dest = store::relative_file(kind, m.project.as_str(), &m.location.common_dir)
                .ok_or("invalid location")?;
            let record = Record {
                schema: 2,
                project: m.project.to_string(),
                common_dir: m.location.common_dir.clone(),
                value,
            };
            let output =
                serde_json::to_vec_pretty(&record).map_err(|_| "cannot encode sync projection")?;
            if let Some(existing) = snapshot.get(&dest).or_else(|| plan.writes.get(&dest)) {
                let a: serde_json::Value = decode(existing)?;
                let b: serde_json::Value = decode(&output)?;
                if a != b {
                    return Err("incompatible sync records at one project/location".into());
                }
            }
            plan.writes.insert(dest, output);
            plan.remove.insert(source);
        }
    }
    for path in snapshot.keys() {
        if path.extension().is_some_and(|ext| ext == "json")
            && path.components().count() == 2
            && ["projects", "state", "git"]
                .iter()
                .any(|kind| path.starts_with(kind))
            && !plan.remove.contains(path)
        {
            return Err("unmapped local sync record blocks cutover".into());
        }
    }
    Ok(plan)
}

/// Explicit v2-to-v2 rekey (vega-lhc.14.7): move every location's records of
/// `from` to `to`, unchanged apart from the ID. Returns the plan and the
/// registered checkouts, which the caller must resolve to `to` before
/// journaling. A differing record of `to` at the same location blocks.
pub fn rekey(
    snapshot: &Snapshot,
    from: &str,
    to: &ProjectId,
) -> Result<(Plan, BTreeSet<PathBuf>), String> {
    let mut plan = Plan {
        writes: Snapshot::new(),
        remove: BTreeSet::new(),
    };
    let mut checkouts = BTreeSet::new();
    for kind in ["projects", "state", "git"] {
        let dir = Path::new(kind)
            .join("v2")
            .join(vega_herdr::project::encode_component(from));
        for (path, bytes) in snapshot.range(dir.clone()..) {
            if !path.starts_with(&dir) {
                break;
            }
            let mut record: Record<serde_json::Value> = decode(bytes)?;
            if record.project != from {
                return Err(format!(
                    "sync record/filename mismatch at {}",
                    path.display()
                ));
            }
            record.project = to.to_string();
            if kind == "projects" {
                record.value["key"] = to.as_str().into();
                let p: Project = serde_json::from_value(record.value.clone())
                    .map_err(|_| "invalid project record")?;
                checkouts.insert(p.dir);
            }
            let dest = store::relative_file(kind, to.as_str(), &record.common_dir)
                .ok_or("invalid location")?;
            let output =
                serde_json::to_vec_pretty(&record).map_err(|_| "cannot encode sync record")?;
            if let Some(existing) = snapshot.get(&dest) {
                if decode::<serde_json::Value>(existing)? != decode::<serde_json::Value>(&output)? {
                    return Err(format!("conflicting join at bd/{}", dest.display()));
                }
            }
            plan.writes.insert(dest, output);
            plan.remove.insert(path.clone());
        }
    }
    Ok((plan, checkouts))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::GitState;
    use crate::store::State;

    fn mapping(old: &str, common: &str) -> Mapping {
        Mapping {
            old_key: old.into(),
            project: serde_json::from_value(serde_json::json!("github.com/team/repo")).unwrap(),
            location: RepoLocation {
                common_dir: common.into(),
                checkout: Path::new(common).parent().unwrap().into(),
            },
        }
    }

    fn snapshot(m: &Mapping, stopped: i64, fetched: i64) -> Snapshot {
        let p = serde_json::json!({ "key": m.old_key, "dir": m.location.checkout,
            "database": "ORIGINAL", "remote": "http://remote/ORIGINAL", "bd": "/bin/bd",
            "data": m.location.checkout.join(".beads/dolt"), "since": 10, "resumed": 20 });
        let s = State {
            stopped: Some(stopped),
            stop_reason: Some("conflict".into()),
            seen_write: 456,
            ..State::default()
        };
        let git = GitState {
            fetched,
            skip: Some("dirty".into()),
            ..GitState::default()
        };
        [
            ("projects", serde_json::to_vec(&p).unwrap()),
            ("state", serde_json::to_vec(&s).unwrap()),
            ("git", serde_json::to_vec(&git).unwrap()),
        ]
        .into_iter()
        .map(|(kind, bytes)| (legacy_file(kind, &m.old_key), bytes))
        .collect()
    }

    #[test]
    fn duplicate_clones_preserve_stop_resume_git_and_dolt_paths() {
        let a = mapping("~/a/.git", "/a/.git");
        let b = mapping("~/b/.bare", "/b/.bare");
        let mut files = snapshot(&a, 30, 101);
        files.extend(snapshot(&b, 15, 202));
        files.insert(
            "backup/historical/.beads/config.yaml".into(),
            b"untouched".to_vec(),
        );
        let original = files.clone();
        let plan = prepare(&files, &[a.clone(), b.clone()]).unwrap();
        assert_eq!(files, original, "preparation does not mutate snapshots");
        assert_eq!(plan.writes.len(), 6);
        assert_eq!(plan.remove.len(), 6);
        for (m, stopped, fetched) in [(&a, true, 101), (&b, false, 202)] {
            let at = |kind| {
                store::relative_file(kind, m.project.as_str(), &m.location.common_dir).unwrap()
            };
            let p: Record<Project> = decode(&plan.writes[&at("projects")]).unwrap();
            let s: Record<State> = decode(&plan.writes[&at("state")]).unwrap();
            let g: Record<GitState> = decode(&plan.writes[&at("git")]).unwrap();
            assert_eq!(s.value.is_stopped(&p.value), stopped);
            assert_eq!(p.value.database, "ORIGINAL");
            assert_eq!(p.value.data, m.location.checkout.join(".beads/dolt"));
            assert_eq!(s.value.seen_write, 456);
            assert_eq!(g.value.fetched, fetched);
            assert_eq!(g.value.skip.as_deref(), Some("dirty"));
        }
        // Persist/replay this exact plan after an interrupted startup; preparing
        // against the original snapshot plus partially written outputs is stable.
        let journal = serde_json::to_vec(&plan).unwrap();
        let restored: Plan = decode(&journal).unwrap();
        files.extend(restored.writes);
        assert_eq!(prepare(&files, &[a, b]).unwrap().writes, plan.writes);
    }

    #[test]
    fn conflicts_and_unmapped_sources_block_before_writing() {
        let a = mapping("a/b", "/a/.git");
        let b = mapping("a_b", "/b/.git");
        let files = snapshot(&a, 30, 101);
        assert!(prepare(&files, &[a.clone(), b])
            .unwrap_err()
            .contains("collide"));
        assert!(prepare(&files, &[]).unwrap_err().contains("unmapped"));
        let b = mapping("other", "/a/.git");
        let mut files = files;
        files.extend(snapshot(&b, 99, 202));
        assert!(prepare(&files, &[a.clone(), b])
            .unwrap_err()
            .contains("incompatible"));
        let mut b = a.clone();
        b.project = serde_json::from_value(serde_json::json!("github.com/team/fork")).unwrap();
        assert!(prepare(&files, &[a, b])
            .unwrap_err()
            .contains("conflicting"));
    }

    #[test]
    fn rekey_keeps_stop_flags_and_blocks_a_differing_join() {
        let m = mapping("~/r/.git", "/r/.git");
        let v2 = prepare(&snapshot(&m, 30, 101), std::slice::from_ref(&m))
            .unwrap()
            .writes;
        let to: ProjectId = serde_json::from_value(serde_json::json!("git.example/r")).unwrap();
        let (plan, checkouts) = rekey(&v2, m.project.as_str(), &to).unwrap();
        assert_eq!(checkouts, BTreeSet::from([PathBuf::from("/r")]));
        assert_eq!(plan.remove, v2.keys().cloned().collect());
        let at = |kind| store::relative_file(kind, to.as_str(), &m.location.common_dir).unwrap();
        let p: Record<Project> = decode(&plan.writes[&at("projects")]).unwrap();
        let s: Record<State> = decode(&plan.writes[&at("state")]).unwrap();
        assert_eq!(
            (p.project.as_str(), p.value.key.as_str()),
            (to.as_str(), to.as_str())
        );
        assert!(s.value.is_stopped(&p.value));
        assert_eq!(p.value.database, "ORIGINAL");
        // Same bytes under the new ID join; a differing record blocks.
        let mut joined = v2.clone();
        joined.extend(plan.writes.clone());
        assert!(rekey(&joined, m.project.as_str(), &to).is_ok());
        let mut other: Record<serde_json::Value> = decode(&plan.writes[&at("state")]).unwrap();
        other.value["seen_write"] = 1.into();
        joined.insert(at("state"), serde_json::to_vec(&other).unwrap());
        assert!(rekey(&joined, m.project.as_str(), &to)
            .unwrap_err()
            .contains("conflicting join"));
    }
}
