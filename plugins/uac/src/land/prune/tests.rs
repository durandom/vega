use super::*;
use tempfile::TempDir;
struct Fixture {
    root: TempDir,
    repo: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let repo = root.path().join("repo");
        fs::create_dir(&repo).unwrap();
        git(&repo, &["init", "-q", "-b", "main"]).unwrap();
        for (key, value) in [
            ("user.name", "Fixture"),
            ("user.email", "fixture@example.invalid"),
            ("commit.gpgsign", "false"),
            ("core.hooksPath", "/dev/null"),
        ] {
            git(&repo, &["config", key, value]).unwrap();
        }
        fs::write(repo.join("base"), "base").unwrap();
        commit(&repo);
        let remote = root.path().join("remote.git");
        git(&repo, &["init", "--bare", "-q", remote.to_str().unwrap()]).unwrap();
        git(
            &repo,
            &["remote", "add", "origin", remote.to_str().unwrap()],
        )
        .unwrap();
        git(&repo, &["push", "-q", "origin", "main"]).unwrap();
        Self { root, repo }
    }
    fn branch(&self, name: &str) {
        git(&self.repo, &["branch", name]).unwrap();
    }
    fn tree(&self, name: &str) -> PathBuf {
        self.branch(name);
        let path = self.root.path().join(format!("tree {name}"));
        git(
            &self.repo,
            &["worktree", "add", "-q", path.to_str().unwrap(), name],
        )
        .unwrap();
        path.canonicalize().unwrap()
    }
    fn run(&self, dry: bool, svc: &mut Fake) -> Report {
        prune(&self.repo, dry, svc).unwrap()
    }
    fn exists(&self, branch: &str) -> bool {
        branch_tip(&self.repo, branch).is_ok()
    }
}
fn commit(dir: &Path) {
    git(dir, &["add", "."]).unwrap();
    git(
        dir,
        &[
            "commit",
            "-q",
            "-m",
            "fixture",
            "--trailer",
            "Assisted-by: codex",
        ],
    )
    .unwrap();
}
#[derive(Default)]
struct Fake {
    panes: Vec<Value>,
    agents: Vec<Value>,
    unavailable: bool,
    remove_fail: bool,
    removed: Vec<String>,
}
impl Cleanup for Fake {
    fn authority(&mut self, dir: &Path, target: &str) -> Result<()> {
        live::checkout_authority(dir, target)
    }
    fn paneless(&mut self, path: &Path) -> Result<()> {
        if self.unavailable {
            return Err(Fault::new("inventory unavailable"));
        }
        check_paneless(&self.panes, &self.agents, path)
    }
    fn remove(&mut self, dir: &Path, tree: &Tree) -> Result<()> {
        self.removed.push(tree.branch.clone().unwrap());
        if self.remove_fail {
            return Err(Fault {
                message: "wt failed".into(),
                status: Some(23),
                output: String::new(),
            });
        }
        // The owner is mocked, not Git: use real worktree lifecycle in disposable fixtures.
        git(dir, &["worktree", "remove", tree.path.to_str().unwrap()])?;
        Ok(())
    }
}
#[test]
fn merged_branches_and_paneless_trees_are_removed_idempotently_in_order() {
    let f = Fixture::new();
    f.branch("z-merged");
    f.branch("a-merged");
    let path = f.tree("m-tree");
    let mut svc = Fake::default();
    let r = f.run(false, &mut svc);
    assert!(!r.failed);
    assert!(r.residue.is_empty());
    assert_eq!(
        r.actions
            .iter()
            .filter(|(k, _)| k == "branch")
            .map(|(_, v)| v.as_str())
            .collect::<Vec<_>>(),
        ["a-merged", "m-tree", "z-merged"]
    );
    assert!(!path.exists());
    assert!(!f.exists("m-tree"));
    assert!(f.exists("main"));
    let r = f.run(false, &mut svc);
    assert!(r.actions.is_empty());
    assert!(r.residue.is_empty());
}
#[test]
fn dry_run_preserves_refs_trees_dirty_files_and_stashes() {
    let f = Fixture::new();
    f.branch("merged");
    let path = f.tree("tree");
    fs::write(f.repo.join("stash-file"), "save").unwrap();
    git(&f.repo, &["stash", "push", "-u", "-m", "keep"]).unwrap();
    let refs = git(&f.repo, &["show-ref"]).unwrap();
    let inventory = trees(&f.repo).unwrap();
    let mut svc = Fake::default();
    let r = f.run(true, &mut svc);
    assert_eq!(r.actions.len(), 3);
    assert!(svc.removed.is_empty());
    assert_eq!(git(&f.repo, &["show-ref"]).unwrap(), refs);
    assert_eq!(trees(&f.repo).unwrap(), inventory);
    assert!(path.exists());
    f.run(false, &mut svc);
    assert!(!git(&f.repo, &["stash", "list"]).unwrap().is_empty());
}
#[test]
fn unmerged_and_squash_merged_branches_and_trees_stay_residue() {
    let f = Fixture::new();
    let path = f.tree("squashed");
    fs::write(path.join("feature"), "feature").unwrap();
    commit(&path);
    git(&f.repo, &["merge", "--squash", "squashed"]).unwrap();
    git(
        &f.repo,
        &[
            "commit",
            "-q",
            "-m",
            "squash integration",
            "--trailer",
            "Assisted-by: codex",
        ],
    )
    .unwrap();
    git(&f.repo, &["push", "-q", "origin", "main"]).unwrap();
    git(&f.repo, &["switch", "-q", "-c", "unmerged"]).unwrap();
    fs::write(f.repo.join("unmerged"), "unique").unwrap();
    commit(&f.repo);
    git(&f.repo, &["switch", "-q", "main"]).unwrap();
    let r = f.run(false, &mut Fake::default());
    assert_eq!(r.residue.len(), 2);
    assert!(r.actions.is_empty());
    assert!(f.exists("unmerged"));
    assert!(f.exists("squashed"));
    assert!(path.exists());
}
#[test]
fn dirty_untracked_staged_locked_and_active_operation_trees_stay() {
    let f = Fixture::new();
    let dirty = f.tree("dirty");
    fs::write(dirty.join("base"), "modified").unwrap();
    let untracked = f.tree("untracked");
    fs::write(untracked.join("new"), "new").unwrap();
    let staged = f.tree("staged");
    fs::write(staged.join("base"), "staged").unwrap();
    git(&staged, &["add", "."]).unwrap();
    let locked = f.tree("locked");
    git(&f.repo, &["worktree", "lock", locked.to_str().unwrap()]).unwrap();
    let active = f.tree("active");
    fs::write(
        state_path(&active, "CHERRY_PICK_HEAD").unwrap(),
        sha(&active, "HEAD").unwrap(),
    )
    .unwrap();
    let r = f.run(false, &mut Fake::default());
    assert_eq!(r.residue.len(), 5);
    assert!(r.actions.is_empty());
    for path in [dirty, untracked, staged, locked, active] {
        assert!(path.exists());
    }
}
#[test]
fn live_pane_or_agent_in_descendant_and_unknown_inventory_retain_tree() {
    let f = Fixture::new();
    let path = f.tree("live");
    fs::create_dir(path.join("sub")).unwrap();
    for agent in [false, true] {
        let row = json!({"pane_id":"w1:p2", "cwd": path.join("sub")});
        let mut svc = Fake::default();
        if agent {
            svc.agents.push(row);
        } else {
            svc.panes.push(row);
        }
        assert_eq!(f.run(false, &mut svc).residue.len(), 1);
        assert!(path.exists());
        assert!(f.exists("live"));
    }
    let mut svc = Fake {
        unavailable: true,
        ..Default::default()
    };
    assert_eq!(f.run(false, &mut svc).residue.len(), 1);
    svc.unavailable = false;
    svc.panes.push(json!({"pane_id":"unknown"}));
    assert_eq!(f.run(false, &mut svc).residue.len(), 1);
}
#[test]
fn detached_and_multiple_checked_out_trees_are_retained() {
    let f = Fixture::new();
    let first = f.tree("multiple");
    let second = f.root.path().join("second");
    git(
        &f.repo,
        &[
            "worktree",
            "add",
            "--force",
            second.to_str().unwrap(),
            "multiple",
        ],
    )
    .unwrap();
    let detached = f.root.path().join("detached");
    git(
        &f.repo,
        &[
            "worktree",
            "add",
            "--detach",
            detached.to_str().unwrap(),
            "HEAD",
        ],
    )
    .unwrap();
    let r = f.run(false, &mut Fake::default());
    assert_eq!(r.residue.len(), 2);
    assert!(r.actions.is_empty());
    assert!(first.exists());
    assert!(second.exists());
    assert!(detached.exists());
}
#[test]
fn wt_failure_retains_branch_and_reports_operational_failure() {
    let f = Fixture::new();
    let path = f.tree("tree");
    let mut svc = Fake {
        remove_fail: true,
        ..Default::default()
    };
    let r = f.run(false, &mut svc);
    assert!(r.failed);
    assert!(r.actions.is_empty());
    assert_eq!(r.residue.len(), 1);
    assert!(path.exists());
    assert!(f.exists("tree"));
}
#[test]
fn policy_target_and_fetch_are_honored_and_invalid_policy_stops() {
    let f = Fixture::new();
    git(&f.repo, &["branch", "release"]).unwrap();
    git(&f.repo, &["switch", "release"]).unwrap();
    fs::create_dir(f.repo.join(".vega")).unwrap();
    fs::write(
        f.repo.join(".vega/land.toml"),
        "strategy='ff-only'\ncheck=['true']\ntarget='release'\nremote='upstream'\n",
    )
    .unwrap();
    commit(&f.repo);
    git(&f.repo, &["remote", "rename", "origin", "upstream"]).unwrap();
    git(&f.repo, &["push", "upstream", "release"]).unwrap();
    f.branch("merged");
    let r = f.run(false, &mut Fake::default());
    assert!(r.actions.iter().any(|(_, b)| b == "merged"));
    assert!(f.exists("release"));
    fs::write(f.repo.join(".vega/land.toml"), "unknown=true").unwrap();
    commit(&f.repo);
    assert!(prune(&f.repo, false, &mut Fake::default()).is_err());
}
#[test]
fn stale_remote_tracking_ref_does_not_prove_merging_and_fetch_failure_stops() {
    let f = Fixture::new();
    git(&f.repo, &["switch", "-c", "unmerged"]).unwrap();
    fs::write(f.repo.join("new"), "new").unwrap();
    commit(&f.repo);
    git(&f.repo, &["update-ref", "refs/remotes/origin/main", "HEAD"]).unwrap();
    git(&f.repo, &["switch", "main"]).unwrap();
    assert_eq!(f.run(false, &mut Fake::default()).residue.len(), 1);
    git(
        &f.repo,
        &[
            "remote",
            "set-url",
            "origin",
            "/nonexistent/vega-prune-remote",
        ],
    )
    .unwrap();
    assert!(prune(&f.repo, false, &mut Fake::default()).is_err());
    assert!(f.exists("unmerged"));
}
#[test]
fn linked_sentinel_checkout_is_not_authorized_and_usage_is_strict() {
    let f = Fixture::new();
    let path = f.tree("sentinel");
    assert!(prune(&path, false, &mut Fake::default()).is_err());
    for args in [
        vec!["--force"],
        vec!["branch"],
        vec!["--dry-run", "--dry-run"],
        vec!["--dryrun"],
    ] {
        assert!(parse(&args.into_iter().map(String::from).collect::<Vec<_>>()).is_err());
    }
    assert!(!parse(&[]).unwrap());
    assert!(parse(&["--dry-run".into()]).unwrap());
}

