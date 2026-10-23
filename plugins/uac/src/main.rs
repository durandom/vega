//! `vega uac`: coordinator mode (UAC, after Doom's Union Aerospace Corporation).
//!
//! `vega-uac --skill` is the role guide (docs/design/uac-role.md); `vega skills
//! uac` loads it, and its description keeps workers from loading it. `vega
//! uac` is the coordinator's home view: every agent that works on this
//! session's project, on every herdr machine, with its parent and bead.
//!
//! A project is a normalized Git fetch remote identity shared across hosts;
//! local common directories and checkouts remain location metadata. Worktrees
//! belong to it. Remote membership uses the registry's published project ID;
//! a remote path is never resolved on this host.

mod beads;
mod land;

use std::env;
use std::path::Path;
use std::process::ExitCode;
use std::thread;
use std::time::Duration;

use serde_json::Value;
use vega_herdr::lanes;
use vega_herdr::state::{self as st, parent_of_on, Attention};
use vega_herdr::{
    agent_name, cell, collapse_home, herdr, herdr_on, me, me_line, print_failure, status_counts,
    status_rank, str_of, Failure, Me,
};

const SKILL: &str = include_str!("../../../docs/design/uac-role.md");

/// `vega uac rules`: generic coordinator rules, loaded on demand from the guide.
const RULES: &str = include_str!("../../../docs/design/uac-rules.md");

/// herdr answers locally in milliseconds; over ssh a healthy machine needs
/// about a second. A machine slower than this is reported as unreachable.
const MACHINE_TIMEOUT: Duration = Duration::from_secs(8);

const HELP: &str = include_str!("../help/uac.md");

fn run(mode: Mode) -> ExitCode {
    match home(mode) {
        Ok(()) => ExitCode::SUCCESS,
        Err(f) => {
            print_failure(&f);
            ExitCode::from(1)
        }
    }
}

fn main() -> ExitCode {
    let mut args: Vec<String> = env::args().skip(1).collect();
    if args.first().is_some_and(|a| a == "land") {
        return land::main(&args[1..]);
    }
    if args.first().is_some_and(|a| a == "prune") {
        return land::prune::main(&args[1..]);
    }
    // `adopt --headless`: the flag may stand anywhere after `adopt`.
    let headless = args.first().is_some_and(|a| a == "adopt") && {
        let n = args.len();
        args.retain(|a| a != "--headless");
        args.len() < n
    };
    // `--help` always passes (AXI), also after a command: `adopt --help`.
    if args.iter().any(|a| a == "-h" || a == "--help") {
        print!("{HELP}");
        return ExitCode::SUCCESS;
    }
    match args.as_slice() {
        [] => run(Mode::View),
        [a] if a == "status" => run(Mode::Status),
        [a, rest @ ..] if a == "capacity" => done(capacity_alias(rest)),
        [a] if a == "claim" => run(Mode::Claim),
        [a] if a == "release" => run(Mode::Release),
        [a, t] if a == "adopt" && !t.starts_with('-') => {
            run(Mode::Adopt(t.clone(), None, headless))
        }
        [a, t, f, m] if a == "adopt" && f == "--machine" => {
            run(Mode::Adopt(t.clone(), Some(m.clone()), headless))
        }
        [a, id, t] if a == "answer" && !id.starts_with('-') => match answer_here(id, t) {
            Ok(()) => ExitCode::SUCCESS,
            Err(f) => {
                print_failure(&f);
                ExitCode::from(1)
            }
        },
        [a, t, f, b] if a == "park" && f == "--bead" && !t.starts_with('-') => {
            done(park(t, b, None))
        }
        [a, t, f, b, mf, m] if a == "park" && f == "--bead" && mf == "--machine" => {
            done(park(t, b, Some(m)))
        }
        [a, t] if a == "unpark" && !t.starts_with('-') => done(unpark(t, None)),
        [a, t, mf, m] if a == "unpark" && mf == "--machine" => done(unpark(t, Some(m))),
        [a, q] if a == "ask" && !q.starts_with('-') => run(Mode::Ask(q.clone(), None)),
        [a, w, d, q] if a == "ask" && w == "--within" => run(Mode::Ask(q.clone(), Some(d.clone()))),
        [a] if a == "--skill" => {
            print!("{SKILL}");
            ExitCode::SUCCESS
        }
        [a] if a == "rules" => {
            print!("{RULES}");
            ExitCode::SUCCESS
        }
        [a] if a == "-h" || a == "--help" => {
            print!("{HELP}");
            ExitCode::SUCCESS
        }
        [a] if a == "-v" || a == "-V" || a == "--version" => {
            println!("{}", vega_build::version_line!());
            ExitCode::SUCCESS
        }
        _ => {
            let message = format!(
                "unknown argument {}; vega uac takes `status`, `ask [--within <d>] \"<question>\"`, `claim`, `release`, `adopt <pane>`, `answer <bead> \"<answer>\"`, `capacity [...]`, `park <pane> --bead <id>`, `unpark <pane>`, `rules` or no argument",
                args[0]
            );
            print_failure(&Failure::new("usage", message, "valid usage below"));
            print!("{HELP}");
            ExitCode::from(2)
        }
    }
}

/// `vega uac capacity [...]` runs `vega-capacity [...]` in its place
/// (docs/design/capacity.md); returns only when that fails to start.
fn capacity_alias(args: &[String]) -> Result<(), Failure> {
    use std::os::unix::process::CommandExt;
    let e = std::process::Command::new("vega-capacity")
        .args(args)
        .exec();
    Err(Failure::new(
        "capacity_missing",
        format!("vega-capacity: {e}"),
        "mise run install-cli in the vega checkout installs it",
    ))
}

fn done(r: Result<(), Failure>) -> ExitCode {
    match r {
        Ok(()) => ExitCode::SUCCESS,
        Err(f) => {
            print_failure(&f);
            ExitCode::from(1)
        }
    }
}

/// The agents and workspaces one machine reported.
struct Snapshot {
    /// `None` for this machine, else the herdr machine label.
    machine: Option<String>,
    agents: Vec<Value>,
    workspaces: Vec<Value>,
    tabs: Vec<Value>,
}

fn snapshot(machine: Option<&str>) -> Result<Snapshot, Failure> {
    let list = |what: &str, key: &str| -> Result<Vec<Value>, Failure> {
        Ok(
            herdr_on(machine, &[what, "list"], MACHINE_TIMEOUT)?["result"][key]
                .as_array()
                .cloned()
                .unwrap_or_default(),
        )
    };
    Ok(Snapshot {
        machine: machine.map(str::to_string),
        agents: list("agent", "agents")?,
        workspaces: list("workspace", "workspaces")?,
        tabs: list("tab", "tabs")?,
    })
}

/// Labels of the enabled herdr machines; none if the list cannot be read.
fn machines() -> Vec<String> {
    herdr(&["machine", "list", "--json"])
        .ok()
        .and_then(|v| v.as_array().cloned())
        .unwrap_or_default()
        .iter()
        .filter(|m| m["enabled"] != false)
        .filter_map(|m| m["label"].as_str().map(str::to_string))
        .collect()
}

/// What `home` does after it collected the project's agents.
enum Mode {
    View,
    Status,
    /// `vega uac ask`: the question and `--within`.
    Ask(String, Option<String>),
    Claim,
    Release,
    /// `adopt <pane> [--machine <m>] [--headless]`.
    Adopt(String, Option<String>, bool),
}

