//! The reminder store under `$XDG_STATE_HOME/vega/reminders/`: one
//! definition `<id>.json` per reminder (written by the CLI, deleted by
//! `cancel` or by vegad once a one-shot is done) and its firing state
//! `state/<id>.json` (a [`SlotState`], written by vegad only), so the CLI
//! and vegad never write the same file.

use std::fs;
use std::path::{Path, PathBuf};

use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::slot::SlotState;
use crate::when::When;
use crate::{create_new, read_json, remove_if_exists, to_json, write_atomic, Error};

/// Text limit in bytes.
pub const MAX_TEXT: usize = 2048;
/// Active reminders per host.
pub const MAX_ACTIVE: usize = 100;

/// Who gets the mail.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase", deny_unknown_fields)]
pub enum Target {
    /// Resolved once at creation; one-shot reminders only.
    Pane {
        pane: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        host: Option<String>,
    },
    /// A herdr agent name on this host, resolved at each firing.
    Name { name: String },
    /// The project's UAC, resolved at each firing.
    Uac { project: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Reminder {
    pub id: String,
    pub when: When,
    pub to: Target,
    pub text: String,
    /// The pane that set it, and its host (the owner).
    pub by: String,
    pub host: String,
    pub created: Timestamp,
    /// `--now`: deliver with urgency.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub now: bool,
}

fn invalid(msg: impl Into<String>) -> Error {
    Error::new("reminder_invalid", msg)
}

impl Reminder {
    pub fn validate(&self) -> Result<(), Error> {
        if !valid_id(&self.id) {
            return Err(invalid(format!("id {:?}: want r + 8 hex digits", self.id)));
        }
        if self.text.trim().is_empty() {
            return Err(invalid("empty text"));
        }
        if self.text.len() > MAX_TEXT {
            return Err(invalid(format!(
                "text is {} bytes; at most {MAX_TEXT}",
                self.text.len()
            )));
        }
        if self.when.is_recurring() && matches!(self.to, Target::Pane { .. }) {
            return Err(invalid(
                "a recurring reminder takes an agent name or --uac, not a pane id (panes do not outlive their agent)",
            ));
        }
        Ok(())
    }

    /// The state a fresh reminder starts from: nothing before its creation.
    pub fn initial_state(&self) -> SlotState {
        SlotState::new(self.created)
    }
}

fn valid_id(id: &str) -> bool {
    id.len() == 9
        && id.starts_with('r')
        && id[1..]
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// `r` + 8 hex digits from the creation instant, pid and text.
pub fn new_id(created: Timestamp, text: &str) -> String {
    let mut h = Sha256::new();
    h.update(created.as_nanosecond().to_be_bytes());
    h.update(std::process::id().to_be_bytes());
    h.update(text.as_bytes());
    let d = h.finalize();
    format!(
        "r{}",
        d[..4]
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    )
}

/// Definitions read from the store; `broken` names files that do not parse
/// (never skipped silently).
#[derive(Debug, Default)]
pub struct Listing {
    pub reminders: Vec<Reminder>,
    pub broken: Vec<(PathBuf, String)>,
}

#[derive(Debug, Clone)]
pub struct Store {
    pub root: PathBuf,
}

impl Store {
    pub fn new(root: PathBuf) -> Self {
        Store { root }
    }

    /// `$XDG_STATE_HOME/vega/reminders`.
    pub fn default_root() -> Option<PathBuf> {
        crate::state_dir().map(|d| d.join("reminders"))
    }

    fn def_path(&self, id: &str) -> PathBuf {
        self.root.join(format!("{id}.json"))
    }

    fn state_path(&self, id: &str) -> PathBuf {
        self.root.join("state").join(format!("{id}.json"))
    }

    fn checked(id: &str) -> Result<&str, Error> {
        if valid_id(id) {
            Ok(id)
        } else {
            Err(Error::new(
                "reminder_unknown",
                format!("{id:?} is no reminder id"),
            ))
        }
    }

    /// CLI: stores a new definition. `reminder_limit` at [`MAX_ACTIVE`].
    pub fn create(&self, r: &Reminder) -> Result<(), Error> {
        r.validate()?;
        let l = self.list()?;
        if l.reminders.len() + l.broken.len() >= MAX_ACTIVE {
            return Err(Error::new(
                "reminder_limit",
                format!("{MAX_ACTIVE} active reminders on this host; cancel some (vega post reminders --all)"),
            ));
        }
        if !create_new(&self.def_path(&r.id), &to_json(r))? {
            return Err(invalid(format!("{} exists", r.id)));
        }
        Ok(())
    }

    /// Re-read right before a firing: `None` once cancelled.
    pub fn get(&self, id: &str) -> Result<Option<Reminder>, Error> {
        read_json(&self.def_path(Self::checked(id)?))
    }

    /// CLI: deletes the definition; `false` if there was none. The state file
    /// is vegad's to remove ([`Store::prune_states`]).
    pub fn cancel(&self, id: &str) -> Result<bool, Error> {
        remove_if_exists(&self.def_path(Self::checked(id)?))
    }

    pub fn list(&self) -> Result<Listing, Error> {
        let mut out = Listing::default();
        let entries = match fs::read_dir(&self.root) {
            Ok(e) => e,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(out),
            Err(e) => return Err(Error::io(&self.root, e)),
        };
        let mut paths: Vec<PathBuf> = entries
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| is_def(p))
            .collect();
        paths.sort();
        for p in paths {
            match read_json::<Reminder>(&p) {
                Ok(Some(r))
                    if r.validate().is_ok() && p.file_stem().is_some_and(|s| *s == *r.id) =>
                {
                    out.reminders.push(r)
                }
                Ok(Some(_)) => out.broken.push((p, "invalid definition".into())),
                Ok(None) => {} // cancelled meanwhile
                Err(e) => out.broken.push((p, e.message)),
            }
        }
        Ok(out)
    }

    /// vegad: the firing state, or the initial one for a new reminder.
    pub fn state(&self, r: &Reminder) -> Result<SlotState, Error> {
        Ok(read_json(&self.state_path(&r.id))?.unwrap_or_else(|| r.initial_state()))
    }

    /// vegad only.
    pub fn save_state(&self, id: &str, s: &SlotState) -> Result<(), Error> {
        write_atomic(&self.state_path(Self::checked(id)?), &to_json(s))
    }

    /// vegad: a one-shot whose slot is `last_slot` is deleted with its state.
    pub fn finish_once(&self, id: &str) -> Result<(), Error> {
        remove_if_exists(&self.def_path(Self::checked(id)?))?;
        remove_if_exists(&self.state_path(id))?;
        Ok(())
    }

    /// vegad: removes state files whose definition is gone (cancelled).
    pub fn prune_states(&self) -> Result<Vec<String>, Error> {
        let dir = self.root.join("state");
        let entries = match fs::read_dir(&dir) {
            Ok(e) => e,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
            Err(e) => return Err(Error::io(&dir, e)),
        };
        let mut removed = vec![];
        for p in entries
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| is_def(p))
        {
            let id = p
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or_default()
                .to_string();
            if !self.def_path(&id).exists() && remove_if_exists(&p)? {
                removed.push(id);
            }
        }
        removed.sort();
        Ok(removed)
    }
}

fn is_def(p: &Path) -> bool {
    p.extension().is_some_and(|x| x == "json")
        && p.file_stem().and_then(|s| s.to_str()).is_some_and(valid_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::slot::Step;
    use crate::when::tests::{berlin, ts};

    fn reminder(id: &str, when: When, to: Target) -> Reminder {
        Reminder {
            id: id.into(),
            when,
            to,
            text: "check the deploy".into(),
            by: "w3V:p1".into(),
            host: "proteus".into(),
            created: ts("2026-10-24T05:00:00Z"),
            now: false,
        }
    }

    fn uac() -> Target {
        Target::Uac {
            project: "github.com/o/brainz".into(),
        }
    }

    #[test]
    fn ids() {
        let id = new_id(ts("2026-10-24T05:00:00Z"), "x");
        assert!(valid_id(&id), "{id}");
        assert_ne!(id, new_id(ts("2026-10-24T05:00:00Z"), "y"));
        for bad in ["r1234567", "r1234567g", "x12345678", "r../../xx"] {
            assert!(!valid_id(bad), "{bad}");
        }
    }

    #[test]
    fn validation() {
        let at = When::At(ts("2026-10-24T06:00:00Z"));
        let pane = Target::Pane {
            pane: "w3V:p1".into(),
            host: None,
        };
        assert!(reminder("r00000001", at, pane.clone()).validate().is_ok());
        let e = reminder("r00000001", When::daily("08:30").unwrap(), pane)
            .validate()
            .unwrap_err();
        assert!(e.message.contains("recurring"), "{}", e.message);
        let mut r = reminder("r00000001", at, uac());
        r.text = "x".repeat(MAX_TEXT + 1);
        assert!(r.validate().is_err());
        r.text = " ".into();
        assert!(r.validate().is_err());
    }

    #[test]
    fn create_list_cancel() {
        let dir = tempfile::tempdir().unwrap();
        let s = Store::new(dir.path().join("reminders"));
        assert!(s.list().unwrap().reminders.is_empty());
        let r = reminder("r00000001", When::weekly("mon 09:00").unwrap(), uac());
        s.create(&r).unwrap();
        assert_eq!(s.create(&r).unwrap_err().code, "reminder_invalid", "exists");
        let json = fs::read_to_string(dir.path().join("reminders/r00000001.json")).unwrap();
        assert!(json.contains("\"when\": \"weekly mon 09:00\""), "{json}");
        assert!(json.contains("\"kind\": \"uac\""), "{json}");
        assert_eq!(s.get("r00000001").unwrap(), Some(r.clone()));
        fs::write(dir.path().join("reminders/r0000000f.json"), "{").unwrap();
        let l = s.list().unwrap();
        assert_eq!(l.reminders, vec![r]);
        assert_eq!(l.broken.len(), 1);
        assert!(s.cancel("r00000001").unwrap());
        assert!(!s.cancel("r00000001").unwrap());
        assert_eq!(s.get("r00000001").unwrap(), None);
        assert_eq!(s.cancel("../x").unwrap_err().code, "reminder_unknown");
    }

    #[test]
    fn limit() {
        let dir = tempfile::tempdir().unwrap();
        let s = Store::new(dir.path().to_path_buf());
        for i in 0..MAX_ACTIVE {
            s.create(&reminder(
                &format!("r{i:08x}"),
                When::daily("08:30").unwrap(),
                uac(),
            ))
            .unwrap();
        }
        let e = s
            .create(&reminder("rffffffff", When::daily("08:30").unwrap(), uac()))
            .unwrap_err();
        assert_eq!(e.code, "reminder_limit");
        assert!(e.message.contains("vega post reminders"));
    }

    #[test]
    fn one_shot_lifecycle_with_cancel_check() {
        let dir = tempfile::tempdir().unwrap();
        let s = Store::new(dir.path().to_path_buf());
        let r = reminder("r00000002", When::At(ts("2026-10-24T06:00:00Z")), uac());
        s.create(&r).unwrap();
        let tz = berlin();
        let mut st = s.state(&r).unwrap();
        assert_eq!(
            st.step(&r.when, &tz, ts("2026-10-24T05:59:30Z"), true),
            Step::Idle
        );
        let Step::Fire { slot, late } = st.step(&r.when, &tz, ts("2026-10-24T06:00:10Z"), true)
        else {
            panic!()
        };
        assert!(!late);
        st.begin(slot);
        s.save_state(&r.id, &st).unwrap();
        // Publish failed: still pending after a reload, retried each tick.
        let mut st = s.state(&r).unwrap();
        assert_eq!(
            st.step(&r.when, &tz, ts("2026-10-24T06:00:40Z"), true),
            Step::Resume(slot)
        );
        st.finish(slot);
        s.save_state(&r.id, &st).unwrap();
        assert!(st.is_done(&r.when));
        s.finish_once(&r.id).unwrap();
        assert!(s.list().unwrap().reminders.is_empty());
        assert!(fs::read_dir(dir.path().join("state"))
            .unwrap()
            .next()
            .is_none());
    }

    #[test]
    fn prune_states_of_cancelled() {
        let dir = tempfile::tempdir().unwrap();
        let s = Store::new(dir.path().to_path_buf());
        let r = reminder("r00000003", When::daily("08:30").unwrap(), uac());
        s.create(&r).unwrap();
        s.save_state(&r.id, &r.initial_state()).unwrap();
        assert!(s.prune_states().unwrap().is_empty());
        s.cancel(&r.id).unwrap();
        assert_eq!(s.prune_states().unwrap(), vec!["r00000003"]);
    }
}
