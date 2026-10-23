//! `vega ci`: CI watch for GitHub and Forgejo/Gitea (docs/design/ci-watch.md).
//!
//! `vega ci watch` registers a PR or branch for this pane and returns at once;
//! vegad polls each watched target once per host with conditional requests
//! and mails the watchers a new verdict (green, red with the failed checks).
//! `vega ci` and `vega ci show` read vegad's cache: no API call.

use std::env;
use std::process::{Command, ExitCode};

use clap::error::ErrorKind;
use clap::{CommandFactory, Parser, Subcommand};
use vega_ci_watch::checks::BUCKETS;
use vega_ci_watch::forge;
use vega_ci_watch::store::{self, Watch};
use vega_ci_watch::target::parse_remote;
use vega_ci_watch::{unix_now, Bucket, Target};
use vega_herdr::{cell, collapse_home, local_time, print_failure, Failure};

const SKILL: &str = include_str!("../SKILL.md");

#[derive(Parser)]
#[command(
    name = "vega ci",
    bin_name = "vega ci",
    disable_version_flag = true,
    about = "CI watch for GitHub and Forgejo/Gitea: one poller per host (vegad), the verdict arrives as mail",
    after_help = "Without a command: this host's watches from vegad's cache (no API call) and, for GitHub, the gh rate limit.\n\nExamples:\n  vega ci                      # watches, verdicts, gh limit\n  vega ci watch                # the current branch; the result arrives as mail\n  vega ci watch 42             # PR #42 of this checkout's repo\n  vega ci show o/r#42          # the checks of the current head\n  vega ci unwatch 42"
)]
struct Cli {
    /// Print the agent guide for this plugin, SKILL.md form (read by `vega skills`)
    #[arg(long)]
    skill: bool,
    /// Print the bare version
    #[arg(short = 'V', short_alias = 'v', long)]
    version: bool,
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Register a PR or branch; vegad mails you its verdict (returns at once)
    #[command(
        after_help = include_str!("../help/watch.md")
    )]
    Watch {
        #[command(flatten)]
        sel: Sel,
        /// The pane to mail [default: this pane]
        #[arg(long = "for")]
        pane: Option<String>,
    },
    /// Stop watching a target
    Unwatch {
        #[command(flatten)]
        sel: Sel,
        #[arg(long = "for")]
        pane: Option<String>,
    },
    /// The failed, pending and cancelled checks of a watched target's current head
    Show {
        #[command(flatten)]
        sel: Sel,
        /// Also list the passed and skipped checks
        #[arg(long)]
        all: bool,
    },
}

/// What to watch: a target argument, or a GitHub Actions run.
#[derive(clap::Args)]
struct Sel {
    #[arg(conflicts_with_all = ["run", "workflow"])]
    target: Option<String>,
    /// owner/repo [default: the checkout's remote]
    #[arg(short = 'R', long)]
    repo: Option<String>,
    /// A GitHub Actions run id (a workflow_dispatch run has no PR)
    #[arg(long, conflicts_with = "workflow")]
    run: Option<u64>,
    /// A workflow file or id: its newest workflow_dispatch run
    #[arg(long)]
    workflow: Option<String>,
    /// The branch, tag or sha the workflow was dispatched on [default: any]
    #[arg(long = "ref", requires = "workflow")]
    git_ref: Option<String>,
}

fn main() -> ExitCode {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(e) if matches!(e.kind(), ErrorKind::DisplayHelp) => {
            let _ = e.print();
            return ExitCode::SUCCESS;
        }
        Err(e) => {
            let msg = e.to_string();
            let message = msg
                .lines()
                .take_while(|l| !l.trim().is_empty())
                .map(str::trim)
                .collect::<Vec<_>>()
                .join(" ");
            print_failure(&Failure::new(
                "usage",
                message.trim_start_matches("error: "),
                "valid usage below",
            ));
            print!("{}", Cli::command().render_help());
            return ExitCode::from(2);
        }
    };
    if cli.version {
        println!("{}", vega_build::version_line!());
        return ExitCode::SUCCESS;
    }
    if cli.skill {
        print!("{SKILL}");
        return ExitCode::SUCCESS;
    }
    let result = match cli.cmd {
        None => home(),
        Some(Cmd::Watch { sel, pane }) => watch(&sel, pane),
        Some(Cmd::Unwatch { sel, pane }) => unwatch(&sel, pane),
        Some(Cmd::Show { sel, all }) => show(&sel, all),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(f) => {
            print_failure(&f);
            ExitCode::from(if f.code == "usage" { 2 } else { 1 })
        }
    }
}

fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .args(args)
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (out.status.success() && !s.is_empty()).then_some(s)
}

/// The checkout's upstream remote and branch: `origin/feat` → (origin, feat);
/// without an upstream, (origin, current branch).
fn current_branch() -> Option<(String, String)> {
    if let Some(up) = git(&[
        "rev-parse",
        "--abbrev-ref",
        "--symbolic-full-name",
        "@{upstream}",
    ]) {
        if let Some((remote, branch)) = up.split_once('/') {
            return Some((remote.to_string(), branch.to_string()));
        }
    }
    Some(("origin".into(), git(&["branch", "--show-current"])?))
}

/// (host, owner/repo) from `-R`, else from the checkout's remote.
fn default_repo(repo: Option<&str>, remote: &str) -> Option<(String, String)> {
    match repo {
        Some(r) => Some(match r.split_once('/') {
            Some((h, rest)) if h.contains('.') => (h.to_string(), rest.to_string()),
            _ => (vega_ci_watch::target::GITHUB.to_string(), r.to_string()),
        }),
        None => parse_remote(&git(&["remote", "get-url", remote])?),
    }
}

fn resolve(sel: &Sel) -> Result<Target, Failure> {
    let (target, repo) = (sel.target.as_deref(), sel.repo.as_deref());
    if sel.run.is_some() || sel.workflow.is_some() {
        let (host, repo) = default_repo(repo, "origin").ok_or_else(|| {
            Failure::new(
                "usage",
                "no repo for the run",
                "run it in a GitHub checkout or pass -R owner/repo",
            )
        })?;
        let t = match (sel.run, &sel.workflow) {
            (Some(id), _) => Target::run(&host, &repo, id),
            (None, Some(w)) => Target::workflow(&host, &repo, w, sel.git_ref.as_deref()),
            (None, None) => unreachable!(),
        };
        return github_run(t.resolved(&mut forge::http_probe));
    }
    let (remote, arg) = match target {
        Some(t) => ("origin".to_string(), t.to_string()),
        None => current_branch().ok_or_else(|| {
            Failure::new(
                "no_target",
                "no target and no current branch here",
                "pass a PR number, URL or branch: vega ci watch 42",
            )
        })?,
    };
    let default = default_repo(repo, &remote);
    github_run(Target::from_arg(
        &arg,
        default.as_ref().map(|(h, r)| (h.as_str(), r.as_str())),
        &mut forge::http_probe,
    ))
}

/// The usage failure of a target that did not parse, or of a run on a forge
/// without GitHub Actions' API.
fn github_run(t: Result<Target, String>) -> Result<Target, Failure> {
    let t = t.map_err(|e| {
        Failure::new(
            "usage",
            e,
            "vega ci watch [42|owner/repo#42|<branch>|--run <id>|--workflow <file>] [-R owner/repo]",
        )
    })?;
    if t.is_run() && !t.forge.is_github() {
        return Err(Failure::new(
            "unsupported",
            format!("{} is not GitHub: run watches need GitHub Actions", t.host),
            "watch the PR or branch instead: vega ci watch <pr|branch>",
        ));
    }
    Ok(t)
}

/// The pane a watch mails: `--for`, else ours. Ours comes from
/// `vega_herdr::me`, which checks `HERDR_PANE_ID` against the process tree:
/// the var is stale in a long-lived process and missing in tool shells that
/// filter the env (Codex `inherit = "core"`); either would mail someone else.
fn my_pane(pane: Option<String>) -> Result<String, Failure> {
    my_pane_with(pane, vega_herdr::me)
}