fn home(mode: Mode) -> Result<(), Failure> {
    let status = matches!(mode, Mode::Status);
    let Me {
        pane: my_pane,
        workspace: my_ws,
        program,
        cwd: my_cwd,
        ..
    } = me()?;
    // Remote machines answer over ssh: ask them in parallel with the local one.
    let remote: Vec<_> = machines()
        .into_iter()
        .map(|m| {
            let handle = thread::spawn({
                let m = m.clone();
                move || snapshot(Some(&m))
            });
            (m, handle)
        })
        .collect();
    // The registry (other hosts' lineage, UACs, statuses), in parallel.
    let reg = thread::spawn(|| vega_registry::read_blocking(Duration::from_secs(4)));
    let local = snapshot(None)?;

    let bin = env::current_exe()
        .map(|p| collapse_home(&p.display().to_string()))
        .unwrap_or_else(|_| "unknown".into());
    println!("bin: {bin}");
    println!("description: UAC facility console: every agent on this project across your herdr machines, with parent and bead");
    let my_name = local
        .agents
        .iter()
        .find(|a| a["pane_id"] == my_pane.as_str())
        .map(agent_name)
        .unwrap_or("-");
    println!("me: {}", me_line(&my_pane, &program, my_name, &my_cwd));

    let project = project_key(&local.workspaces, &my_ws, &my_cwd)?;
    match &project {
        Some(p) => println!("project: {}", cell(p)),
        None => println!("project: none (not in a git repo); showing only workspace {my_ws}"),
    }
    let registry = reg
        .join()
        .unwrap_or_else(|_| Err("registry read crashed".into()));
    let view = registry.as_ref().ok();
    if let Some(p) = &project {
        match &mode {
            Mode::Claim => {
                return claim(
                    p,
                    &my_pane,
                    &local.agents,
                    view,
                    &project_path(&local.workspaces, &my_ws, &my_cwd),
                )
            }
            Mode::Release => return release(p, &my_pane),
            Mode::Adopt(target, machine, headless) => {
                return adopt(target, machine.as_deref(), &my_pane, *headless)
            }
            _ => {
                println!("uac: {}", uac_line(p, &my_pane, &local.agents, view));
                match &registry {
                    Ok(v) => println!(
                        "registry: {} hosts in sync",
                        machines_in(v).iter().filter(|h| v.fresh(h)).count()
                    ),
                    Err(e) => println!(
                        "registry: off ({}); parents and attention from this machine only",
                        cell(e)
                    ),
                }
            }
        }
    } else if matches!(mode, Mode::Claim | Mode::Release | Mode::Adopt(..)) {
        return Err(Failure::new(
            "no_project",
            "not in a git repo: a UAC belongs to a project",
            "run it from the project's checkout",
        ));
    }

    let mut snapshots = vec![local];
    let mut unreachable = Vec::new();
    for (m, handle) in remote {
        match handle.join() {
            Ok(Ok(s)) => snapshots.push(s),
            Ok(Err(f)) => unreachable.push(format!("{m} ({})", f.code)),
            Err(_) => unreachable.push(format!("{m} (crashed)")),
        }
    }

    let host = st::post_host();
    let mut lookup = beads::Lookup::default();
    let watching = vega_ci_watch::store::waiting(vega_ci_watch::unix_now());
    let mut rows = Vec::new();
    for s in &snapshots {
        let m = s.machine.as_deref();
        for a in &s.agents {
            let pane = str_of(a, "pane_id");
            if m.is_none() && pane == my_pane {
                continue;
            }
            let mine = match &project {
                Some(p) if m.is_none() => {
                    project_key(&s.workspaces, str_of(a, "workspace_id"), str_of(a, "cwd"))
                        .ok()
                        .flatten()
                        .as_ref()
                        == Some(p)
                }
                Some(p) => remote_in_project(view, m.unwrap_or(&host), pane, p),
                None => m.is_none() && str_of(a, "workspace_id") == my_ws,
            };
            if !mine {
                continue;
            }
            // CI watches are host-local.
            let ci = m.is_none().then(|| watching.get(pane)).flatten();
            let (attention, silence) = {
                // Local records first; a parent on another host has them in
                // its registry link.
                let link = view.and_then(|v| link_for(v, m.unwrap_or(&host), pane));
                let done = st::done_of(m, pane).map(|d| d.outcome).or_else(|| {
                    link.as_ref()
                        .and_then(|l| l["done"].as_str())
                        .and_then(st::Outcome::parse)
                });
                let silence = st::silence_of(m, pane).or_else(|| {
                    let l = link.as_ref().filter(|l| l["silent"] == true)?;
                    Some(st::Silence {
                        question: l["question"].as_str().map(String::from),
                        last: String::new(),
                        quota: st::Quota::from_json(&l["quota"]),
                    })
                });
                let attention = st::attention(
                    Some(true),
                    Some(str_of(a, "agent_status")),
                    done,
                    silence.as_ref(),
                    st::park_of(m, pane).as_ref(),
                );
                (st::waiting(attention, ci.is_some()), silence)
            };
            rows.push(Row {
                machine: m.unwrap_or(&host).to_string(),
                pane: pane.to_string(),
                name: agent_name(a).to_string(),
                tab: s
                    .tabs
                    .iter()
                    .find(|t| t["tab_id"] == str_of(a, "tab_id"))
                    .map(|t| str_of(t, "label").to_string())
                    .unwrap_or_default(),
                kind: str_of(a, "agent").to_string(),
                status: str_of(a, "agent_status").to_string(),
                dialog: if m.is_none() && matches!(str_of(a, "agent_status"), "blocked" | "unknown")
                {
                    vega_herdr::herdr_text(&["pane", "read", pane, "--source", "visible"])
                        .ok()
                        .and_then(|raw| {
                            vega_herdr::dialog::classify(
                                &raw,
                                str_of(a, "agent"),
                                str_of(a, "agent_status"),
                            )
                        })
                } else {
                    None
                },
                attention,
                silence,
                ci: ci.cloned().unwrap_or_default(),
                parent: parent_of_on(m, pane, agent_name(a))
                    .map(|p| p.label_on(&host, m))
                    .or_else(|| {
                        let l = link_for(view?, m.unwrap_or(&host), pane)?;
                        let (pp, ph) = (l["parent"].as_str()?, l["parent_host"].as_str()?);
                        Some(if ph.eq_ignore_ascii_case(&host) {
                            pp.to_string()
                        } else {
                            format!("{pp} on {ph}")
                        })
                    })
                    .unwrap_or_else(|| "-".into()),
                mode: parent_of_on(m, pane, agent_name(a))
                    .map(|_| st::mode_of(m, pane, agent_name(a)).as_str().to_string())
                    .unwrap_or_default(),
                // bd runs in the agent's cwd, so only local agents get a bead.
                bead: match m {
                    None => lookup.bead(Path::new(str_of(a, "cwd")), &host),
                    Some(_) => String::new(),
                },
                unread: a["tokens"]["mail_since"]
                    .as_str()
                    .and_then(|t| t.parse::<i64>().ok())
                    .map(|ms| unix_now() - ms / 1000),
                rank: status_rank(str_of(a, "agent_status")),
            });
        }
    }
    let project_panes: std::collections::HashSet<String> = rows
        .iter()
        .filter(|r| r.machine == host)
        .map(|r| r.pane.clone())
        .collect();
    // My sentinels whose pane is gone from a machine that answered: exited.
    // On an unreachable machine absence proves nothing.
    for c in st::children() {
        let Some(s) = snapshots.iter().find(|s| s.machine == c.machine) else {
            continue;
        };
        let here = s.agents.iter().any(|a| a["pane_id"] == c.pane.as_str());
        // One UAC oversees the project: gone children of any local project
        // agent count, not only mine.
        let ours = c.parent.pane == my_pane || project_panes.contains(&c.parent.pane);
        if here || !ours || c.parent.host != host {
            continue;
        }
        rows.push(Row {
            machine: c.machine.clone().unwrap_or_else(|| host.clone()),
            pane: c.pane.clone(),
            name: c.name.clone(),
            tab: String::new(),
            kind: "-".into(),
            status: "gone".into(),
            dialog: None,
            attention: Some(Attention::Exited),
            silence: None,
            ci: Vec::new(),
            parent: c.parent.label_on(&host, c.machine.as_deref()),
            mode: String::new(),
            bead: String::new(),
            unread: None,
            rank: 0,
        });
    }
    rows.sort_by(|a, b| (a.rank, &a.machine).cmp(&(b.rank, &b.machine)));

    if let Mode::Ask(question, within) = &mode {
        return ask(&rows, &host, question, within.as_deref());
    }

    if !unreachable.is_empty() {
        println!(
            "unreachable[{}]: {}",
            unreachable.len(),
            unreachable.join(", ")
        );
    }
    if rows.is_empty() {
        println!("agents: 0 other agents on this project on any reachable machine");
        if status {
            report(project.as_deref());
            return Ok(());
        }
        println!("help: vega mux spawn <name> --kind claude --headless --worktree <branch> --base <base> --prompt-file <brief>   # start a sentinel; <base>: the branch with the work the task needs");
        return Ok(());
    }
    let statuses: Vec<Value> = rows
        .iter()
        .map(|r| serde_json::json!({"agent_status": r.status}))
        .collect();
    println!(
        "status: {}",
        status_counts(&statuses.iter().collect::<Vec<_>>())
    );
    for line in agent_table(&rows) {
        println!("{line}");
    }
    for line in attention_table(&rows, &host) {
        println!("{line}");
    }
    if status {
        report(project.as_deref());
        return Ok(());
    }
    println!("help[4]:");
    println!("  vega mux show <pane|name> [--machine <m>]            # status and recent output");
    println!(
        "  vega mux send <pane|name> \"<text>\" [--machine <m>]   # reply arrives as a new prompt"
    );
    println!("  vega mux spawn <name> --kind claude --headless --worktree <branch> --base <base> --prompt-file <brief>   # <base>: the branch with the work the task needs");
    println!("  vega mux close <pane|name> [--machine <m>]           # closes a sentinel's worktree workspace; then wt remove <branch>");
    Ok(())
}

/// Remote paths are metadata only; facility membership requires a fresh,
/// schema-2 registry publication for the same project ID.
fn remote_in_project(
    view: Option<&vega_registry::View>,
    host: &str,
    pane: &str,
    project: &str,
) -> bool {
    let host = vega_registry::token(&host.to_lowercase());
    view.and_then(|v| v.agent(&host, pane))
        .filter(|a| a["schema"] == 2)
        .and_then(|a| a["project"].as_str())
        == Some(project)
}

/// Resolve this session from its local workspace/common directory or cwd.
/// Never call this with a remote host's workspace metadata.
fn project_key(workspaces: &[Value], my_ws: &str, my_cwd: &str) -> Result<Option<String>, Failure> {
    let path = project_path(workspaces, my_ws, my_cwd);
    vega_herdr::project::resolve_optional(&path, &st::post_host())
        .map(|p| p.map(|p| p.id.to_string()))
}

fn project_path(workspaces: &[Value], my_ws: &str, my_cwd: &str) -> std::path::PathBuf {
    let path = workspaces
        .iter()
        .find(|w| w["workspace_id"] == my_ws)
        .and_then(|w| {
            w["worktree"]["checkout_path"]
                .as_str()
                .filter(|p| !p.is_empty())
                .or_else(|| w["worktree"]["repo_key"].as_str().filter(|p| !p.is_empty()))
        })
        .unwrap_or(my_cwd);
    expand_home(path)
}

fn claim_project(key: &str, path: &Path) -> Result<vega_herdr::project::Project, Failure> {
    let project = vega_herdr::project::resolve(path, &st::post_host())?;
    if project.id.as_str() != key {
        return Err(Failure::new(
            "project_identity_changed",
            "project identity changed before claim",
            "retry from the workspace after the explicit rekey migration",
        ));
    }
    Ok(project)
}

/// `~/x` as a filesystem path: `Me.cwd` is collapsed for display, and a
/// process cannot start in a literal `~` dir (a workspace made before
/// `git init` has no repo_key, so this fallback finds the project).
fn expand_home(path: &str) -> std::path::PathBuf {
    match (path.strip_prefix("~/"), env::var("HOME")) {
        (Some(rest), Ok(home)) => Path::new(&home).join(rest),
        _ if path == "~" => env::var("HOME").map(Into::into).unwrap_or_default(),
        _ => path.into(),
    }
}

struct Row {
    machine: String,
    pane: String,
    name: String,
    /// The herdr tab label the Slayer sees.
    tab: String,
    kind: String,
    status: String,
    attention: Option<Attention>,
    /// The silent record: what its finished turn ended with.
    silence: Option<st::Silence>,
    /// The targets its `vega ci watch` waits on.
    ci: Vec<String>,
    dialog: Option<vega_herdr::dialog::Dialog>,
    parent: String,
    /// headed or headless (`vega mux mode`); empty without a local parent record.
    mode: String,
    bead: String,
    /// Age of the oldest unread mail (vegad's `mail_since` token), seconds.
    unread: Option<i64>,
    rank: usize,
}

