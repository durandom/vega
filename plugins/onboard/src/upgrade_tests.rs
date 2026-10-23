//! Behind detection and `vega upgrade` on fixture repos (a bare upstream,
//! the checkout, a second clone that pushes), the target pane choice, and
//! fake mise, brew and moshi-hook.

use super::*;
use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt;
use vega_config::Env;

fn sh(dir: &Path, args: &[&str]) -> String {
    let o = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .unwrap();
    assert!(
        o.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&o.stderr)
    );
    String::from_utf8_lossy(&o.stdout).trim().to_string()
}

fn commit(dir: &Path, file: &str, text: &str) -> String {
    fs::write(dir.join(file), text).unwrap();
    sh(dir, &["add", file]);
    sh(dir, &["commit", "-qm", file]);
    sh(dir, &["rev-parse", "HEAD"])
}

const LOG1: &str = "# Changelog\n\n## 1 · 2026-10-01\n\n- feature: one\n";
const LOG2: &str =
    "# Changelog\n\n## 2 · 2026-10-20\n\n- feature: vega upgrade\n\n## 1 · 2026-10-01\n\n- feature: one\n";

struct Fixture {
    d: tempfile::TempDir,
    /// The installed checkout.
    checkout: PathBuf,
    /// Another clone that pushes upstream.
    other: PathBuf,
    /// The installed commit.
    installed: String,
}

impl Fixture {
    fn new() -> Fixture {
        let d = tempfile::tempdir().unwrap();
        let up = d.path().join("up.git");
        fs::create_dir_all(&up).unwrap();
        sh(&up, &["init", "-q", "--bare", "-b", "main"]);
        let checkout = d.path().join("src/vega");
        let other = d.path().join("other");
        for c in [&checkout, &other] {
            fs::create_dir_all(c.parent().unwrap()).unwrap();
        }
        sh(
            d.path(),
            &[
                "clone",
                "-q",
                up.to_str().unwrap(),
                checkout.to_str().unwrap(),
            ],
        );
        sh(&checkout, &["checkout", "-q", "-b", "main"]);
        let installed = commit(&checkout, "CHANGELOG.md", LOG1);
        sh(&checkout, &["push", "-q", "-u", "origin", "main"]);
        sh(
            d.path(),
            &["clone", "-q", up.to_str().unwrap(), other.to_str().unwrap()],
        );
        Fixture {
            d,
            checkout,
            other,
            installed,
        }
    }

    fn push_upstream(&self) -> String {
        commit(&self.other, "a", "a");
        let sha = commit(&self.other, "CHANGELOG.md", LOG2);
        sh(&self.other, &["push", "-q", "origin", "main"]);
        sha
    }

    fn state(&self) -> PathBuf {
        self.d.path().join("state/vega")
    }

    fn record(&self, by: &str) {
        fs::create_dir_all(self.state()).unwrap();
        fs::write(
            self.state().join("install.json"),
            json!({"checkout": self.checkout, "commit": self.installed, "by": by, "at": 1})
                .to_string(),
        )
        .unwrap();
    }

    fn bin(&self) -> PathBuf {
        let b = self.d.path().join("bin");
        fs::create_dir_all(&b).unwrap();
        b
    }

    fn fake(&self, dir: &Path, name: &str, body: &str) -> PathBuf {
        fs::create_dir_all(dir).unwrap();
        let p = dir.join(name);
        fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
        fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
        p
    }

    fn probe(&self) -> Probe {
        let home = self.d.path().join("home");
        fs::create_dir_all(&home).unwrap();
        Probe {
            env: Env::at(&home),
            path: vec![self.bin()],
            vars: BTreeMap::new(),
            tailscale_app: self.d.path().join("none"),
        }
    }

    /// Calls of the fake tools, one per line.
    fn calls(&self) -> String {
        fs::read_to_string(self.d.path().join("calls")).unwrap_or_default()
    }