#[test]
fn fetched_remote_ahead_does_not_bypass_git_safe_deletion() {
    let f = Fixture::new();
    let old = sha(&f.repo, "HEAD").unwrap();
    git(&f.repo, &["switch", "-c", "remote-only"]).unwrap();
    fs::write(f.repo.join("remote-only"), "remote-only").unwrap();
    commit(&f.repo);
    git(&f.repo, &["push", "origin", "HEAD:main"]).unwrap();
    git(&f.repo, &["switch", "main"]).unwrap();
    assert_eq!(sha(&f.repo, "HEAD").unwrap(), old);
    let r = f.run(false, &mut Fake::default());
    assert!(r.failed); // git branch -d refuses: local target is still behind.
    assert!(r.actions.is_empty());
    assert_eq!(r.residue.len(), 1);
    assert!(f.exists("remote-only"));
    let output = Command::new(env::current_exe().unwrap())
        .args([
            "--exact",
            "land::prune::tests::emit_refusal_in_child",
            "--nocapture",
        ])
        .env("VEGA_PRUNE_TEST_REFUSAL_REPO", &f.repo)
        .output()
        .unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stdout.contains("error{code,message,hint}:"), "{stdout}");
    assert!(stdout.contains("prune_incomplete"), "{stdout}");
    assert!(stdout.contains("fast-forward"), "{stdout}");
    assert!(stdout.contains("never use -D"), "{stdout}");
    assert!(!stderr.contains("branch -D"), "{stderr}");
}