fn agent_table(rows: &[Row]) -> Vec<String> {
    let mut out = vec![format!(
        "agents[{}]{{machine,pane,name,tab,kind,status,attention,unread_mail,parent,mode,bead}}:",
        rows.len()
    )];
    for r in rows {
        out.push(format!(
            "  {},{},{},{},{},{},{},{},{},{},{}",
            cell(&r.machine),
            r.pane,
            cell(&r.name),
            cell(&r.tab),
            cell(&r.kind),
            r.status,
            r.attention.map_or("", Attention::as_str),
            r.unread
                .map(|s| format!("{} old", age(s)))
                .unwrap_or_default(),
            cell(&r.parent),
            r.mode,
            cell(&r.bead)
        ));
    }
    out
}

/// The project's registered UAC on this machine: pane and host.
fn uac_record(key: &str) -> Option<(String, String)> {
    st::project_uac_of(key)
}

/// When the UAC claimed the project (status counts from here without beads).
fn uac_record_at(key: &str) -> Option<i64> {
    read_project_timestamp(&st::project_file("uac", key)?, key)
}

/// `45s`, `25m`, `2h05m`, `3d`.
fn age(secs: i64) -> String {
    let secs = secs.max(0);
    match secs {
        0..60 => format!("{secs}s"),
        60..3600 => format!("{}m", secs / 60),
        3600..172800 => format!("{}h{:02}m", secs / 3600, secs % 3600 / 60),
        _ => format!("{}d", secs / 86400),
    }
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// The project's one UAC tracker epic (label uac-tracker): found, or created
/// when there is none. Several open trackers refuse: the Slayer keeps one.
/// `None` in a repo without beads.
fn tracker(key: &str, cwd: &Path) -> Result<Option<String>, Failure> {
    if !beads::has_beads(cwd) {
        return Ok(None);
    }
    let found = beads::trackers(cwd).ok_or_else(|| {
        Failure::new(
            "bd_failed",
            "bd list failed",
            "check `bd list --label uac-tracker`",
        )
    })?;
    match found.as_slice() {
        [t] => Ok(Some(t.id.clone())),
        [] => beads::create_tracker(cwd, key)
            .map(Some)
            .ok_or_else(|| Failure::new("bd_failed", "bd create failed", "run bd doctor")),
        many => Err(Failure::new(
            "several_trackers",
            format!(
                "{} open UAC trackers: {}",
                many.len(),
                many.iter()
                    .map(|t| t.id.as_str())
                    .collect::<Vec<_>>()
                    .join(" ")
            ),
            "keep one: close the others (bd close <id> --reason ...), then claim",
        )),
    }
}

fn alive(agents: &[Value], pane: &str) -> bool {
    agents.iter().any(|a| a["pane_id"] == pane)
}

/// `w30:p1 on proteus (you)`, `none registered; vega uac claim`, ...
fn uac_line(key: &str, me: &str, agents: &[Value], view: Option<&vega_registry::View>) -> String {
    let here = st::post_host();
    match uac_record(key) {
        None => match remote_uac(key, view) {
            Some((p, h)) => format!("{p} on {h}"),
            None => "none registered (the UAC session runs `vega uac claim`)".into(),
        },
        Some((p, h)) if p == me && h == here => format!("{p} on {h} (you)"),
        Some((p, h)) if h == here && !alive(agents, &p) => {
            format!("{p} on {h}, gone (`vega uac claim` takes over)")
        }
        Some((p, h)) => format!("{p} on {h}"),
    }
}

/// A live UAC of `key` on another machine, from the registry: (pane, host).
fn remote_uac(key: &str, view: Option<&vega_registry::View>) -> Option<(String, String)> {
    let v = view?;
    let here = vega_registry::token(&st::post_host().to_lowercase());
    v.uacs(key).into_iter().find_map(|u| {
        let (pane, host) = (u["pane"].as_str()?, u["host"].as_str()?);
        let token = vega_registry::token(&host.to_lowercase());
        (token != here && matches!(v.status(&token, pane), Some(Some(_))))
            .then(|| (pane.to_string(), host.to_string()))
    })
}

/// Hosts with a beat in the registry.
fn machines_in(view: &vega_registry::View) -> Vec<String> {
    view.entries
        .keys()
        .filter_map(|k| k.strip_suffix(".beat").map(String::from))
        .collect()
}

/// The registry link (lineage record kept by the parent's host) of the
/// agent `pane` on `host`.
fn link_for(view: &vega_registry::View, host: &str, pane: &str) -> Option<Value> {
    let token = vega_registry::token(&host.to_lowercase());
    view.links().into_iter().find(|l| {
        l["pane"] == pane
            && l["child_host"].as_str().map(vega_registry::token) == Some(token.clone())
    })
}

/// `vega uac claim`: this session becomes the project's UAC. A live other
/// UAC on this machine refuses: one UAC per project.
fn claim(
    key: &str,
    me: &str,
    agents: &[Value],
    view: Option<&vega_registry::View>,
    path: &Path,
) -> Result<(), Failure> {
    let project = claim_project(key, path)?;
    let here = st::post_host();
    let mut took_over = None;
    // A live UAC of this project on another machine (registry) refuses too.
    if let Some((p, h)) = remote_uac(key, view) {
        return Err(Failure::new(
            "uac_exists",
            format!("{p} on {h} is the UAC of {}", key),
            &format!("one UAC per project: hand over (`vega uac release` in {p} on {h}) or close it; then claim"),
        ));
    }
    match uac_record(key) {
        Some((p, h)) if p == me && h == here => {
            retain_claim_location(key, &project.location)?;
            println!("uac: {p} on {h} (you, already registered)");
            if let Some(t) = tracker(key, &project.location.checkout)? {
                println!("tracker: {t}");
            }
            boot_decisions(&project.location.checkout);
            return Ok(());
        }
        Some((p, h)) if h != here || alive(agents, &p) => {
            return Err(Failure::new(
                "uac_exists",
                format!("{p} on {h} is the UAC of {}", key),
                &format!(
                    "one UAC per project: hand over (`vega uac release` in {p}) or close it; then claim"
                ),
            ));
        }
        Some((p, _)) => took_over = Some(p),
        None => {}
    }
    // Before registering: several trackers refuse the claim.
    let tracker = tracker(key, &project.location.checkout)?;
    let file = st::project_file("uac", key)
        .ok_or_else(|| Failure::new("no_home", "no state dir", "set HOME or XDG_STATE_HOME"))?;
    let _ = file.parent().map(std::fs::create_dir_all);
    let common = project.location.common_dir.to_string_lossy();
    let origin = vega_registry::project_origin_name(&common);
    let prior = match std::fs::read_to_string(&file) {
        Ok(text) => serde_json::from_str::<Value>(&text).map_err(|e| {
            Failure::new(
                "state_io",
                e.to_string(),
                "repair the UAC state before claiming",
            )
        })?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => serde_json::json!({}),
        Err(e) => {
            return Err(Failure::new(
                "state_io",
                e.to_string(),
                "check the state dir",
            ))
        }
    };
    let record = claim_record(
        prior,
        serde_json::json!({"schema": 2, "pane": me, "host": here, "at": unix_now(), "project": key,
        "common_dir": common, "checkout": project.location.checkout, "folder_name": vega_registry::project_folder(&common), "origin_name": origin}),
        tracker.as_deref(),
    );
    std::fs::write(&file, record.to_string())
        .map_err(|e| Failure::new("state_io", e.to_string(), "check the state dir"))?;
    println!("uac: {me} on {here} (you)");
    match tracker {
        Some(t) => println!("tracker: {t}"),
        None => println!("tracker: none (no beads in this repo; status counts from this claim)"),
    }
    if let Some(p) = took_over {
        println!("took_over: {p} (gone)");
    }
    boot_decisions(&project.location.checkout);
    println!("help: vega uac   # the facility view");
    Ok(())
}

fn claim_locations(
    record: &Value,
) -> std::collections::BTreeSet<vega_herdr::project::RepoLocation> {
    let mut locations = std::collections::BTreeSet::new();
    if let Some(list) = record["locations"].as_array() {
        for location in list {
            if let Ok(location) = serde_json::from_value(location.clone()) {
                locations.insert(location);
            }
        }
    }
    if let (Some(common), Some(checkout)) =
        (record["common_dir"].as_str(), record["checkout"].as_str())
    {
        locations.insert(vega_herdr::project::RepoLocation {
            common_dir: common.into(),
            checkout: checkout.into(),
        });
    }
    locations
}

/// An idempotent claim from another local checkout retains the original
/// primary execution location and timestamp while adding discovery metadata.
fn retain_claim_location(
    key: &str,
    location: &vega_herdr::project::RepoLocation,
) -> Result<(), Failure> {
    let error = || {
        Failure::new(
            "state_io",
            "Could not retain local claim location",
            "check the vega state directory",
        )
    };
    let file = st::project_file("uac", key).ok_or_else(error)?;
    let text = std::fs::read_to_string(&file).map_err(|_| error())?;
    let mut record: Value = serde_json::from_str(&text).map_err(|_| error())?;
    if record["schema"] != 2 || record["project"] != key {
        return Err(error());
    }
    let before = record.clone();
    let mut locations = claim_locations(&record);
    locations.insert(location.clone());
    record["locations"] = serde_json::json!(locations);
    let mut aliases = std::collections::BTreeSet::new();
    for field in ["folder_name", "origin_name"] {
        if let Some(alias) = record[field].as_str() {
            aliases.insert(alias.to_owned());
        }
    }
    if let Some(list) = record["aliases"].as_array() {
        aliases.extend(list.iter().filter_map(|v| v.as_str().map(str::to_owned)));
    }
    aliases
        .insert(vega_registry::project_folder(&location.common_dir.to_string_lossy()).to_owned());
    if let Some(origin) = vega_registry::project_origin_name(&location.common_dir.to_string_lossy())
    {
        aliases.insert(origin);
    }
    record["aliases"] = serde_json::json!(aliases);
    if record != before {
        let tmp = file.with_extension(format!("tmp{}", std::process::id()));
        std::fs::write(&tmp, record.to_string()).map_err(|_| error())?;
        std::fs::rename(tmp, file).map_err(|_| error())?;
    }
    Ok(())
}

/// A takeover changes the live claim while retaining migrated project metadata.
fn claim_record(mut prior: Value, next: Value, tracker: Option<&str>) -> Value {
    let mut aliases = std::collections::BTreeSet::new();
    for value in [&prior, &next] {
        for field in ["folder_name", "origin_name"] {
            if let Some(alias) = value[field].as_str() {
                aliases.insert(alias.to_owned());
            }
        }
        if let Some(list) = value["aliases"].as_array() {
            aliases.extend(list.iter().filter_map(|v| v.as_str().map(str::to_owned)));
        }
    }
    let mut locations = claim_locations(&prior);
    locations.extend(claim_locations(&next));
    if !prior.is_object() {
        prior = serde_json::json!({});
    }
    if let Some(fields) = next.as_object() {
        for (key, value) in fields {
            prior[key] = value.clone();
        }
    }
    prior["locations"] = serde_json::json!(locations);
    prior["aliases"] = serde_json::json!(aliases);
    if let Some(tracker) = tracker {
        prior["tracker"] = tracker.into();
    }
    prior
}

/// `vega uac release`: hand the project back; only the UAC itself.
fn release(key: &str, me: &str) -> Result<(), Failure> {
    match uac_record(key) {
        Some((p, h)) if p == me && h == st::post_host() => {
            if let Some(f) = st::project_file("uac", key) {
                let _ = std::fs::remove_file(f);
            }
            // vegad set the VEGA name; names have no TTL in herdr.
            let _ = vega_herdr::herdr_text(&[
                "pane",
                "report-metadata",
                me,
                "--source",
                "vega:vegad",
                "--clear-display-agent",
                "--clear-token",
                "role",
            ]);
            println!("released: {me} is no longer the UAC of {}", key);
            Ok(())
        }
        Some((p, h)) => Err(Failure::new(
            "not_uac",
            format!("{p} on {h} is the UAC, not you"),
            "only the UAC releases itself",
        )),
        None => {
            println!("released: no UAC registered (no-op)");
            Ok(())
        }
    }
}

/// `vega uac adopt <pane|name>`: a session the UAC did not spawn gets the UAC
/// as its parent, so vegad supervises it and its report goes to the UAC. The
/// session is told so. This machine only (until the registry). Headed unless
/// `headless`: an adopted session is mostly one the user works in, so its
/// in-pane questions stay (`vega mux mode`).
fn adopt(target: &str, machine: Option<&str>, me: &str, headless: bool) -> Result<(), Failure> {
    let a =
        herdr_on(machine, &["agent", "get", target], MACHINE_TIMEOUT)?["result"]["agent"].clone();
    let pane = str_of(&a, "pane_id").to_string();
    if machine.is_none() && pane == me {
        return Err(Failure::new(
            "usage",
            "that is your own pane",
            "adopt another session; see `vega uac`",
        ));
    }
    if machine.is_none() {
        vega_herdr::project::resolve_optional(Path::new(str_of(&a, "cwd")), &st::post_host())?;
    }
    st::save_parent_on(machine, &pane, agent_name(&a), me);
    let mode = if headless {
        st::Mode::Headless
    } else {
        st::Mode::Headed
    };
    st::save_mode(machine, &pane, agent_name(&a), mode);
    // Its worktree becomes a lane: status labels the beads it creates from now.
    let lane = match machine {
        None => {
            let here = env::current_dir().unwrap_or_default();
            lanes::record_if_uac(Path::new(str_of(&a, "cwd")), &here, me)?
        }
        Some(_) => None,
    };
    // Across machines the report travels through vega post to this host.
    let host_flag = machine
        .map(|_| format!(" --host {}", st::post_host()))
        .unwrap_or_default();
    let questions = match mode {
        st::Mode::Headless => format!("Questions and decisions go to VEGA, never to the user in your pane: vega post send{host_flag} {me} \"<question>\", then wait for the answer."),
        st::Mode::Headed => format!("The user works in your pane: ask the user as before; questions for VEGA: vega post send{host_flag} {me} \"<question>\"."),
    };
    let text = format!(
        "VEGA, the project's coordinator (pane {me}), now coordinates your work. When your current task is finished, report once: vega post done --to {me}{host_flag} --outcome succeeded|failed \"<one-line result>\". {questions} Keep working on your task."
    );
    let mut args: Vec<String> = vec!["mux".into()];
    if let Some(m) = machine {
        args.push("--machine".into());
        args.push(m.into());
    }
    args.extend(["send".into(), pane.clone(), text]);
    let sent = std::process::Command::new("vega")
        .args(&args)
        .output()
        .map_err(|e| Failure::new("vega_not_found", e.to_string(), "install vega"))?;
    println!("adopted: {pane} (parent {me}, {})", mode.as_str());
    if let Some(actor) = &lane {
        println!(
            "lane: {actor} (status labels its new beads {})",
            lanes::LABEL
        );
    }
    if !sent.status.success() {
        println!(
            "told: no ({}); tell it yourself",
            String::from_utf8_lossy(&sent.stdout)
                .lines()
                .nth(1)
                .unwrap_or("")
                .trim()
        );
    } else {
        println!("told: yes");
    }
    Ok(())
}

/// `vega uac park <pane> --bead <id>`: the sentinel waits on a decision.
/// vegad leaves it out of the wakes until the bead closes (it checks with
/// `bd show` in this checkout), then wakes the parent once with `decided`.
fn park(target: &str, bead: &str, machine: Option<&str>) -> Result<(), Failure> {
    let a =
        herdr_on(machine, &["agent", "get", target], MACHINE_TIMEOUT)?["result"]["agent"].clone();
    let pane = str_of(&a, "pane_id").to_string();
    if parent_of_on(machine, &pane, agent_name(&a)).is_none() {
        return Err(Failure::new(
            "not_supervised",
            format!("{pane} has no parent on this machine: vegad does not wake anyone for it"),
            &format!("adopt it first: vega uac adopt {target}"),
        ));
    }
    let cwd = env::current_dir().unwrap_or_default();
    let (title, open) = beads::show(&cwd, bead).ok_or_else(|| {
        Failure::new(
            "unknown_bead",
            format!("no bead {bead} in this repo"),
            "run it from the checkout whose beads hold the decision",
        )
    })?;
    if !open {
        return Err(Failure::new(
            "bead_closed",
            format!("{bead} is closed already"),
            &format!("resume {pane} instead: vega mux send {pane} \"<next step>\""),
        ));
    }
    let p = st::Park {
        bead: bead.to_string(),
        title,
        dir: cwd.display().to_string(),
        bd: beads::bd_path(),
        closed: false,
    };
    st::save_park(machine, &pane, &p);
    println!("parked: {pane} on {bead} ({})", cell(&p.title));
    println!(
        "help: vegad wakes you once when {bead} closes; new work you send {pane} unparks it; vega uac unpark {pane} ends it now"
    );
    Ok(())
}

/// `vega uac unpark <pane>`: vegad supervises the sentinel as before.
fn unpark(target: &str, machine: Option<&str>) -> Result<(), Failure> {
    let a =
        herdr_on(machine, &["agent", "get", target], MACHINE_TIMEOUT)?["result"]["agent"].clone();
    let pane = str_of(&a, "pane_id").to_string();
    if st::unpark(machine, &pane) {
        println!("unparked: {pane}");
    } else {
        println!("unparked: {pane} was not parked");
    }
    Ok(())
}

/// `vega uac answer` from anywhere in the project: the board's popup is no
/// herdr pane, so neither the caller's pane nor its workspace is needed.
fn answer_here(id: &str, text: &str) -> Result<(), Failure> {
    let cwd = env::current_dir().unwrap_or_default();
    let key = vega_herdr::project::resolve(&cwd, &st::post_host())?
        .id
        .to_string();
    let me = env::var("HERDR_PANE_ID").unwrap_or_default();
    let reg = thread::spawn(|| vega_registry::read_blocking(Duration::from_secs(4)));
    let agents = herdr_on(None, &["agent", "list"], MACHINE_TIMEOUT)
        .map(|v| {
            v["result"]["agents"]
                .as_array()
                .cloned()
                .unwrap_or_default()
        })
        .unwrap_or_default();
    let registry = reg
        .join()
        .unwrap_or_else(|_| Err("registry read crashed".into()));
    answer(&key, id, text, &me, &agents, registry.as_ref().ok())
}

/// `vega uac answer <bead> "<text>"`: the Slayer's answer to a decision. It
/// lands as a bead comment (author Slayer), and the project's UAC gets mail:
/// it acts and closes the bead. Comments carry any back-and-forth.
fn answer(
    key: &str,
    id: &str,
    text: &str,
    me: &str,
    agents: &[Value],
    view: Option<&vega_registry::View>,
) -> Result<(), Failure> {
    if text.trim().is_empty() {
        return Err(Failure::new(
            "usage",
            "the answer is empty",
            "vega uac answer <bead> \"<answer>\"",
        ));
    }
    let cwd = env::current_dir().unwrap_or_default();
    let (title, open) = beads::show(&cwd, id).ok_or_else(|| {
        Failure::new(
            "unknown_bead",
            format!("no bead {id} in this repo"),
            "see `vega uac status` for the open decisions",
        )
    })?;
    if !beads::comment(&cwd, id, "Slayer", text) {
        return Err(Failure::new(
            "bd_failed",
            format!("bd did not add the comment to {id}"),
            &format!("check `bd comments {id}`"),
        ));
    }
    println!(
        "answered: {id} (comment by Slayer){}",
        if open { "" } else { "; the bead is closed" }
    );
    let here = st::post_host();
    let uac = match uac_record(key) {
        Some((p, h)) if h != here || alive(agents, &p) => Some((p, h)),
        _ => remote_uac(key, view),
    };
    let Some((pane, host)) = uac else {
        println!("uac: none alive; the answer waits in the bead");
        return Ok(());
    };
    if pane == me && host == here {
        println!("uac: you (no mail)");
        return Ok(());
    }
    let mail = format!(
        "The Slayer answered {id} ({title}): {text}\nAct on it, then close {id} with the answer as reason. To ask back: bd comments add {id} \"<question>\" and say so in one line; not by mail."
    );
    let mut args = vec!["post", "send"];
    if host != here {
        args.extend(["--host", host.as_str()]);
    }
    args.extend([pane.as_str(), mail.as_str()]);
    let sent = std::process::Command::new("vega")
        .args(&args)
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| Failure::new("vega_not_found", e.to_string(), "install vega"))?;
    if sent.status.success() {
        println!("uac: {pane} on {host}, told by mail");
    } else {
        println!(
            "uac: {pane} on {host}, not told (vega post send failed); the answer waits in the bead"
        );
    }
    Ok(())
}

