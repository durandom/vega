use super::*;
use tempfile::TempDir;

struct Fixture {
    _root: TempDir,
    repo: PathBuf,
    remote: PathBuf,
    receipt: PathBuf,
    r: Receipt,
}
impl Fixture {
    fn new(strategy: &str) -> Self {
        let root = tempfile::tempdir().unwrap();
        let repo = root.path().join("repo");
        let remote = root.path().join("remote.git");
        fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "-q", "-b", "main"]).unwrap();
        git(&repo, &["config", "user.name", "Fixture"]).unwrap();
        git(&repo, &["config", "user.email", "fixture@example.invalid"]).unwrap();
        git(&repo, &["config", "commit.gpgsign", "false"]).unwrap();
        git(&repo, &["config", "core.hooksPath", "/dev/null"]).unwrap();
        fs::write(repo.join("base"), "base\n").unwrap();
        fs::write(
            repo.join("CHANGELOG.md"),
            "# Changelog\n\n## 1 · 2026-10-20\n\n- published\n\n",
        )
        .unwrap();
        commit(&repo, "base");
        let before = sha(&repo, "HEAD").unwrap();
        git(&repo, &["init", "--bare", "-q", remote.to_str().unwrap()]).unwrap();
        git(
            &repo,
            &["remote", "add", "origin", remote.to_str().unwrap()],
        )
        .unwrap();
        git(&repo, &["push", "-q", "origin", "main"]).unwrap();
        git(&repo, &["switch", "-q", "-c", "feature"]).unwrap();
        fs::write(repo.join("feature"), "feature\n").unwrap();
        commit(&repo, "feature");
        let head = sha(&repo, "HEAD").unwrap();
        git(&repo, &["switch", "-q", "main"]).unwrap();
        let receipt = root.path().join("receipt.json");
        let r = Receipt {
            bead: "v-1".into(),
            branch: "feature".into(),
            head,
            before,
            integrated: None,
            policy: Policy {
                strategy: strategy.into(),
                check: vec!["true".into()],
                install: Vec::new(),
                remote: "origin".into(),
                target: "main".into(),
            },
            assisted_by: "codex".into(),
            summary: "Changed feature; follow-ups: none.".into(),
            sentinel: None,
            keep: false,
            renumbered: Vec::new(),
            checked: false,
            gate: String::new(),
            install: "pending".into(),
            phase: "merge".into(),
            failure: None,
        };
        save(&receipt, &r).unwrap();
        Self {
            _root: root,
            repo,
            remote,
            receipt,
            r,
        }
    }
    fn run(&mut self, svc: &mut Fake) -> Result<()> {
        step(&self.repo, &self.receipt, &mut self.r, svc)
    }
    fn count_merges(&self) -> usize {
        git(&self.repo, &["rev-list", "--merges", "HEAD"])
            .unwrap()
            .lines()
            .count()
    }
    fn add_sentinel(&mut self) {
        let path = self._root.path().join("worker");
        git(
            &self.repo,
            &["worktree", "add", "-q", path.to_str().unwrap(), "feature"],
        )
        .unwrap();
        // Git records the canonical path (/private/var on macOS), just as
        // production evidence obtains it from `git worktree list`.
        let path = path.canonicalize().unwrap();
        self.r.sentinel = Some(Sentinel {
            pane: "w1:p2".into(),
            name: "feature".into(),
            session: "session".into(),
            actor: "h/worker".into(),
            worktree: path,
            brief: 1,
        });
    }
}
fn commit(dir: &Path, message: &str) {
    git(dir, &["add", "."]).unwrap();
    git(
        dir,
        &[
            "commit",
            "-q",
            "-m",
            message,
            "--trailer",
            "Assisted-by: codex",
        ],
    )
    .unwrap();
}
#[derive(Default)]
struct Fake {
    fail: String,
    closed: bool,
    pane_closed: bool,
    removed: bool,
    calls: Vec<String>,
}
impl Fake {
    fn call(&mut self, phase: &str) -> Result<()> {
        self.calls.push(phase.into());
        if self.fail == phase {
            Err(Fault {
                message: format!("injected {phase}"),
                status: Some(23),
                output: "fixture failed".into(),
            })
        } else {
            Ok(())
        }
    }
}
impl Services for Fake {
    fn authority(&mut self, _: &Path, _: &str) -> Result<()> {
        self.call("authority")
    }
    fn completed(&mut self, _: &Path, r: &Receipt) -> Result<()> {
        self.call("completed")?;
        if !r.keep && r.sentinel.as_ref().is_some_and(|s| s.worktree.exists()) {
            return Err(Fault::new("recreated worktree retained"));
        }
        Ok(())
    }
    fn validate(&mut self, _: &Path, _: &Receipt) -> Result<()> {
        self.call("validate")
    }
    fn evidence(
        &mut self,
        _: &Path,
        _: &str,
        _: &str,
        _: &str,
    ) -> Result<(String, Option<Sentinel>)> {
        self.call("evidence")?;
        Ok(("report".into(), None))
    }
    fn gate(&mut self, _: &Path, _: &str, _: &str, _: bool) -> Result<String> {
        self.call("gate")?;
        Ok("pass fixture".into())
    }
    fn assisted_by(&mut self) -> Result<String> {
        Ok("claude".into())
    }
    fn close(&mut self, _: &Path, _: &Receipt) -> Result<()> {
        self.call("close")?;
        self.closed = true;
        Ok(())
    }
    fn pane(&mut self, _: &Path, _: &Receipt) -> Result<()> {
        self.call("pane")?;
        self.pane_closed = true;
        Ok(())
    }
    fn remove(&mut self, _: &Path, _: &Receipt) -> Result<()> {
        self.call("worktree")?;
        self.removed = true;
        Ok(())
    }
}

