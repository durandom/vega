//! Cleanup using Worktrunk integration evidence; authority and worktree safety stay local.
use super::*;

const HELP: &str = include_str!("../../help/prune.md");

trait Cleanup {
    fn authority(&mut self, dir: &Path, target: &str) -> Result<()>;
    fn paneless(&mut self, path: &Path) -> Result<()>;
    fn remove(&mut self, dir: &Path, branch: &str, tree: Option<&Tree>, target: &str)
        -> Result<()>;
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
    fn remove(
        &mut self,
        dir: &Path,
        branch: &str,
        tree: Option<&Tree>,
        target: &str,
    ) -> Result<()> {
        let output = worktrunk(dir, target, &remove_args(branch, tree)?)?;
        removal_outcome(&output, branch)
    }
}
fn remove_args<'a>(branch: &'a str, tree: Option<&'a Tree>) -> Result<Vec<&'a str>> {
    let name = if let Some(tree) = tree {
        if !tree.path.is_absolute() {
            return Err(Fault::new("worktree path is not absolute; retain it"));
        }
        tree.path
            .to_str()
            .ok_or_else(|| Fault::new("worktree path is not UTF-8; retain it"))?
    } else {
        branch
    };
    Ok(vec!["remove", "--foreground", "--format=json", "--", name])
}
// Scope target selection to this subprocess; never rewrite repository/user configuration.
fn worktrunk(dir: &Path, target: &str, args: &[&str]) -> Result<String> {
    let count = env::var("GIT_CONFIG_COUNT")
        .unwrap_or_else(|_| "0".into())
        .parse::<usize>()
        .map_err(|_| Fault::new("invalid GIT_CONFIG_COUNT"))?;
    let mut cmd = Command::new("wt");
    cmd.args(args)
        .current_dir(dir)
        .stdin(Stdio::null())
        .env("GIT_TERMINAL_PROMPT", "0");
    cmd.env(
        format!("GIT_CONFIG_KEY_{count}"),
        "worktrunk.default-branch",
    )
    .env(format!("GIT_CONFIG_VALUE_{count}"), target);
    let out = cmd
        .env("GIT_CONFIG_COUNT", (count + 1).to_string())
        .output()?;
    if !out.status.success() {
        return Err(Fault::new(
            "Worktrunk refused cleanup; inspect the branch and hook approvals",
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}
fn removal_outcome(output: &str, branch: &str) -> Result<()> {
    let row: Value = serde_json::from_str(output)
        .map_err(|_| Fault::new("Worktrunk removal result unavailable; inspect residue"))?;
    // Older wt emits a single-element array with branch_deleted; newer versions
    // expose a more precise branch_outcome. Unknown/retained results fail closed.
    let row = match row.as_array() {
        Some(rows) if rows.len() == 1 => &rows[0],
        Some(_) => return Err(Fault::new("unexpected Worktrunk removal rows")),
        None => &row,
    };
    if row["branch"] != branch {
        return Err(Fault::new(
            "Worktrunk removal branch mismatch; inspect residue",
        ));
    }
    if row.get("branch_outcome").is_none() && row["branch_deleted"] == true {
        return Ok(());
    }
    match row["branch_outcome"].as_str() {
        Some("deleted") => Ok(()),
        Some("retained_unmerged") => Err(Fault::new("Worktrunk retained unmerged branch")),
        _ => Err(Fault::new(
            "Worktrunk retained branch or removal incomplete; inspect residue",
        )),
    }
}
fn integrated(dir: &Path, branch: &str, head: &str, target: &str, frozen: &str) -> Result<bool> {
    if branch_tip(dir, target)? != frozen {
        return Err(Fault::new(
            "local target differs from fetched target; fast-forward it before pruning",
        ));
    }
    // Worktrunk also considers the target's upstream. It must add no commits
    // beyond the frozen policy target; otherwise retain rather than broaden it.
    let upstream_ref = git(
        dir,
        &[
            "for-each-ref",
            "--format=%(upstream)",
            "--",
            &format!("refs/heads/{target}"),
        ],
    )?;
    if !upstream_ref.is_empty() {
        let upstream = sha(dir, &upstream_ref)?;
        if !ancestor(dir, &upstream, frozen)? {
            return Err(Fault::new(
                "target upstream differs from fetched target; reconcile it before pruning",
            ));
        }
    }
    if ancestor(dir, head, frozen)? {
        return Ok(true);
    }
    let output = worktrunk(
        dir,
        target,
        &[
            "list",
            "--branches",
            "--format=json",
            "--config-set",
            "list.json-schema=2",
            "--config-set",
            "list.full=false",
        ],
    )?;
    let list: Value = serde_json::from_str(&output)
        .map_err(|_| Fault::new("Worktrunk integration inventory unavailable; retain branch"))?;
    if list["schema"] != 2 || list["repo"]["default_branch"] != target {
        return Err(Fault::new(
            "Worktrunk integration target unavailable; retain branch",
        ));
    }
    let items = list["items"]
        .as_array()
        .ok_or_else(|| Fault::new("Worktrunk integration rows unavailable; retain branch"))?;
    Ok(items.iter().any(|row| {
        row["branch"] == branch
            && row["head"]["sha"] == head
            && row.get("remote").is_none()
            && row["default_branch"]["integration"]["reason"]
                .as_str()
                .is_some_and(|s| !s.is_empty())
    }))
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
        match integrated(dir, branch, &head, &p.target, &target) {
            Ok(true) => {}
            Ok(false) => {
                report.residue.push((
                    "unmerged_branch".into(),
                    format!("{branch}: not integrated into target"),
                ));
                continue;
            }
            Err(e) => {
                report
                    .residue
                    .push(("retained_branch".into(), format!("{branch}: {}", e.message)));
                continue;
            }
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
                    if !integrated(dir, branch, &head, &p.target, &target)? {
                        return Err(Fault::new(
                            "integration changed; retain branch and worktree",
                        ));
                    }
                    mutation_started = true;
                    svc.remove(dir, branch, Some(tree), &p.target)?;
                    if tree.path.exists() || trees(dir)?.iter().any(|t| t.path == tree.path) {
                        return Err(Fault::new("worktree removal incomplete; retain branch"));
                    }
                }
                report
                    .actions
                    .push(("worktree".into(), tree.path.display().to_string()));
            }
            if !dry && checked.is_empty() {
                svc.authority(dir, &p.target)?;
                if branch_tip(dir, branch)? != head
                    || !integrated(dir, branch, &head, &p.target, &target)?
                    || trees(dir)?
                        .iter()
                        .any(|t| t.branch.as_deref() == Some(branch))
                {
                    return Err(Fault::new("branch changed or checked out; retain it"));
                }
                mutation_started = true;
                svc.remove(dir, branch, None, &p.target)?;
            }
            if !dry && branch_tip(dir, branch).is_ok() {
                return Err(Fault::new("branch deletion incomplete; retain residue"));
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
