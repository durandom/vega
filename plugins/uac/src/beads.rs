//! The bead column: which in-progress bead an agent has claimed.
//!
//! Beads are optional. A repo has them when `bd where` succeeds in it; then
//! one `bd list` per repo gives the claims. The bd shim on our machines sets
//! the claim actor to `<host>/<worktree dir name>`, so an agent's bead is the
//! in-progress bead assigned to its host and worktree. No beads, no bd, no
//! shim (the assignee is then the git user name) or a slow bd: an empty cell.

use std::collections::HashMap;
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::thread;
use std::time::{Duration, Instant};

use serde_json::Value;

/// bd answers in well under a second; a locked database must not hang the view.
const TIMEOUT: Duration = Duration::from_secs(5);

/// Assignee -> ids of the in-progress beads claimed by it.
type Claims = HashMap<String, Vec<String>>;

/// Claims per git common dir, so worktrees of one repo share one `bd list`.
#[derive(Default)]
pub struct Lookup {
    repos: HashMap<String, Option<Claims>>,
}

impl Lookup {
    /// The bead the agent in `cwd` has claimed, e.g. `v-1` or `v-1 +2` for
    /// three claims, or "".
    pub fn bead(&mut self, cwd: &Path, host: &str) -> String {
        let (Some(top), Some(common)) = (
            git(cwd, &["rev-parse", "--show-toplevel"]),
            git_common_dir(cwd),
        ) else {
            return String::new();
        };
        let claims = self.repos.entry(common).or_insert_with(|| load_claims(cwd));
        match claims {
            Some(c) => bead_of(c, &actor(host, &top)),
            None => String::new(),
        }
    }
}

/// The claim actor the bd shim derives: `<host>/<worktree dir name>`.
fn actor(host: &str, top: &str) -> String {
    let dir = Path::new(top)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    format!("{host}/{dir}")
}

/// One id per cell: a worktree can hold many claims, and the list costs
/// tokens per row; `bd list --assignee` shows them all.
fn bead_of(claims: &Claims, actor: &str) -> String {
    match claims.get(actor).map(Vec::as_slice) {
        None | Some([]) => String::new(),
        Some([id]) => id.clone(),
        Some([id, rest @ ..]) => format!("{id} +{}", rest.len()),
    }
}

fn load_claims(cwd: &Path) -> Option<Claims> {
    run(cwd, "bd", &["where", "--json"])?;
    let list = run(
        cwd,
        "bd",
        &["list", "--status", "in_progress", "--json", "--limit", "0"],
    )?;
    parse_claims(&list)
}

fn parse_claims(json: &str) -> Option<Claims> {
    let v: Value = serde_json::from_str(json).ok()?;
    let mut claims = Claims::new();
    for issue in v.as_array()? {
        if let (Some(id), Some(who)) = (issue["id"].as_str(), issue["assignee"].as_str()) {
            claims.entry(who.into()).or_default().push(id.into());
        }
    }
    Some(claims)
}

/// One bead row for the status report.
pub struct Bead {
    pub id: String,
    pub title: String,
}

/// Whether the repo at `cwd` has beads.
pub fn has_beads(cwd: &Path) -> bool {
    run(cwd, "bd", &["where", "--json"]).is_some()
}

/// Beads closed after `rfc3339`, newest first.
pub fn closed_since(cwd: &Path, rfc3339: &str) -> Option<Vec<Bead>> {
    let json = run(
        cwd,
        "bd",
        &[
            "list",
            "--all",
            "--closed-after",
            rfc3339,
            "--sort",
            "closed",
            "--json",
            "--limit",
            "0",
            "--brief",
        ],
    )?;
    parse_beads(&json)
}

/// Open beads that wait for the Slayer (label `slayer-decision`).
pub fn decisions(cwd: &Path) -> Option<Vec<Bead>> {
    let json = run(
        cwd,
        "bd",
        &[
            "list",
            "--label",
            "slayer-decision",
            "--sort",
            "priority",
            "--json",
            "--limit",
            "0",
            "--brief",
        ],
    )?;
    parse_beads(&json)
}

/// One bead's title and whether it is open; `None` if bd does not know it.
pub fn show(cwd: &Path, id: &str) -> Option<(String, bool)> {
    let json = run(cwd, "bd", &["show", id, "--json"])?;
    let v: Value = serde_json::from_str(&json).ok()?;
    let b = v.as_array()?.first()?;
    Some((
        b["title"].as_str().unwrap_or("").to_string(),
        b["status"] != "closed",
    ))
}

/// The Slayer's answer to a decision: its last comment, if the Slayer wrote
/// it (`vega uac answer`). A later comment by anyone else means the answer
/// was taken up or questioned, so it no longer counts as unread.
pub fn slayer_answer(cwd: &Path, id: &str) -> Option<String> {
    let json = run(cwd, "bd", &["comments", id, "--json"])?;
    let v: Value = serde_json::from_str(&json).ok()?;
    let last = v.as_array()?.last()?;
    (last["author"] == "Slayer").then(|| last["text"].as_str().unwrap_or("").trim().to_string())
}