#[test]
fn strict_usage_and_policy_validate_before_effects() {
    let p = |s: &str| parse(&s.split_whitespace().map(str::to_string).collect::<Vec<_>>());
    for text in [
        "",
        "v-1",
        "v-1 --branche feature",
        "v-1 --branch",
        "v-1 --branch x --branch y",
        "v-1 --resume --branch x",
        "v-1 --resume --status",
        "../bad --status",
        "v-1 --status --keep",
        "v-1 extra --status",
        "v-1 --status --retry-install",
        "v-1 --resume --retry-install --install-done",
    ] {
        assert!(p(text).is_err(), "{text}");
    }
    assert!(p("v-1 --branch x --head abc --target integration --keep").is_ok());
    assert!(p("v-1 --resume").unwrap().resume);
    assert!(p("v-1 --resume --retry-install").unwrap().retry_install);
    assert!(p("v-1 --resume --install-done").unwrap().install_done);
    assert!(Policy::parse("strategy='squash'\ncheck=['true']").is_err());
    assert!(Policy::parse("strategy='no-ff'\ncheck=[]").is_err());
    assert!(Policy::parse("strategy='no-ff'\ncheck=['true']\nassisted_by='wrong'").is_err());
    assert!(Policy::parse("strategy='ff-only'\ncheck=['pytest','-q']\ninstall=['true']").is_ok());
}
#[test]
fn both_strategies_and_completed_resume_keep_one_merge() {
    for strategy in ["no-ff", "ff-only"] {
        let mut f = Fixture::new(strategy);
        let mut svc = Fake::default();
        f.run(&mut svc).unwrap();
        assert_eq!(f.r.phase, "done");
        assert!(svc.closed);
        assert_eq!(
            remote_head(&f.repo, &f.r.policy).unwrap(),
            f.r.integrated.clone().unwrap()
        );
        assert_eq!(f.count_merges(), usize::from(strategy == "no-ff"));
        let integrated = sha(&f.repo, "HEAD").unwrap();
        let body = git(&f.repo, &["show", "-s", "--format=%B", "HEAD"]).unwrap();
        assert!(body.contains("Assisted-by: codex"));
        f.r = read(&f.receipt).unwrap();
        f.run(&mut svc).unwrap();
        assert_eq!(sha(&f.repo, "HEAD").unwrap(), integrated);
        assert!(sha(&f.repo, "refs/heads/feature").is_ok());
    }
}
#[test]
fn gate_closure_and_cleanup_failures_resume_without_remerging() {
    for phase in ["authority", "gate", "close", "pane", "worktree"] {
        let mut f = Fixture::new("no-ff");
        f.add_sentinel();
        let mut svc = Fake {
            fail: phase.into(),
            ..Fake::default()
        };
        let e = f.run(&mut svc).unwrap_err();
        assert_eq!(e.status, Some(23));
        if matches!(phase, "authority" | "gate") {
            assert_eq!(remote_head(&f.repo, &f.r.policy).unwrap(), f.r.before);
        }
        if phase == "pane" {
            assert!(!svc.removed);
        }
        svc.fail.clear();
        f.run(&mut svc).unwrap();
        assert_eq!(f.count_merges(), 1, "{phase}");
        assert!(svc.closed && svc.pane_closed && svc.removed);
    }
}
#[test]
fn check_uses_actual_exit_status_and_resume_reruns_failure() {
    let mut f = Fixture::new("no-ff");
    f.r.policy.check = vec![
        "sh".into(),
        "-c".into(),
        "echo all-tests-passed; exit 19".into(),
    ];
    let e = f.run(&mut Fake::default()).unwrap_err();
    assert_eq!(e.status, Some(19));
    assert!(e.output.contains("all-tests-passed"));
    assert!(!f.r.checked);
    assert_eq!(remote_head(&f.repo, &f.r.policy).unwrap(), f.r.before);
    f.r.policy.check = vec!["true".into()];
    f.run(&mut Fake::default()).unwrap();
    assert_eq!(f.count_merges(), 1);
}
#[test]
fn check_mutation_and_changed_head_never_reuse_validation() {
    let mut f = Fixture::new("no-ff");
    f.r.policy.check = vec!["sh".into(), "-c".into(), "echo changed > base".into()];
    assert!(f
        .run(&mut Fake::default())
        .unwrap_err()
        .message
        .contains("dirty"));
    assert!(!f.r.checked);
    assert_eq!(remote_head(&f.repo, &f.r.policy).unwrap(), f.r.before);
    commit(&f.repo, "unexpected");
    f.r.policy.check = vec!["true".into()];
    assert!(f
        .run(&mut Fake::default())
        .unwrap_err()
        .message
        .contains("HEAD changed"));
}
#[test]
fn installs_retry_real_failures_but_interrupted_install_is_uncertain() {
    let mut f = Fixture::new("no-ff");
    f.r.policy.install = vec!["false".into()];
    assert_eq!(f.run(&mut Fake::default()).unwrap_err().status, Some(1));
    assert_eq!(f.r.install, "pending");
    let pushed = remote_head(&f.repo, &f.r.policy).unwrap();
    assert_ne!(pushed, f.r.before);
    f.r.install = "started".into();
    assert!(f
        .run(&mut Fake::default())
        .unwrap_err()
        .message
        .contains("uncertain"));
    f.r.install = "pending".into();
    f.r.policy.install = vec!["true".into()];
    f.run(&mut Fake::default()).unwrap();
    assert_eq!(f.r.install, "done");
    assert_eq!(f.count_merges(), 1);
}
#[test]
fn crash_after_merge_before_receipt_is_reconciled_for_both_strategies() {
    for strategy in ["no-ff", "ff-only"] {
        let mut f = Fixture::new(strategy);
        let old = f.r.clone();
        reconcile_merge(&f.repo, &f.receipt, &mut f.r).unwrap();
        let integrated = sha(&f.repo, "HEAD").unwrap();
        f.r = old;
        f.run(&mut Fake::default()).unwrap();
        assert_eq!(f.r.integrated, Some(integrated));
        assert_eq!(f.count_merges(), usize::from(strategy == "no-ff"));
    }
}
#[test]
fn push_failure_and_crash_after_push_are_reconciled() {
    use std::os::unix::fs::PermissionsExt;
    let mut f = Fixture::new("no-ff");
    let hook = f.remote.join("hooks/pre-receive");
    fs::write(&hook, "#!/bin/sh\necho denied >&2\nexit 1\n").unwrap();
    fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
    // Bare fixtures inherit the global hooksPath too; explicitly use this hook.
    git(&f.remote, &["config", "core.hooksPath", "hooks"]).unwrap();
    assert_eq!(f.run(&mut Fake::default()).unwrap_err().status, Some(1));
    assert_eq!(remote_head(&f.repo, &f.r.policy).unwrap(), f.r.before);
    fs::remove_file(hook).unwrap();
    push(&f.repo, &f.r).unwrap();
    let after = sha(&f.repo, "HEAD").unwrap();
    f.r = read(&f.receipt).unwrap();
    f.run(&mut Fake::default()).unwrap();
    assert_eq!(sha(&f.repo, "HEAD").unwrap(), after);
    assert_eq!(f.count_merges(), 1);
    let timeout = Fault {
        message: "timeout".into(),
        status: Some(128),
        output: "ssh: connect to host github.com port 22: Operation timed out".into(),
    };
    assert!(ssh_timeout(&timeout));
    assert!(!ssh_timeout(&Fault {
        output: "Permission denied (publickey)".into(),
        ..timeout
    }));
}
#[test]
fn remote_movement_is_never_overwritten() {
    let mut f = Fixture::new("no-ff");
    reconcile_merge(&f.repo, &f.receipt, &mut f.r).unwrap();
    let other = git(
        &f.repo,
        &[
            "commit-tree",
            &format!("{}^{{tree}}", f.r.before),
            "-p",
            &f.r.before,
            "-m",
            "concurrent target",
        ],
    )
    .unwrap();
    git(
        &f.repo,
        &["push", "origin", &format!("{other}:refs/heads/main")],
    )
    .unwrap();
    assert!(f
        .run(&mut Fake::default())
        .unwrap_err()
        .message
        .contains("remote target moved"));
    assert_eq!(remote_head(&f.repo, &f.r.policy).unwrap(), other);
}
#[test]
fn keep_never_closes_pane_or_worktree_and_residue_preserves_stashes() {
    let mut f = Fixture::new("no-ff");
    f.add_sentinel();
    f.r.keep = true;
    fs::write(f.repo.join("base"), "stash me").unwrap();
    git(&f.repo, &["stash", "push", "-q", "-m", "preserve"]).unwrap();
    let mut svc = Fake::default();
    f.run(&mut svc).unwrap();
    assert!(!svc.pane_closed && !svc.removed);
    let rows = residue(&f.repo, Some(&f.r));
    assert!(rows.iter().any(|(kind, _)| kind == "stash"));
    assert!(rows
        .iter()
        .any(|(kind, detail)| kind == "unknown" && detail.contains("child processes")));
    assert!(rows.iter().any(|(kind, _)| kind == "worktree"));
    assert_eq!(git(&f.repo, &["stash", "list"]).unwrap().lines().count(), 1);
}
#[test]
fn parallel_changelog_conflict_resolves_and_records_renumbering() {
    let mut f = Fixture::new("no-ff");
    let base = fs::read_to_string(f.repo.join("CHANGELOG.md")).unwrap();
    git(&f.repo, &["switch", "-q", "feature"]).unwrap();
    fs::write(
        f.repo.join("CHANGELOG.md"),
        base.replacen("## 1", "## 2 · 2026-10-21\n\n- incoming\n\n## 1", 1),
    )
    .unwrap();
    commit(&f.repo, "incoming changelog");
    f.r.head = sha(&f.repo, "HEAD").unwrap();
    git(&f.repo, &["switch", "-q", "main"]).unwrap();
    let ours = base.replacen("## 1", "## 2 · 2026-10-21\n\n- target\n\n## 1", 1);
    fs::write(f.repo.join("CHANGELOG.md"), &ours).unwrap();
    commit(&f.repo, "target changelog");
    f.r.before = sha(&f.repo, "HEAD").unwrap();
    git(&f.repo, &["push", "-q", "origin", "main"]).unwrap();
    f.run(&mut Fake::default()).unwrap();
    assert_eq!(f.r.renumbered, [(2, 3)]);
    let result = fs::read_to_string(f.repo.join("CHANGELOG.md")).unwrap();
    assert!(result.ends_with(ours.strip_prefix("# Changelog\n\n").unwrap()));
    assert!(result.contains("## 3 · 2026-10-21\n\n- incoming"));
}
#[test]
fn unsafe_conflict_is_preserved_until_manually_resolved() {
    let mut f = Fixture::new("no-ff");
    git(&f.repo, &["switch", "-q", "feature"]).unwrap();
    fs::write(f.repo.join("base"), "incoming").unwrap();
    commit(&f.repo, "incoming conflict");
    f.r.head = sha(&f.repo, "HEAD").unwrap();
    git(&f.repo, &["switch", "-q", "main"]).unwrap();
    fs::write(f.repo.join("base"), "target").unwrap();
    commit(&f.repo, "target conflict");
    f.r.before = sha(&f.repo, "HEAD").unwrap();
    git(&f.repo, &["push", "-q", "origin", "main"]).unwrap();
    assert!(f
        .run(&mut Fake::default())
        .unwrap_err()
        .message
        .contains("requires UAC resolution"));
    assert!(state_path(&f.repo, "MERGE_HEAD").unwrap().exists());
    fs::write(f.repo.join("base"), "resolved").unwrap();
    commit(&f.repo, "resolved merge");
    f.run(&mut Fake::default()).unwrap();
    assert_eq!(f.count_merges(), 1);
}