    fn calls_path(&self) -> String {
        self.d.path().join("calls").display().to_string()
    }
}

#[test]
fn install_record_absent_local_or_deploy() {
    let f = Fixture::new();
    assert!(read_install(&f.state()).is_none());
    f.record("deploy");
    let i = read_install(&f.state()).unwrap().unwrap();
    assert!(i.deployed());
    assert_eq!(i.checkout, f.checkout);
    assert_eq!(i.short(), &f.installed[..7]);
    fs::write(f.state().join("install.json"), "{}").unwrap();
    assert!(read_install(&f.state()).unwrap().is_err());
}

#[test]
fn behind_after_fetch_with_the_new_changelog_blocks() {
    let f = Fixture::new();
    let up = upstream(&f.checkout, &f.installed).unwrap();
    assert_eq!((up.behind, up.ahead, up.pull), (0, 0, 0));
    assert_eq!(up.name, "origin/main");
    let sha = f.push_upstream();
    // Local refs only: nothing seen before the fetch.
    assert_eq!(upstream(&f.checkout, &f.installed).unwrap().behind, 0);
    fetch(&f.checkout).unwrap();
    let up = upstream(&f.checkout, &f.installed).unwrap();
    assert_eq!((up.behind, up.pull, up.sha.as_str()), (2, 2, sha.as_str()));
    let blocks = blocks_at(&f.checkout, &up.sha, 1);
    assert_eq!(blocks.len(), 1);
    assert_eq!(blocks[0].entries[0].text, "vega upgrade");
    // Pulled but not installed: still behind the installed commit.
    sh(&f.checkout, &["pull", "-q", "--ff-only"]);
    assert_eq!(behind_of(&f.checkout, &f.installed, &sha).unwrap(), 2);
    assert_eq!(behind_of(&f.checkout, &sha, &sha).unwrap(), 0);
    // An unknown installed commit counts from HEAD.
    assert_eq!(behind_of(&f.checkout, &"0".repeat(40), &sha).unwrap(), 0);
}

#[test]
fn proposes_once_per_upstream_state() {
    let f = Fixture::new();
    let mut c = Check::default();
    assert!(c.fetch_due(FETCH_EVERY));
    assert!(!c.pending());
    f.push_upstream();
    fetch(&f.checkout).unwrap();
    let up = upstream(&f.checkout, &f.installed).unwrap();
    c.observe(1000, &up, &blocks_at(&f.checkout, &up.sha, 1));
    assert!(!c.fetch_due(1000 + FETCH_EVERY - 1));
    assert!(c.fetch_due(1000 + FETCH_EVERY));
    assert!(c.pending());
    c.proposed = c.upstream.clone();
    assert!(!c.pending());
    c.save(&f.state()).unwrap();
    let loaded = Check::load(&f.state());
    assert_eq!(loaded, c);
    // The same upstream after the next fetch: nothing new.
    c.observe(2000 + FETCH_EVERY, &up, &[]);
    assert!(!c.pending());
    // A newer upstream state: proposed again.
    commit(&f.other, "b", "b");
    sh(&f.other, &["push", "-q", "origin", "main"]);
    fetch(&f.checkout).unwrap();
    let up = upstream(&f.checkout, &f.installed).unwrap();
    c.observe(3000 + FETCH_EVERY, &up, &[]);
    assert_eq!(c.behind, 3);
    assert!(c.pending());
}