/// Adds a comment by `author`; false if bd refused.
pub fn comment(cwd: &Path, id: &str, author: &str, text: &str) -> bool {
    run(
        cwd,
        "bd",
        &["comments", "add", id, "--author", author, "--", text],
    )
    .is_some()
}

fn parse_beads(json: &str) -> Option<Vec<Bead>> {
    let v: Value = serde_json::from_str(json).ok()?;
    Some(
        v.as_array()?
            .iter()
            .filter_map(|i| {
                Some(Bead {
                    id: i["id"].as_str()?.to_string(),
                    title: i["title"].as_str().unwrap_or("").to_string(),
                })
            })
            .collect(),
    )
}

/// A bead as the lane label and the backlog line need it.
#[derive(Debug, Clone, Default)]
pub struct Row {
    pub id: String,
    pub created_by: String,
    pub created_at: String,
    pub closed_at: Option<String>,
    pub open: bool,
    pub labels: Vec<String>,
}

/// Every bead (closed too) created after `rfc3339`.
pub fn created_after(cwd: &Path, rfc3339: &str) -> Option<Vec<Row>> {
    rows(
        cwd,
        &[
            "list",
            "--all",
            "--created-after",
            rfc3339,
            "--json",
            "--limit",
            "0",
            "--brief",
        ],
    )
}

/// Every bead (closed too) with `label`.
pub fn labelled(cwd: &Path, label: &str) -> Option<Vec<Row>> {
    rows(
        cwd,
        &[
            "list", "--all", "--label", label, "--json", "--limit", "0", "--brief",
        ],
    )
}

/// Adds `label` to the beads `ids`; false if bd refused.
pub fn add_label(cwd: &Path, ids: &[String], label: &str) -> bool {
    let mut args = vec!["update"];
    args.extend(ids.iter().map(String::as_str));
    args.extend(["--add-label", label]);
    run(cwd, "bd", &args).is_some()
}

fn rows(cwd: &Path, args: &[&str]) -> Option<Vec<Row>> {
    parse_rows(&run(cwd, "bd", args)?)
}