#[test]
fn push_retries_only_ssh_timeouts_and_verifies_even_success() {
    let mut f = Fixture::new("no-ff");
    reconcile_merge(&f.repo, &f.receipt, &mut f.r).unwrap();
    let mut attempts = 0;
    push_using(&f.r, |args| {
        if args[0] == "push" {
            attempts += 1;
            if attempts < 3 {
                return Err(Fault {
                    message: "SSH timeout".into(),
                    status: Some(128),
                    output: "ssh: connect to host github.com port 22: Connection timed out".into(),
                });
            }
        }
        git(&f.repo, args)
    })
    .unwrap();
    assert_eq!(attempts, 3);
    // A lost successful response is reconciled from the remote, never resent.
    let mut g = Fixture::new("no-ff");
    reconcile_merge(&g.repo, &g.receipt, &mut g.r).unwrap();
    let mut sent = 0;
    push_using(&g.r, |args| {
        let out = git(&g.repo, args)?;
        if args[0] == "push" {
            sent += 1;
            return Err(Fault {
                message: "lost response".into(),
                status: Some(128),
                output: "ssh: github.com Connection timed out".into(),
            });
        }
        Ok(out)
    })
    .unwrap();
    assert_eq!(sent, 1);
    for text in [
        "Permission denied (publickey)",
        "ssh: github.com Connection timed out",
    ] {
        let mut h = Fixture::new("no-ff");
        reconcile_merge(&h.repo, &h.receipt, &mut h.r).unwrap();
        let mut attempts = 0;
        assert!(push_using(&h.r, |args| {
            if args[0] == "push" {
                attempts += 1;
                return Err(Fault {
                    message: "failed".into(),
                    status: Some(128),
                    output: text.into(),
                });
            }
            git(&h.repo, args)
        })
        .is_err());
        assert_eq!(attempts, if text.starts_with("ssh:") { 3 } else { 1 });
        let e = push_using(&h.r, |args| {
            if args[0] == "push" {
                Ok("claims success without pushing".into())
            } else {
                git(&h.repo, args)
            }
        })
        .unwrap_err();
        assert!(e.message.contains("returned success but remote SHA"));
    }
}