/// `vega uac ask`: one question to every other agent on the project, through
/// `vega post ask`, which gathers the answers into one pointer.
fn ask(rows: &[Row], host: &str, question: &str, within: Option<&str>) -> Result<(), Failure> {
    let targets = ask_targets(rows, host);
    if targets.is_empty() {
        println!("ask: 0 other agents on this project; nothing sent");
        return Ok(());
    }
    run_post_ask(&targets, question, within)
}

/// `<pane>` here, `<pane>@<host>` elsewhere; gone sentinels are skipped.
fn ask_targets(rows: &[Row], host: &str) -> Vec<String> {
    rows.iter()
        .filter(|r| r.status != "gone")
        .map(|r| {
            if r.machine == host {
                r.pane.clone()
            } else {
                format!("{}@{}", r.pane, r.machine.to_lowercase())
            }
        })
        .collect()
}

fn run_post_ask(targets: &[String], question: &str, within: Option<&str>) -> Result<(), Failure> {
    let mut args: Vec<String> = vec!["post".into(), "ask".into()];
    for t in targets {
        args.push("--to".into());
        args.push(t.clone());
    }
    if let Some(w) = within {
        args.push("--within".into());
        args.push(w.into());
    }
    args.push(question.into());
    let status = std::process::Command::new("vega")
        .args(&args)
        .status()
        .map_err(|e| Failure::new("vega_not_found", e.to_string(), "install vega"))?;
    if status.success() {
        Ok(())
    } else {
        std::process::exit(status.code().unwrap_or(1));
    }
}