#[test]
fn mail_names_the_count_the_changes_and_the_command() {
    let mut c = Check {
        upstream: Some("9f8e7d6c5b4a".into()),
        upstream_name: "origin/main".into(),
        behind: 12,
        changes: vec!["8 · 2026-10-20 feature: vega upgrade".into()],
        ..Default::default()
    };
    let i = Install {
        checkout: "/x".into(),
        commit: "a1b2c3d4e5".into(),
        by: "local".into(),
    };
    let m = c.mail(&i);
    assert!(m.starts_with(
        "vega upgrade available: 12 commits upstream of the installed a1b2c3d (origin/main is 9f8e7d6)."
    ));
    assert!(m.ends_with("\n  8 · 2026-10-20 feature: vega upgrade"));
    // Asks first, and says what to do on yes, before the changelog.
    let ask = m
        .find("Ask the user whether to upgrade now. On yes: run vega upgrade")
        .unwrap();
    assert!(ask < m.find("Changelog:").unwrap());
    assert!(m.contains("act on migration entries"));
    assert_eq!(c.token(), "vega \u{2193}12");
    c.changes.clear();
    assert!(c.mail(&i).contains("Changelog: no new entries."));
}

fn pane(id: &str, ws: &str, cwd: &Path, agent: bool) -> Value {
    let mut p = json!({"pane_id": id, "workspace_id": ws, "cwd": cwd});
    if agent {
        p["agent"] = json!("claude");
    }
    p
}

#[test]
fn target_pane_is_an_agent_in_the_checkout_uac_first() {
    let d = tempfile::tempdir().unwrap();
    let vega = d.path().join("src/vega");
    let sub = vega.join("plugins/post");
    let skills = d.path().join("src/vega-skills");
    let worktree = d.path().join("src/vega.upgrade");
    for p in [&sub, &skills, &worktree] {
        fs::create_dir_all(p).unwrap();
    }
    let list = |ps: Vec<Value>| panes(&json!({"result": {"panes": ps}}));
    let ps = list(vec![
        pane("w1:p3", "w1", &sub, true),
        pane("w1:p2", "w1", &vega, false), // a shell
        pane("w2:p1", "w2", &skills, true),
        pane("w3:p1", "w3", &worktree, true), // a sentinel's worktree
        pane("w1:p9", "w1", &vega, true),
    ]);
    assert_eq!(choose(&ps, &vega, &[]).unwrap().id, "w1:p3");
    assert_eq!(choose(&ps, &vega, &["w1:p9".into()]).unwrap().id, "w1:p9");
    // A UAC of another project does not count.
    assert_eq!(choose(&ps, &vega, &["w2:p1".into()]).unwrap().id, "w1:p3");
    let none = list(vec![
        pane("w2:p1", "w2", &skills, true),
        pane("w1:p2", "w1", &vega, false),
    ]);
    assert!(choose(&none, &vega, &[]).is_none());
    let repo = tempfile::tempdir().unwrap();
    let output = Command::new("git")
        .args(["init", "-q"])
        .arg(repo.path())
        .output()
        .unwrap();
    assert!(output.status.success());
    let output = Command::new("git")
        .arg("-C")
        .arg(repo.path())
        .args([
            "remote",
            "add",
            "origin",
            "https://github.com/team/vega.git",
        ])
        .output()
        .unwrap();
    assert!(output.status.success());
    let records = vec![
        ("github.com/team/vega".to_string(), "w3V:p1".to_string(), 1),
        (
            "github.com/team/vega-skills".to_string(),
            "w93:p1".to_string(),
            1,
        ),
    ];
    assert_eq!(
        uac_panes(&records, repo.path()).unwrap(),
        vec!["w3V:p1".to_string()]
    );
    // No session: the token goes to the checkout's workspaces.
    assert_eq!(workspaces(&none, &vega), vec!["w1".to_string()]);
}

#[test]
fn upgrade_refuses_without_record_on_deploy_targets_dirty_or_diverged() {
    let f = Fixture::new();
    let p = f.probe();
    assert_eq!(run(&f.state(), &p).unwrap_err().code, "no_install_record");
    f.record("deploy");
    assert_eq!(run(&f.state(), &p).unwrap_err().code, "deploy_target");
    f.record("local");
    fs::write(f.checkout.join("CHANGELOG.md"), "edited").unwrap();
    assert_eq!(run(&f.state(), &p).unwrap_err().code, "dirty");
    sh(&f.checkout, &["checkout", "-q", "--", "CHANGELOG.md"]);
    commit(&f.checkout, "local", "mine");
    f.push_upstream();
    let r = run(&f.state(), &p).unwrap_err();
    assert_eq!(r.code, "diverged");
    assert!(r
        .message
        .contains("1 commit(s) origin/main lacks and lacks 2"));
    // Untouched: no pull happened.
    assert_eq!(sh(&f.checkout, &["rev-list", "--count", "HEAD"]), "2");
}