fn my_pane_with(
    pane: Option<String>,
    me: impl FnOnce() -> Result<vega_herdr::Me, Failure>,
) -> Result<String, Failure> {
    if let Some(p) = pane.filter(|p| !p.is_empty()) {
        return Ok(p);
    }
    me().map(|m| m.pane).map_err(|f| match f.code.as_str() {
        "not_in_herdr" => Failure::new(
            "not_in_herdr",
            format!("no pane to mail: {}", f.message),
            "run it in your agent's pane; --for <pane> mails that pane instead, so pass your own pane id (`vega mux` lists panes), not the one you report to",
        ),
        _ => f,
    })
}

fn poller_line() -> &'static str {
    if vega_herdr::state::relay_local() {
        "poller: off (vegad in local mode, no broker: verdicts need mail); watches wait for a broker"
    } else if vega_herdr::state::relay_running() {
        "poller: vegad running"
    } else {
        "poller: vegad not running; watches wait until it runs (vega post relay install)"
    }
}

fn watch(sel: &Sel, pane: Option<String>) -> Result<(), Failure> {
    let t = resolve(sel)?;
    let pane = my_pane(pane)?;
    let w = Watch {
        target: t.clone(),
        pane: pane.clone(),
        since: unix_now(),
    };
    if !store::add_watch(&w) {
        return Err(Failure::new(
            "write_failed",
            "could not write the watch file",
            "check $XDG_STATE_HOME/vega/ci (default ~/.local/state/vega/ci)",
        ));
    }
    let others = store::watches()
        .iter()
        .filter(|o| o.target == t && o.pane != pane)
        .count();
    println!("watch: {} for {pane}", t.label());
    if others > 0 {
        println!("shared: {others} other pane(s) watch it; one poll serves all");
    }
    println!("{}", poller_line());
    println!("help: the verdict arrives as mail; do not poll. `vega ci` shows the cached state");
    Ok(())
}

fn unwatch(sel: &Sel, pane: Option<String>) -> Result<(), Failure> {
    let t = resolve(sel)?;
    let pane = my_pane(pane)?;
    if !store::remove_watch(&t, &pane) {
        return Err(Failure::new(
            "not_watched",
            format!("{pane} does not watch {}", t.label()),
            "run `vega ci` for this host's watches",
        ));
    }
    println!("unwatched: {} for {pane}", t.label());
    Ok(())
}

fn ago(secs: i64) -> String {
    match secs {
        s if s < 60 => format!("{s}s ago"),
        s if s < 3600 => format!("{}m ago", s / 60),
        s => format!("{}h ago", s / 3600),
    }
}

fn rate_line(now: i64) -> String {
    match store::load_rate() {
        Some(r) => format!(
            "gh: core {}/{} left, reset {} (seen {})",
            r.remaining,
            r.limit,
            local_time(Some(r.reset)).unwrap_or_else(|| r.reset.to_string()),
            ago(now - r.at)
        ),
        None => "gh: core limit not seen yet (vegad records it with its first poll)".into(),
    }
}

