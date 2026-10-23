//! Conservative ancestry-only cleanup. Git fixtures are real; mux and wt stay owned services.
use super::*;

const HELP: &str = include_str!("../../help/prune.md");

trait Cleanup {
    fn authority(&mut self, dir: &Path, target: &str) -> Result<()>;
    fn paneless(&mut self, path: &Path) -> Result<()>;
    fn remove(&mut self, dir: &Path, tree: &Tree) -> Result<()>;
}
struct Live;
impl Cleanup for Live {
    fn authority(&mut self, dir: &Path, target: &str) -> Result<()> {
        live::Live.authority(dir, target)
    }
    fn paneless(&mut self, path: &Path) -> Result<()> {
        let inventory = vega_herdr::herdr(&["pane", "list"]).map_err(|e| Fault::new(e.message))?;
        let panes = inventory["result"]["panes"]
            .as_array()
            .ok_or_else(|| Fault::new("pane inventory unavailable; retain worktree"))?;
        // Agent cwd may be more specific than the pane's shell cwd. Check both,
        // including idle/done agents: prune never closes a pane.
        check_paneless(panes, &live::agents()?, path)
    }
    fn remove(&mut self, dir: &Path, tree: &Tree) -> Result<()> {
        command(dir, "wt", &remove_args(tree)?)?;
        Ok(())
    }
}
fn remove_args(tree: &Tree) -> Result<[&str; 5]> {
    if !tree.path.is_absolute() {
        return Err(Fault::new("worktree path is not absolute; retain it"));
    }
    let path = tree
        .path
        .to_str()
        .ok_or_else(|| Fault::new("worktree path is not UTF-8; retain it"))?;
    Ok(["remove", "--foreground", "--no-delete-branch", "--", path])
}
fn check_paneless(panes: &[Value], agents: &[Value], path: &Path) -> Result<()> {
    // Missing cwd means absence cannot be established. Fail closed.
    if panes
        .iter()
        .chain(agents)
        .any(|p| p["cwd"].as_str().is_none_or(str::is_empty))
    {
        return Err(Fault::new("pane/agent cwd unknown; retain worktree"));
    }
    let occupied = live::occupants(panes, path, None, None);
    let agents = live::occupants(agents, path, None, None);
    if occupied.is_empty() && agents.is_empty() {
        Ok(())
    } else {
        Err(Fault::new("live pane or agent; retain worktree"))
    }
}