#[test]
fn emit_refusal_in_child() {
    let Some(path) = env::var_os("VEGA_PRUNE_TEST_REFUSAL_REPO") else {
        return;
    };
    let r = prune(Path::new(&path), false, &mut Fake::default()).unwrap();
    assert_eq!(display(r, false), ExitCode::from(1));
}

#[test]
fn production_worktrunk_argv_preserves_branches_and_never_forces() {
    for branch in [
        "merged/topic",
        "--force",
        "-f",
        "--yes",
        "--reap",
        "-D",
        "--force-delete",
    ] {
        let tree = Tree {
            path: "/fixture/merged topic".into(),
            branch: Some(branch.into()),
            locked: false,
        };
        let args = remove_args(&tree).unwrap();
        assert_eq!(
            args,
            [
                "remove",
                "--foreground",
                "--no-delete-branch",
                "--",
                "/fixture/merged topic"
            ]
        );
        for forbidden in ["--force", "-f", "--reap", "-D", "--force-delete", "--yes"] {
            assert!(!args[..3].contains(&forbidden));
        }
        assert_eq!(args[args.len() - 2], "--");
        assert!(!args.contains(&branch));
    }
    let relative = Tree {
        path: "--force".into(),
        branch: Some("--force".into()),
        locked: false,
    };
    assert!(remove_args(&relative).is_err());
}

