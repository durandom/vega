//! UAC lanes: the worktrees a project's UAC runs sentinels in. `vega mux
//! spawn` and `vega uac adopt` record one when the UAC starts or adopts a
//! sentinel; `vega uac status` labels the lane's beads.
//!
//! bd records a bead's creator as its actor, `BEADS_ACTOR`: `<host>/<worktree
//! dir name>`, which `vega mux spawn` exports into the agent's env (`actor`)
//! and the dotfiles bd shim derives the same way (without either it is the
//! git user name and nothing matches). An adopted agent keeps the env it was
//! started with, so it needs the shim. So a lane's beads
//! are the ones its actor created after the lane started: the sentinel does
//! nothing special, and the project's hierarchy stays as it is.
//!
//! Records live in `$XDG_STATE_HOME/vega/lanes/v2/<hex(project ID)>.json`, one JSON line
//! per lane, on the UAC's machine. A worktree dir reused later by a session
//! the UAC does not run still matches: the label is a hint, not a lock.

use std::io::Write;
use std::path::Path;
use std::process::Command;

use serde_json::{json, Value};

use crate::state::{project_file, project_uac_of};
use crate::{local_host, Failure};

/// The label `vega uac status` puts on the beads a lane created.
pub const LABEL: &str = "uac-lane";

/// One lane: the bd actor of its worktree and when it started (Unix seconds).
#[derive(Debug, Clone, PartialEq)]
pub struct Lane {
    pub actor: String,
    pub since: i64,
}

/// Resolve an owner-local directory to the full project ID.
pub fn project_key(dir: &Path) -> Result<String, Failure> {
    crate::project::resolve(dir, &crate::state::post_host()).map(|p| p.id.to_string())
}

/// The bd actor of `dir`, as the bd shim derives it: `<host>/<worktree dir>`.
pub fn actor(dir: &Path) -> Option<String> {
    let top = git(dir, &["rev-parse", "--show-toplevel"])?;
    let name = Path::new(&top).file_name()?.to_string_lossy().into_owned();
    Some(format!("{}/{name}", local_host()))
}

/// Records `dir` as a lane of its project when `me` (a pane on this machine)
/// is that project's UAC and `dir` is another worktree than `here` (the
/// UAC's own). The lane's actor, if recorded.
pub fn record_if_uac(dir: &Path, here: &Path, me: &str) -> Result<Option<String>, Failure> {
    let Some(project) = crate::project::resolve_optional(dir, &crate::state::post_host())? else {
        return Ok(None);
    };
    let key = project.id.to_string();
    let Some((pane, host)) = project_uac_of(&key) else {
        return Ok(None);
    };
    if pane != me || host != crate::state::post_host() {
        return Ok(None);
    }
    let Some(lane) = actor(dir) else {
        return Ok(None);
    };
    if actor(here).as_deref() == Some(lane.as_str()) {
        return Ok(None);
    }
    if !record(&key, &lane, me) {
        return Err(Failure::new(
            "state_io",
            "Could not record project lane",
            "check the vega state directory",
        ));
    }
    Ok(Some(lane))
}

/// Appends a lane of project `key`; false if the record could not be written.
pub fn record(key: &str, actor: &str, pane: &str) -> bool {
    let Some(file) = project_file("lanes", key) else {
        return false;
    };
    let _ = file.parent().map(std::fs::create_dir_all);
    let line =
        json!({"schema": 2, "project": key, "actor": actor, "since": unix_now(), "pane": pane})
            .to_string();
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(file)
        .and_then(|mut f| writeln!(f, "{line}"))
        .is_ok()
}

/// The lanes of project `key`, one per actor (its first start).
pub fn lanes(key: &str) -> Vec<Lane> {
    project_file("lanes", key)
        .and_then(|f| std::fs::read_to_string(f).ok())
        .map(|t| parse(&t, key))
        .unwrap_or_default()
}

pub(crate) fn parse(text: &str, key: &str) -> Vec<Lane> {
    let mut out: Vec<Lane> = Vec::new();
    for v in text
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
    {
        if v["schema"] != 2 || v["project"] != key {
            continue;
        }
        let (Some(actor), Some(since)) = (v["actor"].as_str(), v["since"].as_i64()) else {
            continue;
        };
        match out.iter_mut().find(|l| l.actor == actor) {
            Some(l) => l.since = l.since.min(since),
            None => out.push(Lane {
                actor: actor.into(),
                since,
            }),
        }
    }
    out
}

fn git(dir: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lanes_are_one_per_actor_from_its_first_start() {
        let text = r#"{"actor":"proteus/fix","since":200,"pane":"w1:p1"}
not json
{"actor":"proteus/docs","since":150,"pane":"w1:p1"}
{"actor":"proteus/fix","since":100,"pane":"w1:p1"}
{"since":50}"#;
        assert_eq!(
            parse(
                &text.replace("{", "{\"schema\":2,\"project\":\"github.com/team/repo\","),
                "github.com/team/repo"
            ),
            vec![
                Lane {
                    actor: "proteus/fix".into(),
                    since: 100
                },
                Lane {
                    actor: "proteus/docs".into(),
                    since: 150
                },
            ]
        );
    }

    #[test]
    fn no_repo_has_no_actor_or_project() {
        let dir = std::env::temp_dir().join(format!("vega-lanes-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(actor(&dir), None);
        assert!(project_key(&dir).is_err());
        let _ = std::fs::remove_dir(&dir);
    }
}