#[test]
fn preflight_freezes_target_policy_and_supports_integration_override() {
    let f = Fixture::new("no-ff");
    fs::create_dir(f.repo.join(".vega")).unwrap();
    fs::write(
        f.repo.join(".vega/land.toml"),
        "strategy='no-ff'\ncheck=['true']\n",
    )
    .unwrap();
    commit(&f.repo, "land policy");
    git(&f.repo, &["switch", "-q", "-c", "integration"]).unwrap();
    git(&f.repo, &["push", "-q", "origin", "integration"]).unwrap();
    let a = Args {
        bead: "v-1".into(),
        branch: Some("feature".into()),
        target: Some("integration".into()),
        keep: true,
        ..Args::default()
    };
    let r = begin(&f.repo, &a, &mut Fake::default()).unwrap();
    assert_eq!(r.policy.target, "integration");
    assert_eq!(r.policy.strategy, "no-ff");
    assert!(r.keep);
    assert_eq!(r.head, f.r.head);
    let mut bad = a;
    bad.head = Some("deadbeef".into());
    assert!(begin(&f.repo, &bad, &mut Fake::default())
        .unwrap_err()
        .message
        .contains("full source branch tip"));
    fs::write(f.repo.join("untracked"), "retain").unwrap();
    assert!(begin(&f.repo, &bad, &mut Fake::default())
        .unwrap_err()
        .message
        .contains("dirty"));
}

