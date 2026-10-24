//! Per job files under `$XDG_STATE_HOME/vega/schedule/<hex project id>/<job>/`:
//! `state.json` (the [`SlotState`], written by vegad) and one run file
//! `<UTC slot>.json` plus `<UTC slot>.log` per run (written by the runner).

use std::fs;
use std::path::{Path, PathBuf};

use jiff::{SignedDuration, Timestamp};
use serde::{Deserialize, Serialize};

use crate::slot::SlotState;
use crate::{
    create_new, encode_component, read_json, remove_if_exists, to_json, write_atomic, Error,
};

/// Pruning keeps the last this many runs ...
pub const KEEP_RUNS: usize = 20;
/// ... and none older than this.
pub const KEEP_FOR: SignedDuration = SignedDuration::from_hours(30 * 24);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    /// The runner took the slot; the script runs or the runner died.
    Started,
    Succeeded,
    /// Exit code ≠ 0 or killed by a signal.
    Failed,
    Timeout,
    /// Still `started` while nobody holds the job's lock.
    Lost,
    SpawnError,
    /// Not run: `reason` says why (`running`, `modified: <path>`,
    /// `checkout on <branch>`, `missed`).
    Skipped,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunFile {
    pub slot: Timestamp,
    pub started: Timestamp,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended: Option<Timestamp>,
    pub outcome: Outcome,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signal: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl RunFile {
    pub fn started(slot: Timestamp, now: Timestamp) -> Self {
        RunFile {
            slot,
            started: now,
            ended: None,
            outcome: Outcome::Started,
            exit_code: None,
            signal: None,
            reason: None,
        }
    }
}

/// `20261024T053000Z`: sortable and safe in a file name.
pub fn slot_name(slot: Timestamp) -> String {
    slot.strftime("%Y%m%dT%H%M%SZ").to_string()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobDir {
    pub dir: PathBuf,
}

impl JobDir {
    /// `state_root` is `$XDG_STATE_HOME/vega` ([`crate::state_dir`]).
    pub fn new(state_root: &Path, project_id: &str, job: &str) -> Self {
        JobDir {
            dir: state_root
                .join("schedule")
                .join(encode_component(project_id))
                .join(job),
        }
    }

    pub fn state_path(&self) -> PathBuf {
        self.dir.join("state.json")
    }

    pub fn load_state(&self) -> Result<Option<SlotState>, Error> {
        read_json(&self.state_path())
    }

    pub fn save_state(&self, s: &SlotState) -> Result<(), Error> {
        write_atomic(&self.state_path(), &to_json(s))
    }

    pub fn run_path(&self, slot: Timestamp) -> PathBuf {
        self.dir.join(format!("{}.json", slot_name(slot)))
    }

    pub fn log_path(&self, slot: Timestamp) -> PathBuf {
        self.dir.join(format!("{}.log", slot_name(slot)))
    }

    /// Creates the slot's run file; `Ok(false)` if it exists (a respawned
    /// runner exits at once).
    pub fn create_run(&self, run: &RunFile) -> Result<bool, Error> {
        create_new(&self.run_path(run.slot), &to_json(run))
    }

    /// Fills in an existing run file.
    pub fn write_run(&self, run: &RunFile) -> Result<(), Error> {
        write_atomic(&self.run_path(run.slot), &to_json(run))
    }

    pub fn read_run(&self, slot: Timestamp) -> Result<Option<RunFile>, Error> {
        read_json(&self.run_path(slot))
    }

    /// All run files, oldest slot first.
    pub fn runs(&self) -> Result<Vec<RunFile>, Error> {
        let mut runs = vec![];
        for p in self.run_files()? {
            runs.extend(read_json::<RunFile>(&p)?);
        }
        runs.sort_by_key(|r| r.slot);
        Ok(runs)
    }

    fn run_files(&self) -> Result<Vec<PathBuf>, Error> {
        let entries = match fs::read_dir(&self.dir) {
            Ok(e) => e,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
            Err(e) => return Err(Error::io(&self.dir, e)),
        };
        let mut files: Vec<PathBuf> = entries
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| {
                p.extension().is_some_and(|x| x == "json")
                    && p.file_stem()
                        .and_then(|s| s.to_str())
                        .is_some_and(is_slot_name)
            })
            .collect();
        files.sort();
        Ok(files)
    }

    /// Keeps the last [`KEEP_RUNS`] runs and none older than [`KEEP_FOR`]
    /// (by slot); removes run file and log together. Returns the removed
    /// slot names.
    pub fn prune(&self, now: Timestamp) -> Result<Vec<String>, Error> {
        let files = self.run_files()?;
        let cutoff = now.checked_sub(KEEP_FOR).unwrap_or(Timestamp::MIN);
        let keep_from = files.len().saturating_sub(KEEP_RUNS);
        let mut removed = vec![];
        for (i, p) in files.iter().enumerate() {
            let name = p
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or_default()
                .to_string();
            let old = parse_slot_name(&name).is_some_and(|t| t < cutoff);
            if i < keep_from || old {
                remove_if_exists(p)?;
                remove_if_exists(&p.with_extension("log"))?;
                removed.push(name);
            }
        }
        Ok(removed)
    }
}

fn is_slot_name(s: &str) -> bool {
    parse_slot_name(s).is_some()
}

fn parse_slot_name(s: &str) -> Option<Timestamp> {
    Timestamp::strptime(
        "%Y%m%dT%H%M%S%z",
        s.strip_suffix('Z')?.to_string() + "+0000",
    )
    .ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::when::tests::ts;

    #[test]
    fn layout_and_slot_names() {
        let d = JobDir::new(Path::new("/s/vega"), "github.com/o/r", "fristen");
        assert_eq!(
            d.run_path(ts("2026-10-24T05:30:00Z")),
            PathBuf::from(
                "/s/vega/schedule/6769746875622e636f6d2f6f2f72/fristen/20261024T053000Z.json"
            )
        );
        assert_eq!(
            parse_slot_name("20261024T053000Z"),
            Some(ts("2026-10-24T05:30:00Z"))
        );
        assert!(!is_slot_name("state"));
    }

    #[test]
    fn run_file_is_created_once() {
        let root = tempfile::tempdir().unwrap();
        let d = JobDir::new(root.path(), "p", "j");
        let slot = ts("2026-10-24T05:30:00Z");
        let mut run = RunFile::started(slot, slot);
        assert!(d.create_run(&run).unwrap());
        assert!(!d.create_run(&run).unwrap(), "a respawn finds it");
        run.outcome = Outcome::Failed;
        run.exit_code = Some(3);
        d.write_run(&run).unwrap();
        assert_eq!(d.read_run(slot).unwrap(), Some(run.clone()));
        assert_eq!(d.runs().unwrap(), vec![run]);
        assert_eq!(d.load_state().unwrap(), None);
        let s = SlotState::new(slot);
        d.save_state(&s).unwrap();
        assert_eq!(d.load_state().unwrap(), Some(s));
        assert_eq!(d.runs().unwrap().len(), 1, "state.json is no run");
    }

    #[test]
    fn prune_keeps_twenty_and_thirty_days() {
        let root = tempfile::tempdir().unwrap();
        let d = JobDir::new(root.path(), "p", "j");
        let now = ts("2026-10-24T12:00:00Z");
        // 25 daily runs, the oldest 24 days ago; plus one 40 days ago.
        let mut slots: Vec<Timestamp> = (0..25)
            .map(|k| now.checked_sub(SignedDuration::from_hours(24 * k)).unwrap())
            .collect();
        slots.push(
            now.checked_sub(SignedDuration::from_hours(24 * 40))
                .unwrap(),
        );
        for s in &slots {
            d.create_run(&RunFile::started(*s, *s)).unwrap();
            fs::write(d.log_path(*s), "x").unwrap();
        }
        let removed = d.prune(now).unwrap();
        assert_eq!(removed.len(), 6);
        let runs = d.runs().unwrap();
        assert_eq!(runs.len(), KEEP_RUNS);
        assert_eq!(runs.last().unwrap().slot, now);
        assert!(!d.log_path(slots[25]).exists());
        assert!(d.log_path(now).exists());
    }
}