fn home() -> Result<(), Failure> {
    let now = unix_now();
    let bin = env::current_exe()
        .map(|p| collapse_home(&p.display().to_string()))
        .unwrap_or_else(|_| "unknown".into());
    println!("bin: {bin}");
    println!("description: CI watches (GitHub, Forgejo) on this host; vegad polls each once and mails its watchers a new verdict");
    println!("{}", poller_line());
    let watches = store::watches();
    // The rate headers are GitHub's: Forgejo sends none, so the line would only mislead.
    if watches.is_empty() || watches.iter().any(|w| w.target.forge.is_github()) {
        println!("{}", rate_line(now));
    }
    if watches.is_empty() {
        println!("watches: 0 on this host");
        println!("help: vega ci watch [<pr|branch>]   # the verdict arrives as mail");
        return Ok(());
    }
    let mut targets: Vec<(Target, Vec<String>)> = Vec::new();
    for w in watches {
        match targets.iter_mut().find(|(t, _)| *t == w.target) {
            Some((_, panes)) => panes.push(w.pane),
            None => targets.push((w.target, vec![w.pane])),
        }
    }
    println!(
        "watches[{}]{{target,head,verdict,pass,fail,pending,skip,cancel,watchers,polled}}:",
        targets.len()
    );
    let mut errors = Vec::new();
    for (t, panes) in &targets {
        let s = store::load_state(t).filter(|s| s.target.as_ref() == Some(t));
        let row = match &s {
            Some(s) if s.polled > 0 => {
                let sum = s.summary();
                if let Some(e) = &s.error {
                    errors.push((t.label(), e.clone()));
                }
                format!(
                    "{},{},{},{},{}",
                    s.head
                        .as_deref()
                        .map(|h| &h[..h.len().min(7)])
                        .unwrap_or("-"),
                    sum.verdict.as_str(),
                    sum.counts.map(|n| n.to_string()).join(","),
                    cell(&panes.join(" ")),
                    ago(now - s.polled)
                )
            }
            _ => {
                if let Some(e) = s.as_ref().and_then(|s| s.error.clone()) {
                    errors.push((t.label(), e));
                }
                format!("-,unpolled,-,-,-,-,-,{},-", cell(&panes.join(" ")))
            }
        };
        println!("  {},{row}", cell(&t.label()));
    }
    if !errors.is_empty() {
        println!("errors[{}]{{target,error}}:", errors.len());
        for (t, e) in errors {
            println!("  {},{}", cell(&t), cell(&e));
        }
    }
    println!("help[2]:");
    println!("  vega ci show <target>        # the checks of the current head, failed first");
    println!(
        "  vega ci unwatch <target>     # stop early; a merge, close or closed pane ends it too"
    );
    Ok(())
}

/// Show order: what needs a look first.
fn rank(b: Bucket) -> usize {
    [
        Bucket::Fail,
        Bucket::Pending,
        Bucket::Cancel,
        Bucket::Pass,
        Bucket::Skip,
    ]
    .iter()
    .position(|x| *x == b)
    .unwrap_or(9)
}

const SHOW_MAX: usize = 30;

