//! Explicit, resumable coordinator landing. Runtime receipts are outside git.
mod changelog;
mod live;
pub mod prune;
#[cfg(test)]
mod tests;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::env;
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use vega_herdr::{cell, print_failure, Failure};

const HELP: &str = include_str!("../help/land.md");
type Result<T> = std::result::Result<T, Fault>;

#[derive(Debug)]
struct Fault {
    message: String,
    status: Option<i32>,
    output: String,
}
impl Fault {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            status: None,
            output: String::new(),
        }
    }
}
impl From<std::io::Error> for Fault {
    fn from(e: std::io::Error) -> Self {
        Self::new(e.to_string())
    }
}

#[derive(Default, Debug, PartialEq)]
struct Args {
    bead: String,
    branch: Option<String>,
    head: Option<String>,
    target: Option<String>,
    keep: bool,
    resume: bool,
    status: bool,
    retry_install: bool,
    install_done: bool,
}
fn parse(args: &[String]) -> Result<Args> {
    let mut out = Args::default();
    let mut seen = std::collections::HashSet::new();
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        if a.starts_with('-') {
            if !seen.insert(a) {
                return Err(Fault::new(format!("duplicate flag {a}")));
            }
            match a {
                "--branch" | "--head" | "--target" => {
                    i += 1;
                    let value = args
                        .get(i)
                        .filter(|v| !v.is_empty() && !v.starts_with('-'))
                        .ok_or_else(|| Fault::new(format!("{a} requires a value")))?
                        .clone();
                    match a {
                        "--branch" => out.branch = Some(value),
                        "--head" => out.head = Some(value),
                        _ => out.target = Some(value),
                    }
                }
                "--keep" => out.keep = true,
                "--resume" => out.resume = true,
                "--status" => out.status = true,
                "--retry-install" => out.retry_install = true,
                "--install-done" => out.install_done = true,
                _ => return Err(Fault::new(format!("unknown flag {a}"))),
            }
        } else if out.bead.is_empty()
            && a.bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"-._".contains(&c))
        {
            out.bead = a.into();
        } else {
            return Err(Fault::new(format!("unexpected argument {a}")));
        }
        i += 1;
    }
    if (out.retry_install || out.install_done)
        && (!out.resume || out.retry_install && out.install_done)
    {
        return Err(Fault::new("--retry-install or --install-done requires --resume; choose one after inspecting interrupted install effects"));
    }
    if out.bead.is_empty()
        || (out.resume && out.status)
        || ((out.resume || out.status)
            && (out.branch.is_some() || out.head.is_some() || out.target.is_some() || out.keep))
        || (!out.resume && !out.status && out.branch.is_none())
    {
        return Err(Fault::new("use BEAD --branch BRANCH [--head SHA] [--target BRANCH] [--keep], or BEAD --resume|--status"));
    }
    Ok(out)
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Policy {
    strategy: String,
    check: Vec<String>,
    install: Vec<String>,
    remote: String,
    target: String,
}
impl Policy {
    fn parse(text: &str) -> Result<Self> {
        let doc = text
            .parse::<toml_edit::DocumentMut>()
            .map_err(|e| Fault::new(format!("land policy: {e}")))?;
        for (key, _) in doc.iter() {
            if !["strategy", "check", "install", "remote", "target"].contains(&key) {
                return Err(Fault::new(format!("unknown land policy key {key}")));
            }
        }
        let string = |key: &str, default: Option<&str>| -> Result<String> {
            match doc.get(key) {
                Some(v) => v
                    .as_str()
                    .map(str::to_string)
                    .ok_or_else(|| Fault::new(format!("{key} must be a string"))),
                None => default
                    .map(str::to_string)
                    .ok_or_else(|| Fault::new(format!("missing {key}"))),
            }
        };
        let argv = |key: &str, required: bool| -> Result<Vec<String>> {
            let Some(v) = doc.get(key) else {
                return if required {
                    Err(Fault::new(format!("missing {key} argv")))
                } else {
                    Ok(Vec::new())
                };
            };
            let a = v
                .as_array()
                .ok_or_else(|| Fault::new(format!("{key} must be an argv array")))?;
            let values: Option<Vec<String>> = a
                .iter()
                .map(|v| v.as_str().filter(|s| !s.is_empty()).map(str::to_string))
                .collect();
            values
                .filter(|v| !v.is_empty())
                .ok_or_else(|| Fault::new(format!("{key} requires nonempty string argv")))
        };
        let p = Self {
            strategy: string("strategy", None)?,
            check: argv("check", true)?,
            install: argv("install", false)?,
            remote: string("remote", Some("origin"))?,
            target: string("target", Some("main"))?,
        };
        if !["no-ff", "ff-only"].contains(&p.strategy.as_str())
            || p.remote.starts_with('-')
            || p.remote.is_empty()
        {
            return Err(Fault::new("invalid merge strategy or remote"));
        }
        Ok(p)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Sentinel {
    pane: String,
    name: String,
    session: String,
    worktree: PathBuf,
    actor: String,
    brief: u128,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Receipt {
    bead: String,
    branch: String,
    head: String,
    before: String,
    integrated: Option<String>,
    policy: Policy,
    assisted_by: String,
    summary: String,
    sentinel: Option<Sentinel>,
    keep: bool,
    renumbered: Vec<(u64, u64)>,
    checked: bool,
    gate: String,
    install: String,
    phase: String,
    failure: Option<Value>,
}
impl Receipt {
    fn integrated_head(&self) -> Result<&str> {
        self.integrated.as_deref().filter(|head| !head.is_empty())
            .ok_or_else(|| Fault::new("landing receipt is missing the integrated SHA; inspect the receipt before resuming"))
    }
    fn marker(&self) -> String {
        format!("vega-land:{}:{}", self.bead, self.head)
    }
}

/// Only services with separate owners are abstracted; Git in fixtures is real.
trait Services {
    fn authority(&mut self, dir: &Path, target: &str) -> Result<()>;
    fn completed(&mut self, dir: &Path, r: &Receipt) -> Result<()>;
    fn validate(&mut self, dir: &Path, r: &Receipt) -> Result<()>;
    fn evidence(
        &mut self,
        dir: &Path,
        bead: &str,
        branch: &str,
        head: &str,
    ) -> Result<(String, Option<Sentinel>)>;
    fn gate(&mut self, dir: &Path, target: &str, head: &str, start: bool) -> Result<String>;
    /// The calling harness for the Assisted-by trailer.
    fn assisted_by(&mut self) -> Result<String>;
    fn close(&mut self, dir: &Path, r: &Receipt) -> Result<()>;
    fn pane(&mut self, dir: &Path, r: &Receipt) -> Result<()>;
    fn remove(&mut self, dir: &Path, r: &Receipt) -> Result<()>;
}

fn command(dir: &Path, program: &str, args: &[&str]) -> Result<String> {
    let out = Command::new(program)
        .args(args)
        .current_dir(dir)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_MERGE_AUTOEDIT", "no")
        .stdin(Stdio::null())
        .output()
        .map_err(|e| Fault::new(format!("cannot start {program}: {e}")))?;
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr);
    if !stderr.is_empty() {
        eprint!("{stderr}");
    }
    if out.status.success() {
        Ok(stdout)
    } else {
        Err(Fault {
            message: format!(
                "{program} {} failed ({})",
                args.first().unwrap_or(&""),
                out.status
            ),
            status: out.status.code(),
            output: format!("{stdout}{stderr}"),
        })
    }
}
fn git(dir: &Path, args: &[&str]) -> Result<String> {
    command(dir, "git", args).map(|s| s.trim_end().to_string())
}
fn argv(dir: &Path, args: &[String]) -> Result<()> {
    command(
        dir,
        &args[0],
        &args[1..].iter().map(String::as_str).collect::<Vec<_>>(),
    )
    .map(|_| ())
}
fn sha(dir: &Path, rev: &str) -> Result<String> {
    git(
        dir,
        &["rev-parse", "--verify", &format!("{rev}^{{commit}}")],
    )
}
fn ancestor(dir: &Path, a: &str, b: &str) -> Result<bool> {
    match git(dir, &["merge-base", "--is-ancestor", a, b]) {
        Ok(_) => Ok(true),
        Err(e) if e.status == Some(1) => Ok(false),
        Err(e) => Err(e),
    }
}
fn clean(dir: &Path) -> Result<()> {
    if git(dir, &["status", "--porcelain", "--untracked-files=all"])?.is_empty() {
        Ok(())
    } else {
        Err(Fault::new(
            "checkout is dirty; preserve changes and resolve before resume",
        ))
    }
}
fn state_path(dir: &Path, name: &str) -> Result<PathBuf> {
    Ok(PathBuf::from(git(
        dir,
        &["rev-parse", "--path-format=absolute", "--git-path", name],
    )?))
}
fn no_operation(dir: &Path) -> Result<()> {
    for name in [
        "MERGE_HEAD",
        "rebase-merge",
        "rebase-apply",
        "CHERRY_PICK_HEAD",
        "REVERT_HEAD",
    ] {
        if state_path(dir, name)?.exists() {
            return Err(Fault::new(format!("active {name}; inspect before landing")));
        }
    }
    Ok(())
}
fn current(dir: &Path, r: &Receipt) -> Result<()> {
    if git(dir, &["symbolic-ref", "--short", "HEAD"])? != r.policy.target
        || sha(dir, "HEAD")? != *r.integrated.as_ref().unwrap_or(&r.before)
    {
        return Err(Fault::new(
            "target HEAD changed; checks/gate invalidated; UAC reconciliation required",
        ));
    }
    clean(dir)?;
    no_operation(dir)
}
fn save(path: &Path, r: &Receipt) -> Result<()> {
    let tmp = path.with_extension("tmp");
    let mut f = File::create(&tmp)?;
    f.write_all(&serde_json::to_vec_pretty(r).map_err(|e| Fault::new(e.to_string()))?)?;
    f.sync_all()?;
    fs::rename(tmp, path)?;
    File::open(path.parent().unwrap())?.sync_all()?;
    Ok(())
}
fn read(path: &Path) -> Result<Receipt> {
    serde_json::from_slice(&fs::read(path)?).map_err(|e| Fault::new(format!("receipt: {e}")))
}
fn begin(dir: &Path, a: &Args, svc: &mut impl Services) -> Result<Receipt> {
    clean(dir)?;
    no_operation(dir)?;
    let before = sha(dir, "HEAD")?;
    let mut policy = Policy::parse(&git(dir, &["show", &format!("{before}:.vega/land.toml")])?)?;
    if let Some(t) = &a.target {
        policy.target = t.clone();
    }
    svc.authority(dir, &policy.target)?;
    git(
        dir,
        &["check-ref-format", &format!("refs/heads/{}", policy.target)],
    )?;
    if git(dir, &["symbolic-ref", "--short", "HEAD"])? != policy.target {
        return Err(Fault::new(
            "run land from the configured target branch in the main checkout",
        ));
    }
    let branch = a.branch.as_ref().unwrap().clone();
    git(dir, &["check-ref-format", &format!("refs/heads/{branch}")])?;
    if branch == policy.target {
        return Err(Fault::new("source and target branches must differ"));
    }
    let head = sha(dir, &format!("refs/heads/{branch}"))?;
    if a.head.as_ref().is_some_and(|s| s != &head) {
        return Err(Fault::new(
            "--head must equal the full source branch tip SHA",
        ));
    }
    fetch_target(dir, &policy)?;
    if remote_head(dir, &policy)? != before {
        return Err(Fault::new(
            "local target differs from remote; reconcile explicitly before landing",
        ));
    }
    git(dir, &["merge-base", &before, &head])?;
    if policy.strategy == "ff-only"
        && !ancestor(dir, &before, &head)?
        && !ancestor(dir, &head, &before)?
    {
        return Err(Fault::new(
            "source and target diverged under ff-only policy; reconcile before starting a landing",
        ));
    }
    let (summary, sentinel) = svc.evidence(dir, &a.bead, &branch, &head)?;
    let gate = svc.gate(dir, &before, &head, false)?;
    let assisted_by = svc.assisted_by()?;
    Ok(Receipt {
        bead: a.bead.clone(),
        branch,
        head,
        before,
        integrated: None,
        policy,
        assisted_by,
        summary,
        sentinel,
        keep: a.keep,
        renumbered: Vec::new(),
        checked: false,
        gate,
        install: "pending".into(),
        phase: "merge".into(),
        failure: None,
    })
}
fn fetch_target(dir: &Path, p: &Policy) -> Result<String> {
    git(
        dir,
        &["check-ref-format", &format!("refs/heads/{}", p.target)],
    )?;
    git(
        dir,
        &["fetch", &p.remote, &format!("refs/heads/{}", p.target)],
    )?;
    sha(dir, "FETCH_HEAD")
}
fn print_residue(rows: &[(String, String)]) {
    println!("residue[{}]{{kind,detail}}:", rows.len());
    for (kind, text) in rows {
        println!("  {},{}", cell(kind), cell(text));
    }
}
fn remote_head(dir: &Path, p: &Policy) -> Result<String> {
    remote_head_using(p, |args| git(dir, args))
}
fn remote_head_using(p: &Policy, mut run: impl FnMut(&[&str]) -> Result<String>) -> Result<String> {
    let text = run(&[
        "ls-remote",
        "--exit-code",
        &p.remote,
        &format!("refs/heads/{}", p.target),
    ])?;
    let rows: Vec<_> = text.lines().collect();
    if rows.len() != 1 {
        return Err(Fault::new("remote target is unknown or ambiguous"));
    }
    rows[0]
        .split_whitespace()
        .next()
        .map(str::to_string)
        .ok_or_else(|| Fault::new("remote target missing SHA"))
}
fn reconcile_merge(dir: &Path, path: &Path, r: &mut Receipt) -> Result<()> {
    if r.integrated.is_some() {
        return current(dir, r);
    }
    let head = sha(dir, "HEAD")?;
    if head != r.before {
        clean(dir)?;
        no_operation(dir)?;
        let parents = git(dir, &["show", "-s", "--format=%P", &head])?;
        let expected = format!("{} {}", r.before, r.head);
        if (r.policy.strategy == "ff-only" && head == r.head) || parents == expected {
            r.integrated = Some(head);
            return Ok(());
        }
        return Err(Fault::new(
            "unrecognized HEAD after merge intent; inspect receipt and git history",
        ));
    }
    let merge = state_path(dir, "MERGE_HEAD")?;
    if merge.exists() {
        if fs::read_to_string(merge)?.trim() != r.head {
            return Err(Fault::new("MERGE_HEAD does not match landing"));
        }
        if r.renumbered.is_empty() {
            return Err(Fault::new("interrupted merge conflict; inspect and resolve it, commit with Assisted-by, then resume"));
        }
        return changelog::resolve(dir, path, r);
    }
    clean(dir)?;
    no_operation(dir)?;
    if sha(dir, &format!("refs/heads/{}", r.branch))? != r.head {
        return Err(Fault::new("source branch moved since landing began"));
    }
    if ancestor(dir, &r.head, &r.before)? {
        r.integrated = Some(r.before.clone());
        return Ok(());
    }
    let outcome = if r.policy.strategy == "ff-only" {
        git(dir, &["merge", "--ff-only", &r.head])
    } else {
        git(
            dir,
            &[
                "merge",
                "--no-ff",
                "--no-edit",
                &r.head,
                "-m",
                &format!("Merge {} ({})", r.branch, r.bead),
                "-m",
                &format!("Assisted-by: {}", r.assisted_by),
            ],
        )
    };
    match outcome {
        Ok(_) => {
            r.integrated = Some(sha(dir, "HEAD")?);
            current(dir, r)
        }
        Err(e) => {
            if state_path(dir, "MERGE_HEAD")?.exists() {
                changelog::resolve(dir, path, r).map_err(|mut conflict| {
                    if conflict.status.is_none() {
                        conflict.status = e.status;
                    }
                    if conflict.output.is_empty() {
                        conflict.output = e.output;
                    }
                    conflict
                })
            } else {
                Err(e)
            }
        }
    }
}
fn ssh_timeout(e: &Fault) -> bool {
    let t = e.output.to_ascii_lowercase();
    (t.contains("github.com") || t.contains("ssh:"))
        && (t.contains("connection timed out") || t.contains("operation timed out"))
}
fn push(dir: &Path, r: &Receipt) -> Result<()> {
    push_using(r, |args| git(dir, args))
}
fn push_using(r: &Receipt, mut run: impl FnMut(&[&str]) -> Result<String>) -> Result<()> {
    let head = r.integrated_head()?;
    for attempt in 0..3 {
        let remote = remote_head_using(&r.policy, &mut run)?;
        if remote == head {
            return Ok(());
        }
        if remote != r.before {
            return Err(Fault::new(
                "remote target moved; do not overwrite it; reconcile with UAC",
            ));
        }
        match run(&[
            "push",
            &r.policy.remote,
            &format!("{head}:refs/heads/{}", r.policy.target),
        ]) {
            Ok(_) => {
                if remote_head_using(&r.policy, &mut run)? == head {
                    return Ok(());
                }
                return Err(Fault::new(
                    "push returned success but remote SHA does not match",
                ));
            }
            Err(e) => {
                match remote_head_using(&r.policy, &mut run) {
                    Ok(remote) if remote == head => return Ok(()),
                    Ok(_) => {}
                    Err(_) => return Err(e), // Preserve the push failure; remote state is unknown.
                }
                if !ssh_timeout(&e) || attempt == 2 {
                    return Err(e);
                }
                eprintln!("land: GitHub SSH timeout; retry {} of 3", attempt + 2);
            }
        }
    }
    unreachable!()
}
fn step(dir: &Path, path: &Path, r: &mut Receipt, svc: &mut impl Services) -> Result<()> {
    svc.authority(dir, &r.policy.target)?;
    if r.phase == "done" {
        let integrated = r.integrated_head()?;
        current(dir, r)?;
        if remote_head(dir, &r.policy)? != integrated {
            return Err(Fault::new(
                "completed landing remote changed; inspect without replaying phases",
            ));
        }
        return svc.completed(dir, r);
    }
    svc.validate(dir, r)?;
    if git(dir, &["symbolic-ref", "--short", "HEAD"])? != r.policy.target {
        return Err(Fault::new("target branch changed"));
    }
    reconcile_merge(dir, path, r)?;
    r.phase = "check".into();
    save(path, r)?;
    current(dir, r)?;
    if !r.checked {
        argv(dir, &r.policy.check)?;
        current(dir, r)?;
        r.checked = true;
        save(path, r)?;
    }
    r.phase = "gate".into();
    save(path, r)?;
    r.gate = svc.gate(dir, &r.before, r.integrated.as_ref().unwrap(), true)?;
    current(dir, r)?;
    r.phase = "push".into();
    save(path, r)?;
    svc.validate(dir, r)?;
    push(dir, r)?;
    current(dir, r)?;
    r.phase = "install".into();
    save(path, r)?;
    if r.install == "started" {
        return Err(Fault::new("install completion uncertain; inspect effects, then resume with --retry-install or --install-done"));
    }
    if r.install != "done" && r.install != "attested" {
        if !r.policy.install.is_empty() {
            r.install = "started".into();
            save(path, r)?;
            if let Err(e) = argv(dir, &r.policy.install) {
                r.install = "pending".into();
                save(path, r)?;
                return Err(e);
            }
            current(dir, r)?;
        }
        r.install = "done".into();
        save(path, r)?;
    }
    r.phase = "close".into();
    save(path, r)?;
    svc.close(dir, r)?;
    current(dir, r)?;
    if !r.keep && r.sentinel.is_some() {
        r.phase = "pane".into();
        save(path, r)?;
        svc.pane(dir, r)?;
        r.phase = "worktree".into();
        save(path, r)?;
        svc.remove(dir, r)?;
    }
    r.phase = "done".into();
    r.failure = None;
    save(path, r)
}

fn residue(dir: &Path, r: Option<&Receipt>) -> Vec<(String, String)> {
    let mut rows = Vec::new();
    for (kind, args) in [
        ("stash", vec!["stash", "list", "--format=%gd %s"]),
        (
            "unmerged_branch",
            vec!["branch", "--no-merged", "HEAD", "--format=%(refname:short)"],
        ),
        (
            "dirty_path",
            vec!["status", "--porcelain", "--untracked-files=all"],
        ),
    ] {
        match git(dir, &args) {
            Ok(t) => rows.extend(t.lines().map(|l| (kind.into(), l.to_string()))),
            Err(e) => rows.push(("unknown".into(), format!("{kind}: {}", e.message))),
        }
    }
    match git(dir, &["worktree", "list", "--porcelain"]) {
        Ok(list) => {
            for path in list.lines().filter_map(|l| l.strip_prefix("worktree ")) {
                if Path::new(path) != dir
                    && r.and_then(|r| r.sentinel.as_ref())
                        .is_none_or(|s| s.worktree != Path::new(path))
                {
                    rows.push((
                        "worktree".into(),
                        format!("{path}: retained; ownership not inferred from directory"),
                    ));
                }
            }
        }
        Err(e) => rows.push((
            "unknown".into(),
            format!("worktree inventory: {}", e.message),
        )),
    }
    if let Some(r) = r {
        if let Some(s) = &r.sentinel {
            rows.push(("unknown".into(), "child processes: mux owns lifecycle accounting; pane closure alone does not prove process exit".into()));
            if s.worktree.exists() {
                rows.push((
                    "worktree".into(),
                    format!(
                        "{}: retained or cleanup pending; UAC owns next action",
                        s.worktree.display()
                    ),
                ));
            }
            // Pane existence is owned by mux; do not infer termination from worktree removal.
            if r.keep || r.phase != "done" {
                rows.push((
                    "pane".into(),
                    format!("{}: retained or cleanup unverified", s.pane),
                ));
            }
        } else {
            rows.push((
                "unknown".into(),
                "sentinel ownership unavailable; pane/worktree retained".into(),
            ));
        }
        if let Some(e) = &r.failure {
            rows.push((
                "failed_phase".into(),
                format!("{}: {}", r.phase, e["message"]),
            ));
        }
    }
    rows
}
fn display(dir: &Path, path: &Path, r: Option<&Receipt>) {
    println!("receipt: {}", cell(&path.display().to_string()));
    if let Some(r) = r {
        println!(
            "bead: {}\nhead: {}\nintegrated: {}\nphase: {}",
            cell(&r.bead),
            r.head,
            r.integrated.as_deref().unwrap_or("pending"),
            r.phase
        );
        println!(
            "check: {}\ngate: {}\ninstall: {}",
            if r.checked { "exit 0" } else { "pending" },
            cell(&r.gate),
            r.install
        );
        if let Some(e) = &r.failure {
            println!(
                "failure: {}\nchild_exit: {}",
                cell(e["message"].as_str().unwrap_or("unknown")),
                e["exit"]
            );
            let output = e["output"].as_str().unwrap_or("");
            let preview: String = output.chars().take(2000).collect();
            if !preview.is_empty() {
                println!("failed_output: {}", cell(&preview));
            }
            if preview.len() < output.len() {
                println!(
                    "help: last failure output truncated; read output in receipt {}",
                    cell(&path.display().to_string())
                );
            }
        }
    }
    let rows = residue(dir, r);
    print_residue(&rows);
    println!("help: preview merged residue cleanup with vega uac prune --dry-run");
    if let Some(r) = r.filter(|r| r.phase != "done") {
        if r.install == "started" {
            println!("help: inspect installation effects; then vega uac land {} --resume --retry-install OR vega uac land {} --resume --install-done", r.bead, r.bead);
        } else {
            println!(
                "help: inspect failure and residue; then vega uac land {} --resume",
                r.bead
            );
        }
    }
}

fn recover_install(dir: &Path, r: &mut Receipt, attested: bool) -> Result<()> {
    let integrated = r.integrated_head()?;
    current(dir, r)?;
    if r.install != "started" || remote_head(dir, &r.policy)? != integrated {
        return Err(Fault::new(
            "install recovery requires an interrupted install at the verified pushed head",
        ));
    }
    r.install = if attested { "attested" } else { "pending" }.into();
    Ok(())
}

pub fn main(args: &[String]) -> ExitCode {
    if args.iter().any(|a| a == "--help" || a == "-h") {
        print!("{HELP}");
        return ExitCode::SUCCESS;
    }
    let a = match parse(args) {
        Ok(a) => a,
        Err(e) => {
            print_failure(&Failure::new("usage", e.message, HELP));
            return ExitCode::from(2);
        }
    };
    match run(&a, &mut live::Live) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            print_failure(&Failure::new("land", e.message, "vega uac land --help"));
            ExitCode::from(1)
        }
    }
}
fn runtime_root(dir: &Path) -> Result<PathBuf> {
    let common = git(
        dir,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )?;
    let state = env::var_os("XDG_STATE_HOME")
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            env::var_os("HOME")
                .filter(|s| !s.is_empty())
                .map(|h| PathBuf::from(h).join(".local/state"))
        })
        .ok_or_else(|| Fault::new("state directory unavailable"))?;
    if !state.is_absolute() {
        return Err(Fault::new("state directory must be absolute"));
    }
    let root = state.join("vega/uac/land").join(vega_gate::sha256(
        Path::new(&common)
            .canonicalize()?
            .to_string_lossy()
            .as_bytes(),
    ));
    Ok(root)
}
fn run(a: &Args, svc: &mut impl Services) -> Result<()> {
    let cwd = env::current_dir()?;
    let dir = PathBuf::from(git(&cwd, &["rev-parse", "--show-toplevel"])?);
    let root = runtime_root(&dir)?;
    fs::create_dir_all(&root)?;
    let lock = File::options()
        .create(true)
        .truncate(false)
        .write(true)
        .open(root.join("lock"))?;
    lock.try_lock()
        .map_err(|e| Fault::new(format!("another landing holds the repository lock: {e}")))?;
    let path = root.join(format!("{}.json", a.bead));
    if a.status {
        let r = read(&path)?;
        display(&dir, &path, Some(&r));
        return Ok(());
    }
    let mut r = if a.resume {
        read(&path)?
    } else {
        if path.exists() {
            return Err(Fault::new(
                "landing receipt already exists; use --status or --resume",
            ));
        }
        for item in fs::read_dir(&root)? {
            let p = item?.path();
            if p.extension().is_some_and(|s| s == "json") && read(&p)?.phase != "done" {
                return Err(Fault::new(format!(
                    "unfinished landing {}; resume it first",
                    p.display()
                )));
            }
        }
        match begin(&dir, a, svc) {
            Ok(r) => {
                save(&path, &r)?;
                r
            }
            Err(e) => {
                display(&dir, &path, None);
                return Err(e);
            }
        }
    };
    if a.retry_install || a.install_done {
        svc.authority(&dir, &r.policy.target)?;
        recover_install(&dir, &mut r, a.install_done)?;
        save(&path, &r)?;
    }
    let result = step(&dir, &path, &mut r, svc);
    if let Err(e) = &result {
        r.failure = Some(json!({"message": e.message, "exit": e.status, "output": e.output}));
        if r.phase != "done" {
            if sha(&dir, "HEAD").ok().as_deref() != r.integrated.as_deref()
                && r.integrated.is_some()
            {
                r.checked = false;
            }
            save(&path, &r)?;
        }
    }
    display(&dir, &path, Some(&r));
    result
}
