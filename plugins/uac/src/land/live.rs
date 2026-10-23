//! Integrations keep their existing owners: gate, bd, mux and Worktrunk.
use super::*;
use vega_herdr::{agent_name, herdr, lanes, local_host, me, state as st};

pub(super) struct Live;
fn pane_absent_in(inventory: &Value, pane: &str) -> Result<bool> {
    let panes = inventory["result"]["panes"]
        .as_array()
        .ok_or_else(|| Fault::new("pane inventory unavailable; retain pane/worktree"))?;
    Ok(!panes.iter().any(|p| p["pane_id"] == pane))
}
fn pane_absent(pane: &str) -> Result<bool> {
    let inventory = herdr(&["pane", "list"]).map_err(|e| error(e.message))?;
    pane_absent_in(&inventory, pane)
}
fn report_summary(done: Option<st::Done>) -> Result<String> {
    let done = done
        .filter(|d| d.outcome == st::Outcome::Succeeded)
        .ok_or_else(|| {
            Fault::new("sentinel report changed during inspection; recheck completion")
        })?;
    if done.summary.trim().is_empty() {
        return Err(Fault::new(
            "sentinel done report has no summary; add one and recheck completion",
        ));
    }
    Ok(done.summary)
}
fn error(e: impl std::fmt::Display) -> Fault {
    Fault::new(e.to_string())
}
pub(super) fn agents() -> Result<Vec<Value>> {
    herdr(&["agent", "list"]).map_err(|e| error(e.message))?["result"]["agents"]
        .as_array()
        .cloned()
        .ok_or_else(|| Fault::new("agent inventory unavailable"))
}
fn bead(dir: &Path, id: &str) -> Result<Value> {
    let text = command(dir, "bd", &["show", id, "--json"])?;
    let text = vega_plugin::unwrap_bd_envelope(&text);
    let v: Value = serde_json::from_str(&text).map_err(error)?;
    v.as_array()
        .and_then(|a| a.first())
        .filter(|b| b["id"] == id)
        .cloned()
        .ok_or_else(|| Fault::new(format!("bead {id} not found")))
}
fn has_marker(b: &Value, r: &Receipt) -> bool {
    ["close_reason", "notes"].iter().any(|field| {
        b[*field]
            .as_str()
            .is_some_and(|text| text.lines().any(|line| line == r.marker()))
    })
}
fn sentinel_closer(events: &Value, id: &str, actor: &str) -> bool {
    let Some(events) = events.as_array() else {
        return false;
    };
    if events.iter().any(|e| {
        e["issue_id"] == id
            && e["event_type"] == "closed"
            && e["created_at"].as_str().is_none_or(str::is_empty)
    }) {
        return false;
    }
    let latest = events
        .iter()
        .filter(|e| e["issue_id"] == id && e["event_type"] == "closed")
        .filter_map(|e| e["created_at"].as_str().map(|time| (time, e)))
        .max_by_key(|(time, _)| *time);
    latest.is_some_and(|(time, _)| {
        !actor.is_empty()
            && events
                .iter()
                .filter(|e| {
                    e["issue_id"] == id && e["event_type"] == "closed" && e["created_at"] == time
                })
                .all(|e| e["actor"] == actor)
    })
}
pub(super) fn accept_closed(
    dir: &Path,
    b: &Value,
    id: &str,
    sentinel: Option<&Sentinel>,
) -> Result<()> {
    let s = sentinel.ok_or_else(|| Fault::new("closed bead has no matching completed sentinel"))?;
    let text = command(dir, "bd", &["history", id, "--events", "--json"])?;
    let events: Value =
        serde_json::from_str(&vega_plugin::unwrap_bd_envelope(&text)).map_err(error)?;
    if b["status"] == "closed" && sentinel_closer(&events, id, &s.actor) {
        Ok(())
    } else {
        Err(Fault::new(
            "bead closer does not match completed sentinel; inspect before landing",
        ))
    }
}
fn owner_matches(b: &Value, sentinel: &Sentinel, actor: &str) -> bool {
    let owner = b["assignee"].as_str().unwrap_or("");
    owner.is_empty() || owner == sentinel.actor || owner == actor
}
fn reclaim_args<'a>(id: &'a str, owner: &'a str, status: &'a str, actor: &'a str) -> Vec<&'a str> {
    vec![
        "update",
        id,
        "--if-assignee",
        owner,
        "--if-status",
        status,
        "--assignee",
        actor,
    ]
}
fn ready(dir: &Path, b: &Value) -> Result<()> {
    for d in b["dependencies"].as_array().into_iter().flatten() {
        if matches!(
            d["dependency_type"].as_str(),
            Some("blocks" | "conditional-blocks" | "waits-for")
        ) {
            let id = d["id"]
                .as_str()
                .ok_or_else(|| Fault::new("dependency identity missing"))?;
            if bead(dir, id)?["status"] != "closed" {
                return Err(Fault::new(format!("open blocking dependency {id}")));
            }
        }
    }
    Ok(())
}
fn worktree(dir: &Path, branch: &str) -> Result<Option<PathBuf>> {
    parse_worktree(&git(dir, &["worktree", "list", "--porcelain"])?, branch)
}
fn parse_worktree(list: &str, branch: &str) -> Result<Option<PathBuf>> {
    let mut found = Vec::new();
    for block in list.split("\n\n") {
        if block
            .lines()
            .any(|l| l == format!("branch refs/heads/{branch}"))
        {
            if let Some(path) = block.lines().find_map(|l| l.strip_prefix("worktree ")) {
                found.push(PathBuf::from(path));
            }
        }
    }
    if found.len() > 1 {
        return Err(Fault::new(
            "source has multiple worktrees; cleanup identity ambiguous",
        ));
    }
    Ok(found.pop())
}
fn session(a: &Value) -> &str {
    a["agent_session"]["value"].as_str().unwrap_or("")
}
pub(super) fn identity_matches(s: &Sentinel, a: &Value) -> bool {
    a["pane_id"] == s.pane
        && agent_name(a) == s.name
        && session(a) == s.session
        && Path::new(a["cwd"].as_str().unwrap_or("")) == s.worktree
        && matches!(a["agent_status"].as_str(), Some("idle" | "done"))
}
fn verified(s: &Sentinel, a: &Value, parent: &str) -> bool {
    identity_matches(s, a)
        && st::parent_of_on(None, &s.pane, &s.name)
            .is_some_and(|p| p.pane == parent && p.host == local_host())
        && st::brief_of(None, &s.pane) == Some(s.brief)
        && st::done_of(None, &s.pane).is_some_and(|d| d.outcome == st::Outcome::Succeeded)
}
pub(super) fn validate_tree(dir: &Path, r: &Receipt, s: &Sentinel) -> Result<()> {
    if !s.worktree.exists() {
        return Err(Fault::new("sentinel worktree missing before pane closure"));
    }
    if worktree(dir, &r.branch)?.as_ref() != Some(&s.worktree)
        || sha(dir, &format!("refs/heads/{}", r.branch))? != r.head
        || !ancestor(dir, &r.head, r.integrated.as_ref().unwrap())?
        || s.worktree == dir
        || git(
            &s.worktree,
            &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        )? != git(
            dir,
            &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        )?
    {
        return Err(Fault::new(
            "sentinel worktree/branch identity changed; retain it",
        ));
    }
    clean(&s.worktree)?;
    no_operation(&s.worktree)
}
pub(super) fn checkout_authority(dir: &Path, target: &str) -> Result<()> {
    if git(dir, &["symbolic-ref", "--short", "HEAD"])? != target
        || worktree(dir, target)?
            .as_ref()
            .and_then(|p| p.canonicalize().ok())
            != Some(dir.canonicalize()?)
    {
        return Err(Fault::new(
            "sentinels cannot land; run from the UAC target checkout",
        ));
    }
    Ok(())
}
pub(super) fn occupants(
    panes: &[Value],
    root: &Path,
    skip: Option<&str>,
    workspace: Option<&str>,
) -> Vec<String> {
    let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    panes
        .iter()
        .filter(|p| skip.is_none_or(|skip| p["pane_id"] != skip))
        .filter(|p| {
            workspace.is_some_and(|ws| p["workspace_id"] == ws)
                || p["cwd"].as_str().is_some_and(|cwd| {
                    let path = Path::new(cwd)
                        .canonicalize()
                        .unwrap_or_else(|_| PathBuf::from(cwd));
                    path.starts_with(&root)
                })
        })
        .map(|p| p["pane_id"].as_str().unwrap_or("unknown pane").to_string())
        .collect()
}
fn unoccupied(s: &Sentinel, skip: Option<&str>, workspace: Option<&str>) -> Result<()> {
    let inventory = herdr(&["pane", "list"]).map_err(|e| error(e.message))?;
    let panes = inventory["result"]["panes"]
        .as_array()
        .ok_or_else(|| Fault::new("pane inventory unavailable; retain worktree"))?;
    let other = occupants(panes, &s.worktree, skip, workspace);
    if other.is_empty() {
        Ok(())
    } else {
        Err(Fault::new(format!(
            "cleanup scope is shared by {}; retain pane/worktree",
            other.join(", ")
        )))
    }
}
pub(super) fn install_result(r: &Receipt) -> &str {
    if r.policy.install.is_empty() {
        "not configured"
    } else if r.install == "attested" {
        "completion attested by UAC (exit status unmeasured)"
    } else {
        "exit 0"
    }
}
impl Services for Live {
    fn authority(&mut self, dir: &Path, target: &str) -> Result<()> {
        checkout_authority(dir, target)?;
        let key = lanes::project_key(dir).map_err(|e| error(e.message))?;
        let caller = me().map_err(|e| error(e.message))?;
        if !st::project_uac_of(&key).is_some_and(|(p, h)| p == caller.pane && h == st::post_host())
        {
            return Err(Fault::new("only this project's registered UAC may land"));
        }
        Ok(())
    }
    fn completed(&mut self, dir: &Path, r: &Receipt) -> Result<()> {
        let b = bead(dir, &r.bead)?;
        if b["status"] != "closed" || !has_marker(&b, r) {
            return Err(Fault::new(
                "completed landing bead marker changed; inspect without replaying closure",
            ));
        }
        if let Some(s) = &r.sentinel {
            if !r.keep && (s.worktree.exists() || worktree(dir, &r.branch)?.is_some()) {
                return Err(Fault::new("worktree exists after completed cleanup; retain recreated worktree and inspect"));
            }
            let inventory = agents()?;
            if let Some(a) = inventory.iter().find(|a| a["pane_id"] == s.pane) {
                let parent = me().map_err(|e| error(e.message))?.pane;
                if !r.keep || !verified(s, a, &parent) {
                    return Err(Fault::new(
                        "pane exists or changed after completed landing; retain and inspect",
                    ));
                }
            } else if !pane_absent(&s.pane)? {
                return Err(Fault::new(
                    "completed landing pane identity unknown; retain and inspect",
                ));
            }
            if r.keep && s.worktree.exists() {
                validate_tree(dir, r, s)?;
            }
        }
        Ok(())
    }
    fn validate(&mut self, dir: &Path, r: &Receipt) -> Result<()> {
        let b = bead(dir, &r.bead)?;
        ready(dir, &b)?;
        if b["status"] == "closed" {
            if has_marker(&b, r) {
                return Ok(());
            }
            accept_closed(dir, &b, &r.bead, r.sentinel.as_ref())?;
        }
        if sha(dir, &format!("refs/heads/{}", r.branch))? != r.head {
            return Err(Fault::new(
                "source branch moved; inspect before continuing landing",
            ));
        }
        let actor = lanes::actor(dir).ok_or_else(|| Fault::new("UAC actor unavailable"))?;
        let owner = b["assignee"].as_str().unwrap_or("");
        if let Some(s) = &r.sentinel {
            let parent = me().map_err(|e| error(e.message))?.pane;
            if !agents()?.iter().any(|a| verified(s, a, &parent))
                || (!owner.is_empty() && owner != actor && owner != s.actor)
            {
                return Err(Fault::new(
                    "sentinel completion or bead ownership changed; inspect before continuing",
                ));
            }
        } else if !owner.is_empty() && owner != actor {
            return Err(Fault::new(
                "bead ownership changed; inspect before continuing",
            ));
        }
        Ok(())
    }
    fn evidence(
        &mut self,
        dir: &Path,
        id: &str,
        branch: &str,
        _head: &str,
    ) -> Result<(String, Option<Sentinel>)> {
        let b = bead(dir, id)?;
        ready(dir, &b)?;
        let actor = lanes::actor(dir).ok_or_else(|| Fault::new("UAC actor unavailable"))?;
        let parent = me().map_err(|e| error(e.message))?.pane;
        let wt = worktree(dir, branch)?;
        let mut matches = Vec::new();
        for a in agents()? {
            if wt
                .as_ref()
                .is_none_or(|p| Path::new(a["cwd"].as_str().unwrap_or("")) != p)
            {
                continue;
            }
            let s = Sentinel {
                pane: a["pane_id"].as_str().unwrap_or("").into(),
                name: agent_name(&a).into(),
                session: session(&a).into(),
                actor: lanes::actor(wt.as_ref().unwrap()).unwrap_or_default(),
                worktree: wt.clone().unwrap(),
                brief: st::brief_of(None, a["pane_id"].as_str().unwrap_or("")).unwrap_or(0),
            };
            if !s.session.is_empty() && s.name != "-" && s.brief != 0 && verified(&s, &a, &parent) {
                let summary = report_summary(st::done_of(None, &s.pane))?;
                if !owner_matches(&b, &s, &actor) {
                    return Err(Fault::new("bead owner does not match completed sentinel"));
                }
                matches.push((summary, s));
            }
        }
        if matches.len() > 1 {
            return Err(Fault::new(
                "multiple sentinel identities match; inspect manually",
            ));
        }
        if let Some((summary, sentinel)) = matches.pop() {
            if b["status"] == "closed" {
                accept_closed(dir, &b, id, Some(&sentinel))?;
            }
            return Ok((summary, Some(sentinel)));
        }
        if b["status"] == "closed" {
            return Err(Fault::new("closed bead has no matching completed sentinel"));
        }
        if b["assignee"]
            .as_str()
            .is_some_and(|s| !s.is_empty() && s != actor)
        {
            return Err(Fault::new("bead is held by an owner without a matching completed sentinel; inspect before landing"));
        }
        let notes = b["notes"].as_str().unwrap_or("").trim();
        if notes.is_empty() {
            return Err(Fault::new(
                "no completed sentinel report or bead notes; inspect acceptance first",
            ));
        }
        Ok((notes.to_string(), None))
    }
    fn assisted_by(&mut self) -> Result<String> {
        vega_gate::author::from_pane()
            .map(|(h, m)| m.map_or(h.clone(), |m| format!("{h}:{m}")))
            .or_else(|| env::var_os("CODEX_THREAD_ID").map(|_| "codex".into()))
            .or_else(|| env::var_os("CLAUDE_CODE_SESSION_ID").map(|_| "claude".into()))
            .ok_or_else(|| Fault::new("calling harness identity unavailable"))
    }
    fn gate(&mut self, dir: &Path, target: &str, head: &str, start: bool) -> Result<String> {
        // A non-zero inspection exit blocks tool/config errors. Approval comes
        // from the exact-head segment plan below, never from check's exit 0.
        if sha(dir, "HEAD")? == head {
            command(dir, "vega", &["gate", "check", "--target", target])?;
        }
        let Some((blob, cfg)) = vega_gate::git::config_at(dir, target).map_err(error)? else {
            return Ok("not configured on target".into());
        };
        if !vega_gate::git::is_beads_repo(dir) {
            return Err(Fault::new(
                "configured gate needs beads; refusing fail-open landing",
            ));
        }
        let plan = vega_gate::segments::plan(dir, &cfg, &blob, target, head).map_err(error)?;
        let mut passes = Vec::new();
        let mut pending = false;
        for s in &plan.segments {
            let records = vega_gate::record::lookup(dir, &s.key).map_err(error)?;
            // The gate owns diff-key reuse across merge segments. The plan is
            // computed from this exact head, never from a moving branch name.
            match vega_gate::record::status(records) {
                vega_gate::record::Status::Pass(b) => passes.push(b.id),
                _ => pending = true,
            }
        }
        if pending {
            if start {
                let branch = git(dir, &["symbolic-ref", "--short", "HEAD"])?;
                let outcome = vega_gate::start::start(dir, &plan, &cfg, &branch).map_err(error)?;
                match outcome {
                    vega_gate::start::Outcome::Passed => {
                        return Err(Fault::new("gate coverage changed; resume to recheck it"))
                    }
                    vega_gate::start::Outcome::Running(m)
                    | vega_gate::start::Outcome::FailOpen(m)
                    | vega_gate::start::Outcome::Failed(m) => {
                        return Err(Fault::new(format!("gate pending: {m}")))
                    }
                }
            }
            return Err(Fault::new(format!("source head {head} lacks gate pass; run vega gate start in its worktree and wait for verdict")));
        }
        Ok(if passes.is_empty() {
            "pass (no relevant diff)".into()
        } else {
            format!("pass: {}", passes.join(", "))
        })
    }
    fn close(&mut self, dir: &Path, r: &Receipt) -> Result<()> {
        let b = bead(dir, &r.bead)?;
        let already_closed = b["status"] == "closed";
        if already_closed {
            if has_marker(&b, r) {
                return Ok(());
            }
            accept_closed(dir, &b, &r.bead, r.sentinel.as_ref())?;
        }
        ready(dir, &b)?;
        let owner = b["assignee"].as_str().unwrap_or("");
        let actor = lanes::actor(dir).ok_or_else(|| Fault::new("UAC bead actor unavailable"))?;
        if !already_closed && !owner.is_empty() && owner != actor {
            let s = r
                .sentinel
                .as_ref()
                .filter(|s| s.actor == owner)
                .ok_or_else(|| Fault::new("another owner holds bead; retain it"))?;
            let parent = me().map_err(|e| error(e.message))?.pane;
            if !agents()?.iter().any(|a| verified(s, a, &parent)) {
                return Err(Fault::new(
                    "sentinel no longer completed or identity changed; do not reclaim",
                ));
            }
            // Guarded reassignment is the bd contract for transferring this
            // observed claim; --force is mutually exclusive with these guards.
            command(
                dir,
                "bd",
                &reclaim_args(&r.bead, owner, b["status"].as_str().unwrap_or(""), &actor),
            )?;
        } else if !already_closed && owner.is_empty() {
            command(dir, "bd", &["update", &r.bead, "--claim"])?;
        }
        let files = git(
            dir,
            &["diff", "--stat", &r.before, r.integrated.as_ref().unwrap()],
        )?;
        let followups: std::collections::BTreeSet<_> = r
            .summary
            .split(|c: char| !c.is_ascii_alphanumeric() && !"-._".contains(c))
            .map(|s| s.trim_matches('.'))
            .filter(|s| {
                *s != r.bead
                    && s.split_once('-').is_some_and(|(prefix, id)| {
                        !prefix.is_empty()
                            && prefix.chars().all(|c| c.is_ascii_alphabetic())
                            && id.chars().any(|c| c.is_ascii_digit())
                    })
            })
            .collect();
        let followups = if followups.is_empty() {
            "none recorded in sentinel evidence".into()
        } else {
            followups.into_iter().collect::<Vec<_>>().join(", ")
        };
        let reason = format!("{}\n{}\nChanged files/approach:\n{}\nProject check: exit 0. Gate: {}. Push: verified remote SHA {}. Install: {}.\nCommits: source {}, integrated {}. Changelog renumbering: {:?}.\nFollow-ups: {followups}.\n{}", r.bead, r.summary, files, r.gate, r.integrated.as_ref().unwrap(), install_result(r), r.head, r.integrated.as_ref().unwrap(), r.renumbered, r.marker());
        if already_closed {
            command(
                dir,
                "bd",
                &[
                    "update",
                    &r.bead,
                    "--if-status",
                    "closed",
                    "--append-notes",
                    &reason,
                ],
            )?;
        } else {
            command(dir, "bd", &["close", &r.bead, "--reason", &reason])?;
        }
        let closed = bead(dir, &r.bead)?;
        if closed["status"] != "closed" || !has_marker(&closed, r) {
            return Err(Fault::new("bead closure not verified"));
        }
        Ok(())
    }
    fn pane(&mut self, dir: &Path, r: &Receipt) -> Result<()> {
        let s = r.sentinel.as_ref().unwrap();
        let inventory = agents()?;
        let Some(a) = inventory.iter().find(|a| a["pane_id"] == s.pane) else {
            // A closed agent must not be confused with a live shell in its pane.
            if !pane_absent(&s.pane)? {
                return Err(Fault::new(
                    "agent absent but pane still present or unknown; retain",
                ));
            }
            return Ok(());
        };
        validate_tree(dir, r, s)?;
        let parent = me().map_err(|e| error(e.message))?.pane;
        if !verified(s, a, &parent) {
            return Err(Fault::new(
                "sentinel pane identity/completion changed; retain",
            ));
        }
        unoccupied(
            s,
            Some(&s.pane),
            Some(
                a["workspace_id"]
                    .as_str()
                    .ok_or_else(|| Fault::new("sentinel workspace identity unavailable"))?,
            ),
        )?;
        command(dir, "vega", &["mux", "close", &s.pane])?;
        if agents()?.iter().any(|a| a["pane_id"] == s.pane) {
            return Err(Fault::new("pane closure not verified"));
        }
        Ok(())
    }
    fn remove(&mut self, dir: &Path, r: &Receipt) -> Result<()> {
        let s = r.sentinel.as_ref().unwrap();
        if !s.worktree.exists() {
            if worktree(dir, &r.branch)?.is_some() {
                return Err(Fault::new(
                    "worktree directory absent but Git registration remains; retain residue",
                ));
            }
            return Ok(());
        }
        validate_tree(dir, r, s)?;
        unoccupied(s, None, None)?;
        // No --yes/--force/--reap. Worktrunk owns hooks and filesystem lifecycle.
        command(
            dir,
            "wt",
            &["remove", "--foreground", "--no-delete-branch", &r.branch],
        )?;
        if s.worktree.exists() || worktree(dir, &r.branch)?.is_some() {
            return Err(Fault::new("worktree removal incomplete; retained residue"));
        }
        if sha(dir, &format!("refs/heads/{}", r.branch))? != r.head {
            return Err(Fault::new(
                "branch changed during cleanup; inspect Worktrunk result",
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn closed_bead_requires_latest_closer_and_unambiguous_audit_evidence() {
        let old = json!({"issue_id":"v-1","event_type":"closed","actor":"sentinel","created_at":"2026-10-20T00:00:00Z"});
        let new = json!({"issue_id":"v-1","event_type":"closed","actor":"other","created_at":"2026-10-21T00:00:00Z"});
        assert!(sentinel_closer(&json!([old]), "v-1", "sentinel"));
        assert!(!sentinel_closer(&json!([old, new]), "v-1", "sentinel"));
        assert!(!sentinel_closer(&json!([new, old]), "v-1", "sentinel"));
        assert!(!sentinel_closer(&json!([]), "v-1", "sentinel"));
        assert!(!sentinel_closer(&json!({}), "v-1", "sentinel"));
        assert!(!sentinel_closer(&json!([old]), "other-bead", "sentinel"));
        assert!(!sentinel_closer(&json!([old]), "v-1", ""));
        let mut tied = new.clone();
        tied["created_at"] = old["created_at"].clone();
        assert!(!sentinel_closer(&json!([old, tied]), "v-1", "sentinel"));
    }
    #[test]
    fn reclaim_argv_obeys_bd_update_flag_contract() {
        let args = reclaim_args("v-1", "sentinel", "in_progress", "uac");
        // bd update declares force mutually exclusive with each conditional
        // guard and claim; guarded reassignment transfers a live claim itself.
        assert_eq!(
            args,
            [
                "update",
                "v-1",
                "--if-assignee",
                "sentinel",
                "--if-status",
                "in_progress",
                "--assignee",
                "uac"
            ]
        );
        for exclusive in ["--force", "--claim"] {
            assert!(!args.contains(&exclusive));
        }
    }
    #[test]
    #[ignore = "requires installed bd; run explicitly to validate its parser without writes"]
    fn reclaim_argv_passes_installed_bd_parser() {
        let run = |force: bool| {
            let mut args = reclaim_args("v-absent", "sentinel", "in_progress", "uac");
            if force {
                args.push("--force");
            }
            args.push("--readonly");
            let out = Command::new("bd").args(args).output().unwrap();
            format!(
                "{}{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            )
        };
        let rejected = run(true);
        assert!(rejected.contains("if any flags in the group"), "{rejected}");
        let accepted = run(false);
        assert!(
            accepted.contains("not allowed in read-only mode"),
            "{accepted}"
        );
    }
    #[test]
    fn uac_owned_bead_already_matches_completed_sentinel() {
        let s = Sentinel {
            pane: "p".into(),
            name: "s".into(),
            session: "session".into(),
            worktree: "/worker".into(),
            actor: "sentinel".into(),
            brief: 1,
        };
        for owner in ["", "sentinel", "uac"] {
            assert!(owner_matches(&json!({"assignee":owner}), &s, "uac"));
        }
        assert!(!owner_matches(&json!({"assignee":"other"}), &s, "uac"));
    }
    #[test]
    fn worktree_porcelain_selects_branch_and_rejects_ambiguous_ownership() {
        let listing = "worktree /repo/main\nHEAD abc\nbranch refs/heads/main\n\nworktree /repo/detached\nHEAD def\ndetached\n\nworktree /repo/worker\nHEAD def\nbranch refs/heads/feature\n";
        assert_eq!(
            parse_worktree(listing, "feature").unwrap(),
            Some(PathBuf::from("/repo/worker"))
        );
        assert_eq!(parse_worktree(listing, "missing").unwrap(), None);
        let duplicate =
            format!("{listing}\nworktree /repo/reused\nHEAD def\nbranch refs/heads/feature\n");
        assert!(parse_worktree(&duplicate, "feature").is_err());
    }
    #[test]
    fn sentinel_identity_rejects_each_reused_or_busy_component() {
        let s = Sentinel {
            pane: "w1:p2".into(),
            name: "feature".into(),
            session: "session".into(),
            worktree: PathBuf::from("/repo/worker"),
            actor: "host/worker".into(),
            brief: 1,
        };
        let a = json!({"pane_id": s.pane, "name": s.name, "cwd": s.worktree, "agent_session": {"value": s.session}, "agent_status": "idle"});
        assert!(identity_matches(&s, &a));
        for (field, value) in [
            ("pane_id", "w1:p3"),
            ("name", "another"),
            ("cwd", "/elsewhere"),
            ("agent_status", "working"),
        ] {
            let mut reused = a.clone();
            reused[field] = value.into();
            assert!(!identity_matches(&s, &reused));
        }
        let mut reused = a;
        reused["agent_session"]["value"] = "new-session".into();
        assert!(!identity_matches(&s, &reused));
    }
    #[test]
    fn pane_absence_requires_a_complete_inventory() {
        assert!(pane_absent_in(&json!({"result":{"panes":[]}}), "w1:p2").unwrap());
        assert!(
            !pane_absent_in(&json!({"result":{"panes":[{"pane_id":"w1:p2"}]}}), "w1:p2").unwrap()
        );
        assert!(
            pane_absent_in(&json!({"result":{"panes":[{"pane_id":"w1:p3"}]}}), "w1:p2").unwrap()
        );
        assert!(pane_absent_in(&json!({"result":{}}), "w1:p2").is_err());
    }
    #[test]
    fn empty_done_summary_has_its_own_recovery_hint() {
        let empty = st::Done {
            outcome: st::Outcome::Succeeded,
            summary: " \n".into(),
        };
        let e = report_summary(Some(empty)).unwrap_err();
        assert!(e.message.contains("has no summary; add one"));
        assert!(report_summary(None)
            .unwrap_err()
            .message
            .contains("changed during inspection"));
        let failed = st::Done {
            outcome: st::Outcome::Failed,
            summary: "failed".into(),
        };
        assert!(report_summary(Some(failed))
            .unwrap_err()
            .message
            .contains("changed during inspection"));
        let done = st::Done {
            outcome: st::Outcome::Succeeded,
            summary: "implemented".into(),
        };
        assert_eq!(report_summary(Some(done)).unwrap(), "implemented");
    }
}