#[test]
fn upgrade_pulls_installs_and_upgrades_moshi_hook() {
    let f = Fixture::new();
    f.record("local");
    let sha = f.push_upstream();
    let calls = f.calls_path();
    let bin = f.bin();
    f.fake(
        &bin,
        "mise",
        &format!("echo \"mise $* in $(pwd)\" >> {calls}; echo building"),
    );
    f.fake(
        &bin,
        "brew",
        &format!("echo \"brew $*\" >> {calls}; echo 'Upgrading moshi-hook 0.4.27 -> 0.4.28'"),
    );
    let cellar = f.d.path().join("Cellar/moshi-hook/0.4.27/bin");
    let real = f.fake(
        &cellar,
        "moshi-hook",
        &format!("echo \"moshi-hook $*\" >> {calls}; echo 'pairing: ok'; echo 'gateway: down' >&2; exit 3"),
    );
    std::os::unix::fs::symlink(real, bin.join("moshi-hook")).unwrap();
    let p = f.probe();
    let agents = p.env.home.join("Library/LaunchAgents");
    fs::create_dir_all(&agents).unwrap();
    fs::write(agents.join("sh.brew.moshi-hook.plist"), "").unwrap();

    let r = run(&f.state(), &p).unwrap();
    assert_eq!(sh(&f.checkout, &["rev-parse", "HEAD"]), sha);
    let steps: Vec<(&str, bool)> = r.steps.iter().map(|s| (s.step, s.ok)).collect();
    assert_eq!(
        steps,
        vec![
            ("pull", true),
            ("install", true),
            ("moshi-hook upgrade", true),
            ("moshi-hook service", true),
            ("moshi-hook doctor", false),
        ]
    );
    assert_eq!(r.steps[0].result, "2 commit(s) from origin/main");
    assert_eq!(r.steps[2].result, "Upgrading moshi-hook 0.4.27 -> 0.4.28");
    assert_eq!(r.doctor, Some((3, "pairing: ok\ngateway: down\n".into())));
    assert!(r.failed());
    let real_checkout = fs::canonicalize(&f.checkout).unwrap();
    assert_eq!(
        f.calls(),
        format!(
            "mise run install-cli in {}\nbrew upgrade moshi-hook\nbrew services restart moshi-hook\nmoshi-hook doctor\n",
            real_checkout.display()
        )
    );
    assert!(fs::read_to_string(f.state().join("upgrade.log"))
        .unwrap()
        .contains("building"));
}

#[test]
fn current_vega_skips_the_install_and_a_failed_install_stops() {
    let f = Fixture::new();
    f.record("local");
    let calls = f.calls_path();
    f.fake(
        &f.bin(),
        "mise",
        &format!("echo \"mise $*\" >> {calls}; echo 'error: cargo failed' >&2; exit 1"),
    );
    let p = f.probe();
    let r = run(&f.state(), &p).unwrap();
    assert_eq!(r.steps[0].result, "level with origin/main");
    assert!(r.steps[1].result.starts_with("current ("));
    assert_eq!(r.steps[2].result, "not installed: skipped");
    assert!(!r.failed());
    assert_eq!(f.calls(), "");
    f.push_upstream();
    let r = run(&f.state(), &p).unwrap();
    let last = r.steps.last().unwrap();
    assert_eq!(last.step, "install");
    assert!(!last.ok);
    assert!(
        last.result.contains("error: cargo failed"),
        "{}",
        last.result
    );
}