#[derive(Debug, PartialEq)]
struct Tree {
    path: PathBuf,
    branch: Option<String>,
    locked: bool,
}
fn trees(dir: &Path) -> Result<Vec<Tree>> {
    let text = git(dir, &["worktree", "list", "--porcelain", "-z"])?;
    let mut out = Vec::new();
    let mut current: Option<Tree> = None;
    for field in text.split('\0') {
        if let Some(path) = field.strip_prefix("worktree ") {
            if let Some(t) = current.take() {
                out.push(t);
            }
            current = Some(Tree {
                path: path.into(),
                branch: None,
                locked: false,
            });
        } else if let Some(t) = current.as_mut() {
            if let Some(branch) = field.strip_prefix("branch refs/heads/") {
                t.branch = Some(branch.into());
            } else if field == "locked" || field.starts_with("locked ") {
                t.locked = true;
            }
        }
    }
    if let Some(t) = current {
        out.push(t);
    }
    Ok(out)
}
fn policy(dir: &Path) -> Result<Policy> {
    // Read only committed policy; distinguish missing from invalid/unreadable.
    let path = ".vega/land.toml";
    if git(dir, &["ls-tree", "HEAD", "--", path])?.is_empty() {
        return Policy::parse("strategy = 'no-ff'\ncheck = ['true']\n");
    }
    Policy::parse(&git(dir, &["show", &format!("HEAD:{path}")])?)
}
#[derive(Default, Debug)]
struct Report {
    actions: Vec<(String, String)>,
    residue: Vec<(String, String)>,
    failed: bool,
}
fn branch_tip(dir: &Path, branch: &str) -> Result<String> {
    sha(dir, &format!("refs/heads/{branch}"))
}
// Capture Git's refusal rather than forwarding its force-delete suggestion.
fn delete_branch(dir: &Path, branch: &str) -> Result<()> {
    let out = Command::new("git")
        .args(["branch", "-d", "--", branch])
        .current_dir(dir)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .output()?;
    if out.status.success() {
        Ok(())
    } else {
        Err(Fault {
            message: "safe branch deletion refused; inspect residue and reconcile the local target/upstream".into(),
            status: out.status.code(),
            output: String::new(),
        })
    }
}
fn incomplete_failure() -> Failure {
    Failure::new(
        "prune_incomplete",
        "some cleanup actions failed; retained residue is listed above",
        "inspect residue and fast-forward the configured local target/upstream before retrying vega uac prune; never use -D",
    )
}
fn validate_tree(dir: &Path, branch: &str, head: &str, tree: &Tree) -> Result<()> {
    let current = trees(dir)?;
    let matches: Vec<_> = current
        .iter()
        .filter(|t| t.branch.as_deref() == Some(branch))
        .collect();
    if matches.len() != 1
        || matches[0] != tree
        || tree.locked
        || tree.path.canonicalize()? == dir.canonicalize()?
        || git(
            &tree.path,
            &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        )? != git(
            dir,
            &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        )?
        || branch_tip(dir, branch)? != head
        || sha(&tree.path, "HEAD")? != head
    {
        return Err(Fault::new(
            "checked-out worktree identity changed or locked; retain it",
        ));
    }
    clean(&tree.path)?;
    no_operation(&tree.path)
}
fn prune(dir: &Path, dry: bool, svc: &mut impl Cleanup) -> Result<Report> {
    let p = policy(dir)?;
    svc.authority(dir, &p.target)?;
    no_operation(dir)?;
    // Fetch a single explicit ref, then freeze FETCH_HEAD, never a stale tracking ref.
    let target = fetch_target(dir, &p)?;
    let branches = git(
        dir,
        &[
            "for-each-ref",
            "--sort=refname",
            "--format=%(refname:strip=2)",
            "refs/heads/",
        ],
    )?;
    let mut report = Report::default();
    for branch in branches.lines() {
        if branch == p.target {
            continue;
        }
        let head = branch_tip(dir, branch)?;
        if !ancestor(dir, &head, &target)? {
            report.residue.push((
                "unmerged_branch".into(),
                format!("{branch}: unmerged or squash-merged; ancestry required"),
            ));
            continue;
        }
        let checked = trees(dir)?;
        let checked: Vec<_> = checked
            .iter()
            .filter(|t| t.branch.as_deref() == Some(branch))
            .collect();
        let mut mutation_started = false;
        let outcome = (|| -> Result<()> {
            if checked.len() > 1 {
                return Err(Fault::new(
                    "checked out in multiple worktrees; retain branch",
                ));
            }
            if let Some(tree) = checked.first() {
                validate_tree(dir, branch, &head, tree)?;
                svc.paneless(&tree.path)?;
                if !dry {
                    // Reobserve immediately before delegating; wt still enforces cleanliness.
                    svc.authority(dir, &p.target)?;
                    validate_tree(dir, branch, &head, tree)?;
                    svc.paneless(&tree.path)?;
                    mutation_started = true;
                    svc.remove(dir, tree)?;
                    if tree.path.exists() || trees(dir)?.iter().any(|t| t.path == tree.path) {
                        return Err(Fault::new("worktree removal incomplete; retain branch"));
                    }
                }
                report
                    .actions
                    .push(("worktree".into(), tree.path.display().to_string()));
            }
            // Never let wt's squash equivalence policy decide branch deletion.
            if !dry {
                svc.authority(dir, &p.target)?;
                if branch_tip(dir, branch)? != head
                    || !ancestor(dir, &head, &target)?
                    || trees(dir)?
                        .iter()
                        .any(|t| t.branch.as_deref() == Some(branch))
                {
                    return Err(Fault::new("branch changed or checked out; retain it"));
                }
                mutation_started = true;
                delete_branch(dir, branch)?;
            }
            report.actions.push(("branch".into(), branch.into()));
            Ok(())
        })();
        if let Err(e) = outcome {
            // Expected safety exclusions are residue. A failed removal/deletion is also
            // an operational failure, not a claim that all planned work succeeded.
            if mutation_started {
                report.failed = true;
            }
            report
                .residue
                .push(("retained_branch".into(), format!("{branch}: {}", e.message)));
        }
    }
    for tree in trees(dir)? {
        if tree.branch.is_none() && tree.path != dir {
            report.residue.push((
                "worktree".into(),
                format!("{}: detached or bare; retained", tree.path.display()),
            ));
        }
    }
    Ok(report)
}
fn parse(args: &[String]) -> Result<bool> {
    match args {
        [] => Ok(false),
        [a] if a == "--dry-run" => Ok(true),
        _ => Err(Fault::new(
            "valid flags: --dry-run, --help; no positional arguments",
        )),
    }
}
pub fn main(args: &[String]) -> ExitCode {
    if args.iter().any(|a| a == "--help" || a == "-h") {
        print!("{HELP}");
        return ExitCode::SUCCESS;
    }
    let dry = match parse(args) {
        Ok(dry) => dry,
        Err(e) => {
            print_failure(&Failure::new("usage", e.message, HELP));
            return ExitCode::from(2);
        }
    };
    let run = || -> Result<Report> {
        let cwd = env::current_dir()?;
        let dir = PathBuf::from(git(&cwd, &["rev-parse", "--show-toplevel"])?);
        let root = runtime_root(&dir)?;
        locked_prune(&dir, dry, &mut Live, &root)
    };
    match run() {
        Ok(r) => display(r, dry),
        Err(e) => {
            print_failure(&Failure::new("prune", e.message, "vega uac prune --help"));
            ExitCode::from(1)
        }
    }
}
fn locked_prune(dir: &Path, dry: bool, svc: &mut impl Cleanup, root: &Path) -> Result<Report> {
    let p = policy(dir)?;
    svc.authority(dir, &p.target)?;
    fs::create_dir_all(root)?;
    let lock = File::options()
        .create(true)
        .truncate(false)
        .write(true)
        .open(root.join("lock"))?;
    lock.try_lock()
        .map_err(|e| Fault::new(format!("land/prune holds repository lock: {e}")))?;
    for item in fs::read_dir(root)? {
        let path = item?.path();
        if path.extension().is_some_and(|s| s == "json") && read(&path)?.phase != "done" {
            return Err(Fault::new("unfinished landing; resume it before pruning"));
        }
    }
    prune(dir, dry, svc)
}
fn display(r: Report, dry: bool) -> ExitCode {
    println!(
        "dry_run: {dry}\nactions[{}]{{kind,detail}}:",
        r.actions.len()
    );
    for (kind, detail) in r.actions {
        println!("  {},{}", cell(&kind), cell(&detail));
    }
    print_residue(&r.residue);
    if dry {
        println!("help: apply eligible cleanup with vega uac prune");
    }
    if r.failed {
        print_failure(&incomplete_failure());
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}

#[cfg(test)]
mod tests;