/// Landed lists longer than this are cut, with the total.
const LANDED_MAX: usize = 20;

/// `vega uac status`, after running agents and attention: what landed since
/// the last status, then the open decisions for the Slayer (last, so they
/// end the report). Records this status time for the project.
/// The gh rate limit is younger than this, or refreshed by a free 304.
const GH_RATE_MAX_AGE: i64 = 120;

/// The GitHub core limit every agent and CI job of the user shares, and the
/// gh calls per pane on this host (the gh shim counts them;
/// docs/design/ci-watch.md).
fn gh_report() {
    let now = unix_now();
    match vega_ci_watch::store::rate(GH_RATE_MAX_AGE, now) {
        Ok(r) => println!(
            "gh: core {}/{} left, reset {}",
            r.remaining,
            r.limit,
            vega_herdr::local_time(Some(r.reset)).unwrap_or_else(|| r.reset.to_string())
        ),
        Err(e) => println!("gh: core limit unknown ({})", cell(&e)),
    }
    for line in shim_calls("gh", now) {
        println!("{line}");
    }
}

/// The tea calls per pane on this host (the tea shim counts them), only when
/// there are any: most hosts never use Forgejo. No limit line: Forgejo sends
/// no rate headers.
fn tea_report() {
    for line in tea_lines(shim_calls("tea", unix_now())) {
        println!("{line}");
    }
}

/// Nothing for a zero table: one line, no rows.
fn tea_lines(table: Vec<String>) -> Vec<String> {
    if table.len() == 1 {
        Vec::new()
    } else {
        table
    }
}

/// The last hour of `tool` calls the shim counted, split into panes and
/// callers without a herdr pane.
fn shim_calls(tool: &str, now: i64) -> Vec<String> {
    let (panes, sessions): (Vec<_>, Vec<_>) =
        vega_shim::calls::per_pane(tool, vega_shim::calls::HOUR, now)
            .into_iter()
            .partition(|(k, _)| vega_shim::is_pane(k));
    calls_table(tool, &panes, &sessions)
}

/// The `<tool>_calls` table: pane ids only; callers without a herdr pane (a
/// session id) in their own table, labelled by harness.
fn calls_table(tool: &str, panes: &[(String, usize)], sessions: &[(String, usize)]) -> Vec<String> {
    let mut out = Vec::new();
    if panes.is_empty() {
        out.push(format!(
            "{tool}_calls: 0 by agent panes on this host in the last hour"
        ));
    } else {
        out.push(format!("{tool}_calls[{}]{{pane,last_hour}}:", panes.len()));
        out.extend(panes.iter().map(|(p, n)| format!("  {},{n}", cell(p))));
    }
    if !sessions.is_empty() {
        out.push(format!(
            "{tool}_calls_without_pane[{}]{{session,last_hour}}:",
            sessions.len()
        ));
        out.extend(sessions.iter().map(|(k, n)| {
            let (harness, id) = k.split_once(':').unwrap_or(("session", k));
            let id: String = id.chars().take(8).collect();
            format!("  {harness} {},{n}", cell(&id))
        }));
    }
    out
}

/// The beads sync state of every project vegad syncs on this host
/// (docs/design/beads-sync.md); one line when there are none.
fn beads_report() {
    let now = vega_bd_sync::unix_now();
    let projects = vega_bd_sync::store::projects();
    if projects.is_empty() {
        println!("beads_sync: 0 projects provisioned on this host (vega bd init)");
        return;
    }
    println!(
        "beads_sync[{}]{{{}}}:",
        projects.len(),
        vega_bd_sync::store::COLUMNS
    );
    let mut problems = Vec::new();
    for p in &projects {
        let (cells, problem) =
            vega_bd_sync::store::row(p, &vega_bd_sync::store::state(&p.key, &p.common), now);
        println!(
            "  {}",
            cells.iter().map(|c| cell(c)).collect::<Vec<_>>().join(",")
        );
        if let Some(m) = problem {
            problems.push((p.key.clone(), m));
        }
    }
    if !problems.is_empty() {
        println!(
            "beads_sync_problems[{}]{{project,problem}}:",
            problems.len()
        );
        for (k, m) in problems {
            println!("  {},{}", cell(&k), cell(&m));
        }
    }
}

/// vegad's git sync (docs/design/git-sync.md): one row per known repo.
fn git_report() {
    use vega_bd_sync::git;
    let now = vega_bd_sync::unix_now();
    let claimed = git::claimed();
    let repos = git::known_repos(claimed);
    if repos.is_empty() {
        return;
    }
    println!("git_sync[{}]{{{}}}:", repos.len(), git::COLUMNS);
    let mut problems = Vec::new();
    for r in &repos {
        let (cells, problem) = git::row(r, &git::state(&r.key, &r.common), now);
        println!(
            "  {}",
            cells.iter().map(|c| cell(c)).collect::<Vec<_>>().join(",")
        );
        if let Some(m) = problem {
            problems.push((r.key.clone(), m));
        }
    }
    if !problems.is_empty() {
        println!("git_sync_problems[{}]{{project,problem}}:", problems.len());
        for (k, m) in problems {
            println!("  {},{}", cell(&k), cell(&m));
        }
    }
}

fn report(project: Option<&str>) {
    gh_report();
    tea_report();
    beads_report();
    git_report();
    // Provider headroom from token-burn's stored sample (advisory).
    let now = vega_herdr::capacity::now();
    let ledger = vega_herdr::resets::read();
    for line in vega_herdr::capacity::status_lines(vega_herdr::capacity::read(), &ledger, now) {
        println!("{line}");
    }
    // The process cwd: `Me.cwd` is shortened to `~/...` for display.
    let cwd_buf = env::current_dir().unwrap_or_default();
    let cwd = cwd_buf.to_string_lossy();
    let dir = cwd_buf.as_path();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let key = project.unwrap_or(&cwd);
    let beads = beads::has_beads(dir);
    // Since when: the tracker bead with beads, else the state file, else
    // the claim time.
    let tracker = if beads {
        match beads::trackers(dir).as_deref() {
            Some([t]) => Some(t.id.clone()),
            _ => None,
        }
    } else {
        None
    };
    let file = st::project_file("uac-status", key);
    let (since, source) = match &tracker {
        Some(id) => match beads::tracker_since(dir, id).and_then(|s| parse_rfc3339(&s)) {
            Some(t) => (Some(t), format!("tracker {id}")),
            None => (None, format!("tracker {id} unreadable")),
        },
        None => {
            let last = file.as_ref().and_then(|f| read_project_timestamp(f, key));
            match last.or_else(|| uac_record_at(key)) {
                Some(t) => (Some(t), "last status".into()),
                None => (None, "no tracker, claim or earlier status".into()),
            }
        }
    };
    let Some(since) = since else {
        println!("since: unknown ({source}); run `vega uac claim` first");
        print_decisions(dir, beads);
        return;
    };
    let when = vega_herdr::local_time(Some(since)).unwrap_or_else(|| rfc3339(since));
    println!("since: {when} ({source})");
    if beads {
        match beads::closed_since(dir, &rfc3339(since)) {
            Some(b) => print_beads("landed_beads", &b, "beads closed since then"),
            None => println!("landed_beads: unknown (bd failed)"),
        }
    }
    match beads::commits_since(dir, since) {
        Some(c) if c.is_empty() => println!("landed_commits: 0 on the default branch since then"),
        Some(c) => {
            println!("landed_commits[{}]:", c.len());
            for l in c.iter().take(LANDED_MAX) {
                println!("  {}", cell(l));
            }
            if c.len() > LANDED_MAX {
                println!("  … {} more (git log --first-parent)", c.len() - LANDED_MAX);
            }
        }
        None => println!("landed_commits: unknown (no git default branch)"),
    }
    if beads {
        lane_line(dir, key, since, now);
    }
    print_decisions(dir, beads);
    match &tracker {
        Some(id) => {
            if !beads::set_tracker_since(dir, id, &rfc3339(now)) {
                println!("warning: could not record this status on tracker {id}");
            }
        }
        None => {
            if let Some(f) = file {
                let _ = write_status(&f, key, now);
            }
        }
    }
}