#[test]
fn option_shaped_refs_cannot_select_the_coordinator_checkout() {
    let f = Fixture::new();
    let path = f.root.path().join("option-name");
    git(&f.repo, &["update-ref", "refs/heads/--force", "HEAD"]).unwrap();
    git(
        &f.repo,
        &[
            "worktree",
            "add",
            "--detach",
            path.to_str().unwrap(),
            "HEAD",
        ],
    )
    .unwrap();
    git(&path, &["symbolic-ref", "HEAD", "refs/heads/--force"]).unwrap();
    let main_head = sha(&f.repo, "HEAD").unwrap();
    let r = f.run(false, &mut Fake::default());
    assert!(!r.failed, "{r:?}");
    assert!(!path.exists());
    assert!(!f.exists("--force"));
    assert_eq!(sha(&f.repo, "HEAD").unwrap(), main_head);
    assert!(f.repo.join("base").exists());
}

/// A parallel test may have forked a child that holds a just-closed lock fd's open file description
/// until it execs, so the flock is released a moment late; retry while it reports the lock.
fn retry_lock<T>(mut op: impl FnMut() -> Result<T>) -> Result<T> {
    for _ in 0..100 {
        match op() {
            Err(e) if e.message.contains("repository lock") => {
                std::thread::sleep(std::time::Duration::from_millis(20))
            }
            r => return r,
        }
    }
    op()
}

#[test]
fn unfinished_landings_and_shared_lock_block_before_any_cleanup() {
    let f = Fixture::new();
    let tree = f.tree("keep");
    let root = f.root.path().join("land-state");
    fs::create_dir(&root).unwrap();
    let head = sha(&f.repo, "HEAD").unwrap();
    let mut receipt = Receipt {
        bead: "v-fixture".into(),
        branch: "keep".into(),
        head: head.clone(),
        before: head,
        integrated: None,
        policy: policy(&f.repo).unwrap(),
        assisted_by: "codex".into(),
        summary: "fixture".into(),
        sentinel: None,
        keep: true,
        renumbered: Vec::new(),
        checked: false,
        gate: String::new(),
        install: "pending".into(),
        phase: "check".into(),
        failure: None,
    };
    let path = root.join("v-fixture.json");
    save(&path, &receipt).unwrap();
    let refs = git(&f.repo, &["show-ref"]).unwrap();
    let mut svc = Fake::default();
    for dry in [true, false] {
        let error = locked_prune(&f.repo, dry, &mut svc, &root).unwrap_err();
        assert!(error.message.contains("unfinished landing"));
    }
    assert!(svc.removed.is_empty());
    assert!(tree.exists());
    assert_eq!(git(&f.repo, &["show-ref"]).unwrap(), refs);

    receipt.phase = "done".into();
    save(&path, &receipt).unwrap();
    let lock = File::options().write(true).open(root.join("lock")).unwrap();
    retry_lock(|| {
        lock.try_lock()
            .map_err(|e| Fault::new(format!("repository lock: {e}")))
    })
    .unwrap();
    for dry in [true, false] {
        let error = locked_prune(&f.repo, dry, &mut svc, &root).unwrap_err();
        assert!(error.message.contains("repository lock"));
    }
    assert!(svc.removed.is_empty());
    assert!(tree.exists());
    assert_eq!(git(&f.repo, &["show-ref"]).unwrap(), refs);

    drop(lock);
    let report = retry_lock(|| locked_prune(&f.repo, false, &mut svc, &root)).unwrap();
    assert!(!report.failed);
    assert!(report.residue.is_empty());
    assert!(!tree.exists());
    assert!(!f.exists("keep"));
}