#[test]
fn completed_check_and_install_are_not_reexecuted_after_late_failure() {
    let mut f = Fixture::new("no-ff");
    let counter = f._root.path().join("effects");
    let script = format!("echo effect >> '{}'", counter.display());
    f.r.policy.check = vec!["sh".into(), "-c".into(), script.clone()];
    f.r.policy.install = vec!["sh".into(), "-c".into(), script];
    let mut svc = Fake {
        fail: "close".into(),
        ..Fake::default()
    };
    assert!(f.run(&mut svc).is_err());
    assert_eq!(fs::read_to_string(&counter).unwrap().lines().count(), 2);
    f.r = read(&f.receipt).unwrap();
    svc.fail.clear();
    f.run(&mut svc).unwrap();
    assert_eq!(fs::read_to_string(counter).unwrap().lines().count(), 2);
    assert_eq!(f.count_merges(), 1);
}

#[test]
fn sentinel_worktree_is_rejected_before_authority_lookup() {
    let mut f = Fixture::new("no-ff");
    f.add_sentinel();
    let error = live::Live
        .authority(&f.r.sentinel.as_ref().unwrap().worktree, "main")
        .unwrap_err();
    assert!(error.message.contains("sentinels cannot land"));
}

#[test]
fn configured_gate_is_fail_closed_for_missing_running_failed_and_unavailable_records() {
    use std::os::unix::fs::PermissionsExt;
    struct Restore(Option<std::ffi::OsString>);
    impl Drop for Restore {
        fn drop(&mut self) {
            if let Some(v) = &self.0 {
                env::set_var("VEGA_GATE_BD", v);
            } else {
                env::remove_var("VEGA_GATE_BD");
            }
        }
    }
    let f = Fixture::new("no-ff");
    assert_eq!(
        live::Live
            .gate(&f.repo, &f.r.before, &f.r.head, false)
            .unwrap(),
        "not configured on target"
    );
    fs::create_dir(f.repo.join(".vega")).unwrap();
    fs::write(
        f.repo.join(".vega/gate.toml"),
        "[[review]]\nname='fixture'\npaths=['feature']\n",
    )
    .unwrap();
    commit(&f.repo, "configure gate");
    let target = sha(&f.repo, "HEAD").unwrap();
    assert!(live::Live
        .gate(&f.repo, &target, &f.r.head, false)
        .unwrap_err()
        .message
        .contains("needs beads"));
    fs::create_dir(f.repo.join(".beads")).unwrap();
    let stub = f._root.path().join("bd-fixture");
    fs::write(&stub, "#!/bin/sh\nexit 1\n").unwrap();
    fs::set_permissions(&stub, fs::Permissions::from_mode(0o755)).unwrap();
    let _restore = Restore(env::var_os("VEGA_GATE_BD"));
    env::set_var("VEGA_GATE_BD", &stub);
    assert!(live::Live.gate(&f.repo, &target, &f.r.head, false).is_err());
    for body in [
        "[]",
        r#"[{"id":"g-1","status":"open","metadata":{"verdict":"running"}}]"#,
        r#"[{"id":"g-1","status":"closed","metadata":{"verdict":"fail"}}]"#,
    ] {
        fs::write(&stub, format!("#!/bin/sh\ncat <<'JSON'\n{body}\nJSON\n")).unwrap();
        assert!(live::Live
            .gate(&f.repo, &target, &f.r.head, false)
            .unwrap_err()
            .message
            .contains("lacks gate pass"));
    }
    fs::write(&stub, "#!/bin/sh\nprintf '%s\\n' '[{\"id\":\"g-1\",\"status\":\"closed\",\"metadata\":{\"verdict\":\"pass\"}}]'\n").unwrap();
    assert_eq!(
        live::Live.gate(&f.repo, &target, &f.r.head, false).unwrap(),
        "pass: g-1"
    );
}