fn show(sel: &Sel, all: bool) -> Result<(), Failure> {
    let t = resolve(sel)?;
    let s = store::load_state(&t)
        .filter(|s| s.target.as_ref() == Some(&t) && s.polled > 0)
        .ok_or_else(|| {
            let watched = store::watches().iter().any(|w| w.target == t);
            Failure::new(
                if watched { "unpolled" } else { "not_watched" },
                format!(
                    "no cached state for {}{}",
                    t.label(),
                    if watched { " yet" } else { "" }
                ),
                if watched {
                    "vegad polls it within seconds while it runs; see `vega ci`"
                } else {
                    "vega ci watch <target> first; `vega ci` lists this host's watches"
                },
            )
        })?;
    let sum = s.summary();
    println!("target: {}", t.label());
    println!("head: {}", s.head.as_deref().unwrap_or("-"));
    if let Some(p) = &s.pr_state {
        println!("pr: {p}");
    }
    if let Some(m) = &s.merge_state {
        println!(
            "merge: {m}{}",
            if s.needs_rebase() {
                " (rebase onto the base)"
            } else {
                ""
            }
        );
    }
    for id in &s.zombies {
        println!(
            "zombie: run {id}, jobs all completed; force-cancel: gh api -X POST repos/{}/actions/runs/{id}/force-cancel",
            t.repo
        );
    }
    if t.is_run() {
        match s.run_id {
            Some(id) => println!(
                "run: {id} {}",
                [s.run_status.as_deref(), s.run_conclusion.as_deref()]
                    .into_iter()
                    .flatten()
                    .collect::<Vec<_>>()
                    .join(" ")
            ),
            None => println!(
                "run: none yet (the newest workflow_dispatch run of {} since 10 min before the watch)",
                t.workflow.as_deref().unwrap_or("?")
            ),
        }
    }
    println!("verdict: {}", sum.verdict.as_str());
    println!("polled: {}", ago(unix_now() - s.polled));
    if let Some(e) = &s.error {
        println!("error: {}", cell(e));
    }
    let mut checks = s.checks();
    if let Some(c) = checks.iter().find(|c| {
        c.bucket == Bucket::Pending
            && c.note
                .as_deref()
                .is_some_and(|n| n.starts_with("Pending approval"))
    }) {
        println!(
            "note: {} says {:?}: a maintainer must comment /ok-to-test; the verdict stays pending until then",
            cell(&c.name),
            c.note.as_deref().unwrap_or_default()
        );
    }
    if checks.is_empty() {
        println!("checks: 0 for this head");
        return Ok(());
    }
    checks.sort_by(|a, b| (rank(a.bucket), &a.name).cmp(&(rank(b.bucket), &b.name)));
    println!(
        "counts: {}",
        BUCKETS
            .iter()
            .zip(sum.counts)
            .map(|(b, n)| format!("{} {n}", b.as_str()))
            .collect::<Vec<_>>()
            .join(", ")
    );
    let total = checks.len();
    if !all {
        checks.retain(|c| !matches!(c.bucket, Bucket::Pass | Bucket::Skip));
    }
    if checks.is_empty() {
        println!("checks: none failed, pending or cancelled (--all lists the {total} passed and skipped)");
    } else {
        println!("checks[{}]{{bucket,name}}:", checks.len());
        for c in checks.iter().take(SHOW_MAX) {
            println!("  {},{}", c.bucket.as_str(), cell(&c.name));
        }
        if checks.len() > SHOW_MAX {
            println!("  … {} more", checks.len() - SHOW_MAX);
        }
        if !all && checks.len() < total {
            println!("hidden: {} passed or skipped (--all)", total - checks.len());
        }
    }
    if sum.verdict == vega_ci_watch::Verdict::Red {
        if let Some(id) = s.run_id {
            println!(
                "help: gh run view {id} -R {} --log-failed (once, no loop)",
                t.repo
            );
        } else if t.forge.is_github() {
            println!("help: gh run list --commit {} -R {} then gh run view <id> --log-failed (once, no loop)", s.head.as_deref().unwrap_or("<sha>"), t.repo);
        } else {
            println!(
                "help: open the failed check's link in the PR on {} (once, no loop)",
                t.host
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn my_pane_trusts_only_for_and_the_process_tree() {
        let me = |pane: &str| {
            let pane = pane.to_string();
            move || {
                Ok(vega_herdr::Me {
                    pane,
                    workspace: String::new(),
                    agent: String::new(),
                    program: String::new(),
                    cwd: String::new(),
                })
            }
        };
        let none = || -> Result<vega_herdr::Me, Failure> {
            Err(Failure::new(
                "not_in_herdr",
                "no herdr pane hosts this process",
                "h",
            ))
        };
        assert_eq!(
            super::my_pane_with(Some("w1:p1".into()), none).unwrap(),
            "w1:p1"
        );
        // A stale HERDR_PANE_ID never wins over the pane that hosts us.
        std::env::set_var("HERDR_PANE_ID", "w9:p9");
        assert_eq!(super::my_pane_with(None, me("wDR:p2")).unwrap(), "wDR:p2");
        assert_eq!(
            super::my_pane_with(Some(String::new()), me("wDR:p2")).unwrap(),
            "wDR:p2"
        );
        std::env::remove_var("HERDR_PANE_ID");
        let f = super::my_pane_with(None, none).unwrap_err();
        assert_eq!(f.code, "not_in_herdr");
        assert!(f.message.contains("no herdr pane hosts this process"));
        assert!(f.hint.contains("your own pane"));
    }

    use super::*;

    #[test]
    fn skill_has_valid_frontmatter() {
        vega_plugin::assert_guide(SKILL, "ci");
    }

    #[test]
    fn show_order_puts_failures_first() {
        assert!(rank(Bucket::Fail) < rank(Bucket::Pending));
        assert!(rank(Bucket::Pending) < rank(Bucket::Pass));
    }
}