fn read_project_timestamp(file: &Path, key: &str) -> Option<i64> {
    let text = std::fs::read_to_string(file).ok()?;
    let value: Value = serde_json::from_str(&text).ok()?;
    (value["schema"] == 2 && value["project"] == key).then_some(())?;
    value["at"].as_i64()
}

fn write_status(file: &Path, key: &str, at: i64) -> std::io::Result<()> {
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(
        file,
        serde_json::json!({"schema":2,"project":key,"at":at}).to_string(),
    )
}

/// Labels the beads the project's lanes created since the last status
/// (uac-lane), then prints the lane backlog: open, the trend since then, the
/// oldest open one.
fn lane_line(dir: &Path, key: &str, since: i64, now: i64) {
    let mine = lanes::actor(dir);
    let lanes: Vec<lanes::Lane> = lanes::lanes(key)
        .into_iter()
        .filter(|l| Some(&l.actor) != mine.as_ref())
        .collect();
    // An hour of slack: a bead created while the last status ran.
    let mut labelled = 0;
    if !lanes.is_empty() {
        match beads::created_after(dir, &rfc3339(since - 3600)) {
            Some(rows) => {
                let ids = to_label(&rows, &lanes);
                if !ids.is_empty() {
                    if beads::add_label(dir, &ids, lanes::LABEL) {
                        labelled = ids.len();
                    } else {
                        println!(
                            "warning: could not label {} lane beads {}",
                            ids.len(),
                            lanes::LABEL
                        );
                    }
                }
            }
            None => println!("warning: bd list failed; lane beads not labelled"),
        }
    }
    match beads::labelled(dir, lanes::LABEL) {
        Some(rows) => println!("{}", backlog(&rows, since, now, labelled)),
        None => println!("lane_beads: unknown (bd failed)"),
    }
}

/// The beads a lane created after it started that lack the lane label.
fn to_label(rows: &[beads::Row], lanes: &[lanes::Lane]) -> Vec<String> {
    rows.iter()
        .filter(|r| !r.labels.iter().any(|l| l == lanes::LABEL))
        .filter(|r| {
            let at = parse_rfc3339(&r.created_at);
            lanes
                .iter()
                .any(|l| l.actor == r.created_by && at.is_some_and(|t| t >= l.since))
        })
        .map(|r| r.id.clone())
        .collect()
}

/// `lane_beads: 12 open (label uac-lane), +3 created/-2 closed since then, oldest v-7 4d`.
fn backlog(rows: &[beads::Row], since: i64, now: i64, labelled: usize) -> String {
    let after = |t: Option<&str>| t.and_then(parse_rfc3339).is_some_and(|t| t >= since);
    let open: Vec<&beads::Row> = rows.iter().filter(|r| r.open).collect();
    let created = rows.iter().filter(|r| after(Some(&r.created_at))).count();
    let closed = rows
        .iter()
        .filter(|r| after(r.closed_at.as_deref()))
        .count();
    let oldest = open
        .iter()
        .filter_map(|r| Some((parse_rfc3339(&r.created_at)?, &r.id)))
        .min()
        .map(|(t, id)| format!(", oldest {id} {}", age(now - t)))
        .unwrap_or_default();
    let new = if labelled > 0 {
        format!(" ({labelled} labelled now)")
    } else {
        String::new()
    };
    format!(
        "lane_beads: {} open (label {}), +{created} created/-{closed} closed since then{new}{oldest}",
        open.len(),
        lanes::LABEL
    )
}

/// At claim: answers that came in while no UAC ran, and what still waits.
fn boot_decisions(cwd: &Path) {
    if beads::has_beads(cwd) {
        print_decisions(cwd, true);
    }
}

/// Open decisions, split: answered by the Slayer (last comment is theirs,
/// e.g. while no UAC ran) for the UAC to act on, and still waiting.
fn print_decisions(dir: &Path, beads: bool) {
    if beads {
        let Some(all) = beads::decisions(dir) else {
            println!("decisions: unknown (bd failed)");
            return;
        };
        let (answered, waiting): (Vec<_>, Vec<_>) = all
            .into_iter()
            .map(|b| {
                let a = beads::slayer_answer(dir, &b.id);
                (b, a)
            })
            .partition(|(_, a)| a.is_some());
        if !answered.is_empty() {
            println!("answered[{}]{{id,title,answer}}:", answered.len());
            for (b, a) in &answered {
                println!(
                    "  {},{},{}",
                    b.id,
                    cell(&shorten(&b.title, 60)),
                    cell(&shorten(a.as_deref().unwrap_or(""), 200))
                );
            }
            println!("help: act on each answer, then `bd close <id> --reason \"<answer>\"`");
        }
        let waiting: Vec<beads::Bead> = waiting.into_iter().map(|(b, _)| b).collect();
        if waiting.is_empty() {
            println!("decisions: 0 waiting for the Slayer");
        } else {
            print_beads("decisions", &waiting, "");
        }
    } else {
        println!("decisions: no beads in this repo; see the handover notes");
    }
}

/// `YYYY-MM-DDTHH:MM:SS` with `Z` or an offset (bd's timestamps) as Unix seconds.
fn parse_rfc3339(s: &str) -> Option<i64> {
    let num = |r: std::ops::Range<usize>| s.get(r)?.parse::<i64>().ok();
    let (y, m, d) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let (hh, mm, ss) = (num(11..13)?, num(14..16)?, num(17..19)?);
    let rest = s
        .get(19..)?
        .trim_start_matches(|c: char| c == '.' || c.is_ascii_digit());
    let offset = match rest {
        "Z" | "" => 0,
        o => {
            let sign = if o.starts_with('-') { -1 } else { 1 };
            let h = o.get(1..3)?.parse::<i64>().ok()?;
            let mi = o.get(4..6)?.parse::<i64>().ok()?;
            sign * (h * 3600 + mi * 60)
        }
    };
    // Days from civil (Howard Hinnant), the inverse of `rfc3339`.
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(days * 86400 + hh * 3600 + mm * 60 + ss - offset)
}

fn print_beads(name: &str, beads: &[beads::Bead], empty: &str) {
    if beads.is_empty() {
        println!("{name}: 0 {empty}");
        return;
    }
    println!("{name}[{}]{{id,title}}:", beads.len());
    for b in beads.iter().take(LANDED_MAX) {
        println!("  {},{}", b.id, cell(&shorten(&b.title, 80)));
    }
    if beads.len() > LANDED_MAX {
        println!("  … {} more", beads.len() - LANDED_MAX);
    }
}

/// Unix seconds as `YYYY-MM-DDTHH:MM:SSZ` (bd's `--closed-after`).
fn rfc3339(unix: i64) -> String {
    let days = unix.div_euclid(86400);
    let secs = unix.rem_euclid(86400);
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        secs / 3600,
        secs % 3600 / 60,
        secs % 60
    )
}

/// The rows that need the coordinator, with the done report and the literal
/// next command; `all_done` when every sentinel with a parent is completed.
fn attention_table(rows: &[Row], host: &str) -> Vec<String> {
    let need: Vec<&Row> = rows
        .iter()
        .filter(|r| r.attention.is_some() || r.dialog.is_some())
        .collect();
    if need.is_empty() {
        return Vec::new();
    }
    let mut out = vec![format!(
        "attention[{}]{{machine,pane,attention,report,next}}:",
        need.len()
    )];
    for r in &need {
        let a = match &r.dialog {
            Some(d) if d.kind == vega_herdr::dialog::Kind::Question => Attention::Asking,
            Some(_) => Attention::Input,
            None => r.attention.unwrap_or(Attention::Completed),
        };
        let m = (r.machine != host).then_some(r.machine.as_str());
        let mflag = m.map(|m| format!(" --machine {m}")).unwrap_or_default();
        // The abort, question or last lines of a silent turn, else the done
        // report.
        let quota = r.silence.as_ref().and_then(|x| x.quota.as_ref());
        let park = matches!(a, Attention::Parked | Attention::Decided)
            .then(|| st::park_of(m, &r.pane))
            .flatten();
        let report = if let Some(d) = &r.dialog {
            format!("{}: {}", d.kind.as_str(), d.title)
        } else if a == Attention::Waiting {
            format!("ci watch: {}", r.ci.join(" "))
        } else if let Some(p) = park {
            let closed = if p.closed { " (closed)" } else { "" };
            shorten(&format!("{}{closed}: {}", p.bead, p.title), 200)
        } else if let Some(q) = quota.filter(|_| a == Attention::Quota) {
            shorten(&q.summary(), 200)
        } else if let Some(x) = r.silence.as_ref().filter(|_| r.attention.is_some()) {
            shorten(x.question.as_deref().unwrap_or(&x.last), 200)
        } else {
            st::done_of(m, &r.pane)
                .map(|d| shorten(&d.summary, 100))
                .unwrap_or_default()
        };
        // `io:w1:p2` off this machine: selecting it in herdr (vega open)
        // jumps there.
        out.push(format!(
            "  {},{},{},{},{}",
            cell(&r.machine),
            vega_herdr::qualify(m, &r.pane),
            a.as_str(),
            cell(&report),
            cell(&match quota.filter(|_| a == Attention::Quota) {
                Some(q) => q.next(&r.pane, &mflag),
                // Headed: the owner answers in the pane; vegad does not wake.
                None if st::Mode::parse(&r.mode).is_some_and(|m| st::for_user(a, m)) => {
                    "for the user (headed pane; no wake)".to_string()
                }
                None => a.next(&r.pane, &mflag),
            })
        ));
    }
    let sentinels = rows.iter().filter(|r| r.parent != "-").count();
    let completed = need
        .iter()
        .filter(|r| r.parent != "-" && r.attention == Some(Attention::Completed))
        .count();
    if sentinels > 0 && completed == sentinels {
        out.push(format!(
            "all_done: {completed} of {sentinels} sentinels completed; land or release each"
        ));
    }
    out
}