#[test]
fn cleanup_refuses_dirty_worktree_without_deleting_it() {
    let mut f = Fixture::new("no-ff");
    f.add_sentinel();
    reconcile_merge(&f.repo, &f.receipt, &mut f.r).unwrap();
    let s = f.r.sentinel.as_ref().unwrap();
    live::validate_tree(&f.repo, &f.r, s).unwrap();
    fs::write(s.worktree.join("untracked"), "retain").unwrap();
    assert!(live::validate_tree(&f.repo, &f.r, s)
        .unwrap_err()
        .message
        .contains("dirty"));
    assert!(s.worktree.join("untracked").exists());
    assert!(sha(&f.repo, "feature").is_ok());
}

#[test]
fn completed_resume_does_not_replay_cleanup_or_rewrite_receipt() {
    let mut f = Fixture::new("no-ff");
    f.add_sentinel();
    let mut svc = Fake::default();
    f.run(&mut svc).unwrap();
    let path = f.r.sentinel.as_ref().unwrap().worktree.clone();
    // Fake cleanup retained the directory; treat it as a recreated worktree.
    fs::write(path.join("new-work"), "preserve").unwrap();
    let before = fs::read(&f.receipt).unwrap();
    svc.calls.clear();
    svc.fail = "gate".into();
    assert!(f.run(&mut svc).unwrap_err().message.contains("recreated"));
    assert_eq!(svc.calls, ["authority", "completed"]);
    assert_eq!(f.r.phase, "done");
    assert_eq!(fs::read(&f.receipt).unwrap(), before);
    assert_eq!(
        fs::read_to_string(path.join("new-work")).unwrap(),
        "preserve"
    );
    assert_eq!(f.count_merges(), 1);
}