fn parse_rows(json: &str) -> Option<Vec<Row>> {
    let v: Value = serde_json::from_str(json).ok()?;
    let s = |i: &Value, k: &str| i[k].as_str().unwrap_or("").to_string();
    Some(
        v.as_array()?
            .iter()
            .filter(|i| i["id"].is_string())
            .map(|i| Row {
                id: s(i, "id"),
                created_by: s(i, "created_by"),
                created_at: s(i, "created_at"),
                closed_at: i["closed_at"].as_str().map(String::from),
                open: i["status"] != "closed",
                labels: i["labels"]
                    .as_array()
                    .map(|l| {
                        l.iter()
                            .filter_map(|x| x.as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_default(),
            })
            .collect(),
    )
}

/// Label of the project's one UAC tracker epic.
pub const TRACKER_LABEL: &str = "uac-tracker";

/// The open UAC tracker epics (there should be exactly one).
pub fn trackers(cwd: &Path) -> Option<Vec<Bead>> {
    let json = run(
        cwd,
        "bd",
        &[
            "list",
            "--label",
            TRACKER_LABEL,
            "--json",
            "--limit",
            "0",
            "--brief",
        ],
    )?;
    let v: Value = serde_json::from_str(&json).ok()?;
    Some(
        v.as_array()?
            .iter()
            .filter(|i| i["issue_type"] == "epic")
            .filter_map(|i| {
                Some(Bead {
                    id: i["id"].as_str()?.to_string(),
                    title: i["title"].as_str().unwrap_or("").to_string(),
                })
            })
            .collect(),
    )
}

/// Creates the tracker epic; its id.
pub fn create_tracker(cwd: &Path, project: &str) -> Option<String> {
    let json = run(
        cwd,
        "bd",
        &[
            "create",
            "--title",
            &format!("UAC: {project}"),
            "--type",
            "epic",
            "--labels",
            TRACKER_LABEL,
            "--description",
            "The UAC's tracker for this project (vega uac claim). Questions for the Slayer are its children, labelled slayer-decision; metadata last_status is the time of the last `vega uac status`.",
            "--json",
        ],
    )?;
    let v: Value = serde_json::from_str(&json).ok()?;
    v["id"].as_str().map(String::from)
}

/// The tracker's last status time (RFC 3339), else its creation time.
pub fn tracker_since(cwd: &Path, id: &str) -> Option<String> {
    let json = run(cwd, "bd", &["show", id, "--json"])?;
    let v: Value = serde_json::from_str(&json).ok()?;
    let v = if v.is_array() { v[0].clone() } else { v };
    v["metadata"]["last_status"]
        .as_str()
        .or_else(|| v["created_at"].as_str())
        .map(String::from)
}

pub fn set_tracker_since(cwd: &Path, id: &str, rfc3339: &str) -> bool {
    run(
        cwd,
        "bd",
        &[
            "update",
            id,
            "--set-metadata",
            &format!("last_status={rfc3339}"),
        ],
    )
    .is_some()
}

/// `<hash> <subject>` of the default branch's commits since `unix`, newest
/// first (first parent only: one line per landed merge).
pub fn commits_since(cwd: &Path, unix: i64) -> Option<Vec<String>> {
    let branch = git(
        cwd,
        &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"],
    )
    .unwrap_or_else(|| "main".into());
    let out = run(
        cwd,
        "git",
        &[
            "log",
            "--first-parent",
            &format!("--since=@{unix}"),
            "--format=%h %s",
            &branch,
        ],
    )?;
    Some(out.lines().map(str::to_string).collect())
}

/// The absolute git common dir of the repo at `cwd` (herdr's `repo_key`).
pub fn git_common_dir(cwd: &Path) -> Option<String> {
    git(
        cwd,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )
}

fn git(cwd: &Path, args: &[&str]) -> Option<String> {
    run(cwd, "git", args).map(|s| s.trim().to_string())
}

/// stdout of a successful command in `cwd`, or `None` (missing, failed, timed out).
///
/// For `bd`, some releases (1.2) have no `--brief` and wrap `--json` output as
/// `{"data": ..., "schema_version": n}`: the flag is dropped where bd lacks it and the
/// envelope unwrapped, so callers see the bare JSON of other releases.
fn run(cwd: &Path, bin: &str, args: &[&str]) -> Option<String> {
    let args: Vec<&str> = args
        .iter()
        .copied()
        .filter(|a| bin != "bd" || *a != "--brief" || bd_has_brief(cwd))
        .collect();
    let out = run_raw(cwd, bin, &args)?;
    Some(if bin == "bd" {
        vega_plugin::unwrap_bd_envelope(&out)
    } else {
        out
    })
}

/// Whether this `bd` knows `list --brief`; asked once per process.
fn bd_has_brief(cwd: &Path) -> bool {
    static HAS: OnceLock<bool> = OnceLock::new();
    *HAS.get_or_init(|| {
        run_raw(cwd, "bd", &["list", "--help"]).is_some_and(|h| h.contains("--brief"))
    })
}

fn run_raw(cwd: &Path, bin: &str, args: &[&str]) -> Option<String> {
    let mut child = Command::new(bin)
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    // Read on a thread so a large list cannot block the child on a full pipe.
    let mut stdout = child.stdout.take()?;
    let reader = thread::spawn(move || {
        let mut buf = String::new();
        let _ = stdout.read_to_string(&mut buf);
        buf
    });
    let start = Instant::now();
    let status = loop {
        match child.try_wait().ok()? {
            Some(status) => break status,
            None if start.elapsed() < TIMEOUT => thread::sleep(Duration::from_millis(10)),
            None => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    };
    let out = reader.join().ok()?;
    status.success().then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn actor_is_host_and_worktree_dir() {
        assert_eq!(
            actor("proteus", "/Users/m/src/vega.uac"),
            "proteus/vega.uac"
        );
    }

    #[test]
    fn claims_map_assignee_to_beads() {
        let json = r#"[
            {"id": "v-1", "assignee": "proteus/vega.uac"},
            {"id": "v-2", "assignee": "proteus/vega.uac"},
            {"id": "v-3", "assignee": "Marcel Hild"},
            {"id": "v-4"}
        ]"#;
        let c = parse_claims(json).unwrap();
        assert_eq!(bead_of(&c, "proteus/vega.uac"), "v-1 +1");
        assert_eq!(bead_of(&c, "proteus/vega.skill"), "");
        assert!(parse_claims("not json").is_none());
    }

    #[test]
    fn rows_carry_creator_status_and_labels() {
        let json = r#"[
            {"id": "v-1", "created_by": "proteus/fix", "created_at": "2026-10-09T10:00:00Z",
             "status": "open", "labels": ["uac-lane"]},
            {"id": "v-2", "created_at": "2026-10-09T11:00:00+02:00", "status": "closed",
             "closed_at": "2026-10-09T12:00:00+02:00"},
            {"title": "no id"}
        ]"#;
        let r = parse_rows(json).unwrap();
        assert_eq!(r.len(), 2);
        assert!(r[0].open && r[0].labels == ["uac-lane"] && r[0].created_by == "proteus/fix");
        assert!(!r[1].open && r[1].closed_at.is_some() && r[1].labels.is_empty());
    }

    #[test]
    fn no_repo_or_no_beads_gives_an_empty_cell() {
        let dir = tempfile::tempdir().unwrap();
        let mut l = Lookup::default();
        assert_eq!(l.bead(dir.path(), "h"), "");
        Command::new("git")
            .args(["init", "-q"])
            .current_dir(dir.path())
            .status()
            .unwrap();
        assert_eq!(l.bead(dir.path(), "h"), "");
    }
}