fn shorten(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(max - 1).collect::<String>())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skill_has_valid_frontmatter() {
        vega_plugin::assert_guide(SKILL, "uac");
        let fm = vega_plugin::parse_frontmatter(SKILL).unwrap();
        assert!(fm
            .description
            .contains("Use only when the user runs /vega uac"));
        assert!(SKILL.contains("`vega uac rules`"));
        // The served role must expose the PagerDuty authority boundary, not only a doc link.
        let words = SKILL.split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(words.contains("Agents may investigate, acknowledge and add notes."));
        assert!(
            words.contains("Resolve/escalate/reassign require a Slayer-approved board decision.")
        );
        assert!(SKILL.contains("[Design](pagerduty.md)"));
    }

    #[test]
    fn rules_pass_the_skill_lint() {
        let fm = vega_plugin::parse_frontmatter(RULES).unwrap();
        assert_eq!(fm.name, "vega-uac-rules");
        assert_eq!(vega_plugin::lint_guide(RULES), Vec::<String>::new());
    }

    #[test]
    fn remote_membership_uses_fresh_v2_id_and_normalizes_host_case_without_reading_paths() {
        use serde_json::json;
        use vega_registry::{agent_key, beat_key, View, FRESH};
        let project = "github.com/team/repo";
        let mut view = View::default();
        view.entries
            .insert(beat_key("proteus"), json!({"at":unix_now()}));
        let key = agent_key("proteus", "p1");
        let agent = json!({"schema":2,"project":project,"repo_key":"/remote/unavailable/.bare","cwd":"/another/remote/path"});
        view.entries.insert(key.clone(), agent.clone());
        assert!(remote_in_project(Some(&view), "PrOtEuS", "p1", project));
        assert!(!remote_in_project(
            Some(&view),
            "proteus",
            "p1",
            "github.com/team/other"
        ));
        assert!(!remote_in_project(None, "proteus", "p1", project));
        for invalid in [
            json!({"repo_key":project}),
            json!({"schema":1,"project":project}),
            json!({"project":project}),
            json!({"schema":2,"project":""}),
        ] {
            view.entries.insert(key.clone(), invalid);
            assert!(!remote_in_project(Some(&view), "proteus", "p1", project));
        }
        view.entries.insert(key, agent);
        view.entries.insert(
            beat_key("proteus"),
            json!({"at":unix_now()-FRESH.as_secs() as i64-1}),
        );
        assert!(!remote_in_project(Some(&view), "PrOtEuS", "p1", project));
    }

    #[test]
    fn claim_uses_workspace_location_when_cwd_disagrees_and_rejects_identity_change() {
        let tmp = tempfile::tempdir().unwrap();
        for folder in ["workspace", "cwd"] {
            let dir = tmp.path().join(folder);
            assert!(std::process::Command::new("git")
                .args(["init", "-q"])
                .arg(&dir)
                .status()
                .unwrap()
                .success());
            assert!(std::process::Command::new("git")
                .arg("-C")
                .arg(&dir)
                .args([
                    "remote",
                    "add",
                    "origin",
                    &format!("https://github.com/team/{folder}.git")
                ])
                .status()
                .unwrap()
                .success());
        }
        let cwd = tmp.path().join("cwd");
        let common = tmp.path().join("workspace/.git");
        let ws = vec![
            serde_json::json!({"workspace_id":"w1","worktree":{"repo_key":common,"checkout_path":tmp.path().join("workspace")}}),
        ];
        let key = project_key(&ws, "w1", cwd.to_str().unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(key, "github.com/team/workspace");
        let path = project_path(&ws, "w1", cwd.to_str().unwrap());
        let resolved = claim_project(&key, &path).unwrap();
        assert_eq!(resolved.location.common_dir, common.canonicalize().unwrap());
        assert_eq!(
            resolved.location.checkout,
            tmp.path().join("workspace").canonicalize().unwrap()
        );
        assert_eq!(
            vega_registry::project_folder(resolved.location.common_dir.to_str().unwrap()),
            "workspace"
        );
        assert_eq!(
            claim_project("github.com/team/cwd", &path)
                .unwrap_err()
                .code,
            "project_identity_changed"
        );
    }

    #[test]
    fn status_round_trip_requires_matching_v2_project_and_rejects_legacy() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("v2/status.json");
        let key = "github.com/team/repo";
        write_status(&file, key, 42).unwrap();
        assert_eq!(read_project_timestamp(&file, key), Some(42));
        assert_eq!(read_project_timestamp(&file, "github.com/other/repo"), None);
        for value in [
            serde_json::json!(42),
            serde_json::json!({"project":key,"at":42}),
            serde_json::json!({"schema":1,"project":key,"at":42}),
        ] {
            std::fs::write(&file, value.to_string()).unwrap();
            assert_eq!(read_project_timestamp(&file, key), None);
        }
    }

    #[test]
    fn takeover_retains_migrated_aliases_and_tracker() {
        let prior = serde_json::json!({"schema":2,"project":"github.com/team/repo","pane":"gone","folder_name":"first-clone","origin_name":"repo","aliases":["another-clone"],"tracker":"bead-1","at":10});
        let next = serde_json::json!({"schema":2,"project":"github.com/team/repo","pane":"new","folder_name":"second-clone","origin_name":"repo","at":20});
        let record = claim_record(prior, next, None);
        assert_eq!(record["pane"], "new");
        assert_eq!(record["at"], 20);
        assert_eq!(record["tracker"], "bead-1");
        assert_eq!(
            record["aliases"],
            serde_json::json!(["another-clone", "first-clone", "repo", "second-clone"])
        );
    }

    #[test]
    fn project_key_without_repo_key_uses_the_cwd_repo() {
        // A workspace made before `git init` has no worktree.repo_key.
        let dir = tempfile::tempdir().unwrap();
        let ok = std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(dir.path())
            .status()
            .unwrap()
            .success();
        assert!(ok);
        let ws = vec![serde_json::json!({"workspace_id": "w1"})];
        let cwd = dir.path().to_str().unwrap();
        assert!(project_key(&ws, "w1", cwd).unwrap().is_some());
        // Me.cwd arrives collapsed to `~/...`: it must still reach the filesystem.
        let home = env::var("HOME").unwrap();
        assert_eq!(expand_home("~/src/x"), Path::new(&home).join("src/x"));
        assert_eq!(expand_home("/abs"), Path::new("/abs"));
    }

    #[test]
    fn home_relative_strips_any_user_home() {
        assert_eq!(
            vega_herdr::home_relative("/home/marcel/src/sk/.bare"),
            "~/src/sk/.bare"
        );
        assert_eq!(
            vega_herdr::home_relative("/Users/someone/src/vega/.git"),
            "~/src/vega/.git"
        );
        assert_eq!(
            vega_herdr::home_relative("/srv/repo/.git"),
            "/srv/repo/.git"
        );
    }

    #[test]
    fn agent_table_has_machine_and_keeps_empty_cells() {
        let rows = [Row {
            machine: "ganymede".into(),
            pane: "w5S:p1".into(),
            name: "fix".into(),
            tab: "fix".into(),
            kind: "claude".into(),
            status: "working".into(),
            dialog: None,
            attention: None,
            silence: None,
            ci: Vec::new(),
            parent: "w3V:p1 on proteus".into(),
            mode: "headless".into(),
            bead: String::new(),
            unread: Some(1500),
            rank: 1,
        }];
        assert_eq!(
            agent_table(&rows),
            [
                "agents[1]{machine,pane,name,tab,kind,status,attention,unread_mail,parent,mode,bead}:",
                "  ganymede,w5S:p1,fix,fix,claude,working,,25m old,w3V:p1 on proteus,headless,",
            ]
        );
        assert_eq!(
            [age(-3), age(45), age(7500), age(200_000)],
            ["0s", "45s", "2h05m", "2d"]
        );
    }

    fn row(pane: &str, machine: &str, attention: Option<Attention>) -> Row {
        Row {
            machine: machine.into(),
            pane: pane.into(),
            name: "x".into(),
            tab: String::new(),
            kind: "claude".into(),
            status: "idle".into(),
            dialog: None,
            attention,
            silence: None,
            ci: Vec::new(),
            parent: "w3V:p1".into(),
            mode: String::new(),
            bead: String::new(),
            unread: None,
            rank: 0,
        }
    }

    #[test]
    fn ask_targets_local_and_remote_skip_gone() {
        let mut gone = row("w9:p1", "proteus", Some(Attention::Exited));
        gone.status = "gone".into();
        let rows = [
            row("w1:p1", "proteus", None),
            row("w2:p1", "Ganymede", None),
            gone,
        ];
        assert_eq!(ask_targets(&rows, "proteus"), ["w1:p1", "w2:p1@ganymede"]);
    }

    fn bead(id: &str, by: &str, at: &str, closed: Option<&str>, labels: &[&str]) -> beads::Row {
        beads::Row {
            id: id.into(),
            created_by: by.into(),
            created_at: at.into(),
            closed_at: closed.map(String::from),
            open: closed.is_none(),
            labels: labels.iter().map(|l| l.to_string()).collect(),
        }
    }

    #[test]
    fn lane_beads_are_the_unlabelled_ones_a_lane_created_after_it_started() {
        let start = parse_rfc3339("2026-10-09T10:00:00Z").unwrap();
        let lanes = [lanes::Lane {
            actor: "proteus/fix".into(),
            since: start,
        }];
        let rows = [
            bead("v-1", "proteus/fix", "2026-10-09T11:00:00Z", None, &[]),
            bead("v-2", "proteus/fix", "2026-10-09T09:00:00Z", None, &[]),
            bead("v-3", "proteus/main", "2026-10-09T11:00:00Z", None, &[]),
            bead("v-4", "proteus/fix", "2026-10-09T11:59:00+02:00", None, &[]),
            bead(
                "v-5",
                "proteus/fix",
                "2026-10-09T13:00:00Z",
                None,
                &["uac-lane"],
            ),
            bead(
                "v-6",
                "proteus/fix",
                "2026-10-09T13:00:00Z",
                Some("2026-10-09T14:00:00Z"),
                &["x"],
            ),
        ];
        assert_eq!(to_label(&rows, &lanes), ["v-1", "v-6"]);
    }

    #[test]
    fn backlog_counts_open_trend_and_oldest() {
        let since = parse_rfc3339("2026-10-09T10:00:00Z").unwrap();
        let now = since + 3 * 86400;
        let rows = [
            bead("v-1", "a", "2026-10-07T10:00:00Z", None, &[]),
            bead("v-2", "a", "2026-10-09T11:00:00Z", None, &[]),
            bead(
                "v-3",
                "a",
                "2026-10-08T11:00:00Z",
                Some("2026-10-09T12:00:00Z"),
                &[],
            ),
            bead(
                "v-4",
                "a",
                "2026-10-09T11:30:00Z",
                Some("2026-10-09T12:00:00Z"),
                &[],
            ),
        ];
        assert_eq!(
            backlog(&rows, since, now, 2),
            "lane_beads: 2 open (label uac-lane), +2 created/-2 closed since then (2 labelled now), oldest v-1 5d"
        );
        assert_eq!(
            backlog(&[], since, now, 0),
            "lane_beads: 0 open (label uac-lane), +0 created/-0 closed since then"
        );
    }

    #[test]
    fn rfc3339_parses_back() {
        for t in [0, 951_825_600, 1_791_478_000] {
            assert_eq!(parse_rfc3339(&rfc3339(t)), Some(t));
        }
        assert_eq!(
            parse_rfc3339("2026-10-08T18:46:40+02:00"),
            Some(1_791_478_000)
        );
        assert_eq!(
            parse_rfc3339("2026-10-08T16:46:40.123Z"),
            Some(1_791_478_000)
        );
        assert_eq!(parse_rfc3339("nope"), None);
    }

    #[test]
    fn rfc3339_from_unix() {
        assert_eq!(rfc3339(0), "1970-01-01T00:00:00Z");
        assert_eq!(rfc3339(1_791_478_000), "2026-10-08T16:46:40Z");
        assert_eq!(rfc3339(951_825_600), "2000-02-29T12:00:00Z");
    }

    #[test]
    fn local_dialog_attention_includes_kind_and_title() {
        let mut r = row("w1:p1", "proteus", None);
        r.dialog = vega_herdr::dialog::classify(
            "Hooks need review\n› 1. Review hooks\n2. Trust all and continue",
            "codex",
            "unknown",
        );
        let out = attention_table(&[r], "proteus");
        assert!(out[1].contains(",input,hooks-trust: Hooks need review,"));
    }
    #[test]
    fn attention_table_has_next_commands_and_all_done() {
        std::env::set_var("XDG_STATE_HOME", "/nonexistent/vega-test");
        let rows = [
            row("w1:p1", "proteus", Some(Attention::Silent)),
            row("w2:p1", "ganymede", Some(Attention::Exited)),
            row("w3:p1", "proteus", None),
        ];
        assert_eq!(
            attention_table(&rows, "proteus"),
            [
                "attention[2]{machine,pane,attention,report,next}:",
                "  proteus,w1:p1,silent,,vega mux show w1:p1",
                "  ganymede,ganymede:w2:p1,exited,,vega mux --machine ganymede close w2:p1",
            ]
        );
        let done = [
            row("w1:p1", "proteus", Some(Attention::Completed)),
            row("w2:p1", "proteus", Some(Attention::Completed)),
        ];
        let out = attention_table(&done, "proteus");
        assert_eq!(
            out.last().unwrap(),
            "all_done: 2 of 2 sentinels completed; land or release each"
        );
        assert!(attention_table(&rows[2..], "proteus").is_empty());
    }

    #[test]
    fn asking_shows_the_question_silent_the_last_lines() {
        std::env::set_var("XDG_STATE_HOME", "/nonexistent/vega-test");
        let mut asks = row("w1:p1", "proteus", Some(Attention::Asking));
        asks.silence = Some(st::Silence {
            question: Some("Should I also cover Hamburg?".into()),
            last: "Should I also cover Hamburg?".into(),
            quota: None,
        });
        let mut quiet = row("w2:p1", "proteus", Some(Attention::Silent));
        quiet.silence = Some(st::Silence {
            question: None,
            last: "Tests pass.".into(),
            quota: None,
        });
        let mut dialog = row("w3:p1", "proteus", None);
        dialog.dialog = vega_herdr::dialog::classify(
            "Question 1/1 (1 unanswered)\nWhich colour?\n› 1. Red\n2. Blue",
            "codex",
            "blocked",
        );
        let out = attention_table(&[asks, quiet, dialog], "proteus");
        assert_eq!(
            out[1],
            "  proteus,w1:p1,asking,Should I also cover Hamburg?,\
             answer it (vega mux send w1:p1) or ask the Slayer"
        );
        assert_eq!(
            out[2],
            "  proteus,w2:p1,silent,Tests pass.,vega mux show w2:p1"
        );
        assert!(out[3].contains(",asking,question: Which colour?,"));
    }

    #[test]
    fn headed_asking_is_marked_for_the_user() {
        std::env::set_var("XDG_STATE_HOME", "/nonexistent/vega-test");
        let mut asks = row("w1:p1", "proteus", Some(Attention::Asking));
        asks.mode = "headed".into();
        asks.silence = Some(st::Silence {
            question: Some("Rot oder blau?".into()),
            last: "Rot oder blau?".into(),
            quota: None,
        });
        let mut gone = row("w2:p1", "proteus", Some(Attention::Exited));
        gone.mode = "headed".into();
        let out = attention_table(&[asks, gone], "proteus");
        assert_eq!(
            out[1],
            "  proteus,w1:p1,asking,Rot oder blau?,for the user (headed pane; no wake)"
        );
        assert!(!out[2].contains("for the user"), "{}", out[2]);
    }

    #[test]
    fn calls_table_lists_panes_and_labels_sessions() {
        let panes = vec![("w3V:p1".to_string(), 4)];
        let sessions = vec![
            ("codex:01a12009-494e-7831".to_string(), 9),
            ("01a1200c-1485-7e13".to_string(), 2),
        ];
        assert_eq!(
            calls_table("gh", &panes, &sessions),
            [
                "gh_calls[1]{pane,last_hour}:",
                "  w3V:p1,4",
                "gh_calls_without_pane[2]{session,last_hour}:",
                "  codex 01a12009,9",
                "  session 01a1200c,2",
            ]
        );
        assert_eq!(
            calls_table("gh", &[], &[]),
            ["gh_calls: 0 by agent panes on this host in the last hour"]
        );
        assert_eq!(
            tea_lines(calls_table("tea", &panes, &[])),
            ["tea_calls[1]{pane,last_hour}:", "  w3V:p1,4"]
        );
        assert!(tea_lines(calls_table("tea", &[], &[])).is_empty());
    }

    #[test]
    fn waiting_shows_the_watched_target() {
        let mut w = row("w1:p1", "proteus", Some(Attention::Waiting));
        w.ci = vec!["o/r#42".into()];
        assert_eq!(
            attention_table(&[w], "proteus")[1],
            "  proteus,w1:p1,waiting,ci watch: o/r#42,wait: the ci verdict mail wakes w1:p1"
        );
    }

    #[test]
    fn quota_shows_the_abort_and_when_to_retry() {
        std::env::set_var("XDG_STATE_HOME", "/nonexistent/vega-test");
        let abort = |transient, resets: Option<&str>| {
            Some(st::Silence {
                question: None,
                last: String::new(),
                quota: Some(st::Quota {
                    transient,
                    error: "You've hit your limit · resets 3pm".into(),
                    resets: resets.map(String::from),
                }),
            })
        };
        let mut spent = row("w1:p1", "proteus", Some(Attention::Quota));
        spent.silence = abort(false, Some("3pm"));
        let mut busy = row("w2:p1", "ganymede", Some(Attention::Quota));
        busy.silence = abort(true, None);
        let out = attention_table(&[spent, busy], "proteus");
        assert_eq!(
            out[1],
            "  proteus,w1:p1,quota,\"quota exhausted, resets 3pm: You've hit your limit · resets 3pm\",\
             \"retry after 3pm: vega mux send w1:p1 \\\"continue\\\", or move the work to another kind/provider\""
        );
        assert!(
            out[2]
                .ends_with("retry soon: vega mux --machine ganymede send w2:p1 \\\"continue\\\"\""),
            "{}",
            out[2]
        );
    }
    #[test]
    fn same_uac_claims_retain_both_clones_and_the_primary_checkout() {
        const CHILD: &str = "VEGA_UAC_CLONE_CLAIM_TEST";
        if std::env::var_os(CHILD).is_none() {
            let root = tempfile::tempdir().unwrap();
            let root = root.path().canonicalize().unwrap();
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "tests::same_uac_claims_retain_both_clones_and_the_primary_checkout",
                ])
                .env(CHILD, "1")
                .env("HOME", &root)
                .env("XDG_STATE_HOME", root.join("state"))
                .env("VEGA_POST_HOST", "fixture")
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            return;
        }
        let root = std::path::PathBuf::from(std::env::var_os("HOME").unwrap());
        let mut locations = Vec::new();
        for name in ["a", "b"] {
            let path = root.join(name);
            std::fs::create_dir(&path).unwrap();
            assert!(std::process::Command::new("git")
                .arg("-C")
                .arg(&path)
                .args(["init", "-q"])
                .status()
                .unwrap()
                .success());
            assert!(std::process::Command::new("git")
                .arg("-C")
                .arg(&path)
                .args([
                    "remote",
                    "add",
                    "origin",
                    "https://github.com/team/shared.git"
                ])
                .status()
                .unwrap()
                .success());
            let project = vega_herdr::project::resolve(&path, &st::post_host()).unwrap();
            claim(project.id.as_str(), "p1", &[], None, &path).unwrap();
            locations.push(project.location);
        }
        let file = st::project_file("uac", "github.com/team/shared").unwrap();
        let record: Value = serde_json::from_str(&std::fs::read_to_string(file).unwrap()).unwrap();
        assert_eq!(
            record["common_dir"],
            serde_json::json!(locations[0].common_dir)
        );
        assert_eq!(record["checkout"], serde_json::json!(locations[0].checkout));
        let retained = st::project_uac_locations();
        for location in locations {
            assert!(retained.contains(&("github.com/team/shared".into(), location)));
        }
        let repos = vega_bd_sync::git::repos(vega_bd_sync::git::claimed());
        assert_eq!(repos.len(), 2);
    }
}