#[test]
fn bare_layout_target_checkout_is_accepted_but_sentinel_is_not() {
    let f = Fixture::new("no-ff");
    let bare = f._root.path().join("layout.git");
    let target = f._root.path().join("main");
    let worker = f._root.path().join("sentinel");
    git(
        &f.repo,
        &[
            "clone",
            "--bare",
            "-q",
            f.repo.to_str().unwrap(),
            bare.to_str().unwrap(),
        ],
    )
    .unwrap();
    git(
        &bare,
        &["worktree", "add", "-q", target.to_str().unwrap(), "main"],
    )
    .unwrap();
    git(
        &bare,
        &["worktree", "add", "-q", worker.to_str().unwrap(), "feature"],
    )
    .unwrap();
    live::checkout_authority(&target, "main").unwrap();
    assert!(live::checkout_authority(&worker, "main").is_err());
}

#[test]
fn install_attestation_is_distinct_from_a_measured_exit() {
    let mut f = Fixture::new("no-ff");
    assert_eq!(live::install_result(&f.r), "not configured");
    f.r.policy.install = vec!["false".into()];
    f.r.install = "attested".into();
    assert!(live::install_result(&f.r).contains("unmeasured"));
    f.run(&mut Fake::default()).unwrap();
    assert_eq!(f.r.install, "attested");
    f.r.install = "done".into();
    assert_eq!(live::install_result(&f.r), "exit 0");
}

#[test]
fn shared_worktree_occupants_block_cleanup_but_other_worktrees_do_not() {
    let mut f = Fixture::new("no-ff");
    f.add_sentinel();
    let s = f.r.sentinel.as_ref().unwrap();
    let subdir = s.worktree.join("subdir");
    fs::create_dir(&subdir).unwrap();
    let panes = vec![
        json!({"pane_id": s.pane, "cwd": s.worktree}),
        json!({"pane_id": "w1:p3", "cwd": subdir}),
        json!({"pane_id": "w2:p1", "cwd": f.repo}),
    ];
    assert_eq!(
        live::occupants(&panes, &s.worktree, Some(&s.pane), None),
        ["w1:p3"]
    );
    assert_eq!(
        live::occupants(&panes[..1], &s.worktree, None, None).as_slice(),
        std::slice::from_ref(&s.pane)
    );
    assert!(live::occupants(&panes[2..], &s.worktree, None, None).is_empty());
    assert!(s.worktree.exists());
    let shell =
        vec![json!({"pane_id": "w1:p4", "workspace_id": "source-workspace", "cwd": f.repo})];
    assert_eq!(
        live::occupants(&shell, &s.worktree, Some(&s.pane), Some("source-workspace")),
        ["w1:p4"]
    );
    let unknown = vec![json!({"cwd": s.worktree})];
    assert_eq!(
        live::occupants(&unknown, &s.worktree, None, None),
        ["unknown pane"]
    );
}

#[test]
fn failed_push_preserves_original_error_when_remote_verification_also_fails() {
    let mut f = Fixture::new("no-ff");
    reconcile_merge(&f.repo, &f.receipt, &mut f.r).unwrap();
    let mut pushed = false;
    let e = push_using(&f.r, |args| {
        if args[0] == "push" {
            pushed = true;
            return Err(Fault {
                message: "original push".into(),
                status: Some(128),
                output: "ssh: github.com Connection timed out".into(),
            });
        }
        if pushed {
            Err(Fault {
                message: "verification".into(),
                status: Some(2),
                output: "remote unreachable".into(),
            })
        } else {
            git(&f.repo, args)
        }
    })
    .unwrap_err();
    assert_eq!(e.message, "original push");
    assert_eq!(e.status, Some(128));
    assert!(ssh_timeout(&e));
}

#[test]
fn completed_receipt_missing_integrated_sha_returns_error_without_replay() {
    let mut f = Fixture::new("no-ff");
    f.r.phase = "done".into();
    save(&f.receipt, &f.r).unwrap();
    let mut svc = Fake::default();
    let before = fs::read(&f.receipt).unwrap();
    let e = f.run(&mut svc).unwrap_err();
    assert!(e.message.contains("receipt is missing the integrated SHA"));
    assert_eq!(svc.calls, ["authority"]);
    assert_eq!(f.r.phase, "done");
    assert_eq!(sha(&f.repo, "HEAD").unwrap(), f.r.before);
    assert_eq!(fs::read(&f.receipt).unwrap(), before);
}

#[test]
fn interrupted_install_missing_integrated_sha_returns_error_without_attestation() {
    let mut f = Fixture::new("no-ff");
    f.r.install = "started".into();
    for attested in [false, true] {
        let e = recover_install(&f.repo, &mut f.r, attested).unwrap_err();
        assert!(e.message.contains("receipt is missing the integrated SHA"));
        assert_eq!(f.r.install, "started");
        assert_eq!(sha(&f.repo, "HEAD").unwrap(), f.r.before);
        assert_eq!(remote_head(&f.repo, &f.r.policy).unwrap(), f.r.before);
    }
}

#[test]
fn merge_without_unmerged_paths_names_commit_recovery_and_preserves_state() {
    let mut f = Fixture::new("no-ff");
    git(&f.repo, &["merge", "--no-ff", "--no-commit", &f.r.head]).unwrap();
    let e = changelog::resolve(&f.repo, &f.receipt, &mut f.r).unwrap_err();
    assert!(e.message.contains("no unmerged paths remain"));
    assert!(e.message.contains("inspect staged changes"));
    assert!(e.message.contains("git commit --no-edit --trailer"));
    assert!(e.message.contains("vega uac land v-1 --resume"));
    assert!(state_path(&f.repo, "MERGE_HEAD").unwrap().exists());
    assert_eq!(sha(&f.repo, "HEAD").unwrap(), f.r.before);
    assert_eq!(f.count_merges(), 0);
}

#[test]
fn sentinel_closed_bead_keeps_reason_and_appends_receipt_once() {
    use std::os::unix::fs::PermissionsExt;
    struct RestorePath(std::ffi::OsString);
    impl Drop for RestorePath {
        fn drop(&mut self) {
            env::set_var("PATH", &self.0);
        }
    }
    let mut f = Fixture::new("no-ff");
    f.add_sentinel();
    reconcile_merge(&f.repo, &f.receipt, &mut f.r).unwrap();
    f.r.keep = true;
    let bin = f._root.path().join("bin");
    fs::create_dir(&bin).unwrap();
    let state = f._root.path().join("bead.json");
    fs::write(
        &state,
        serde_json::to_vec(&json!([{
            "id": f.r.bead, "status": "closed", "assignee": "h/worker",
            "close_reason": "Sentinel's original close reason", "notes": "Existing notes"
        }]))
        .unwrap(),
    )
    .unwrap();
    let stub = bin.join("bd");
    // A strict stub: only history/show and the guarded append are permitted.
    fs::write(&stub, format!(r#"#!/usr/bin/env python3
import json, sys
path = {path:?}
a = sys.argv[1:]
if a == ['show', 'v-1', '--json']:
    print(open(path).read())
elif a == ['history', 'v-1', '--events', '--json']:
    print(json.dumps([{{'issue_id':'v-1','event_type':'closed','actor':'h/worker','created_at':'2026-10-21T08:00:00Z'}}]))
elif len(a) == 6 and a[:5] == ['update','v-1','--if-status','closed','--append-notes']:
    b = json.load(open(path))
    b[0]['notes'] += '\n' + a[5]
    with open(path, 'w') as out: json.dump(b, out)
else:
    sys.exit('unexpected bd argv: ' + repr(a))
"#, path = state.to_str().unwrap())).unwrap();
    fs::set_permissions(&stub, fs::Permissions::from_mode(0o755)).unwrap();
    let _restore = RestorePath(env::var_os("PATH").unwrap());
    env::set_var(
        "PATH",
        env::join_paths(std::iter::once(bin).chain(env::split_paths(&_restore.0))).unwrap(),
    );
    let before: Value = serde_json::from_slice(&fs::read(&state).unwrap()).unwrap();
    live::accept_closed(&f.repo, &before[0], &f.r.bead, f.r.sentinel.as_ref()).unwrap();
    live::Live.close(&f.repo, &f.r).unwrap();
    let first = fs::read(&state).unwrap();
    live::Live.close(&f.repo, &f.r).unwrap();
    assert_eq!(first, fs::read(&state).unwrap());
    let b: Value = serde_json::from_slice(&first).unwrap();
    assert_eq!(b[0]["close_reason"], "Sentinel's original close reason");
    let notes = b[0]["notes"].as_str().unwrap();
    assert!(notes.starts_with("Existing notes\n"));
    assert_eq!(notes.matches(&f.r.marker()).count(), 1);
    assert!(notes.contains(&f.r.head));
    assert!(notes.contains("Project check: exit 0"));
    // A completed resume accepts the marker in notes without changing it.
    // Avoid external pane inventory: closure itself is verified above.
    live::Live.validate(&f.repo, &f.r).unwrap();
}
