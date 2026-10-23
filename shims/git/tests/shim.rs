//! The git shim against the real git, on fixture repos with fake bd, vega
//! and herdr (plugins/gate/tests/common).

#[path = "../../../plugins/gate/tests/common/mod.rs"]
mod common;

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::OnceLock;

use common::{commit, fakes, git, log, repo};

const SHIM: &str = env!("CARGO_BIN_EXE_git");

/// `PATH` with the real git's exec dir first: the shim under test must not
/// reach an installed git shim (an agent session's PATH), which would gate
/// the push a second time.
fn path() -> &'static str {
    static PATH: OnceLock<String> = OnceLock::new();
    PATH.get_or_init(|| {
        let o = Command::new("git").arg("--exec-path").output().unwrap();
        let exec = String::from_utf8_lossy(&o.stdout).trim().to_string();
        format!("{exec}:{}", std::env::var("PATH").unwrap_or_default())
    })
}

fn cmd(bin: &str, dir: &Path, tmp: &Path, env: &[(&str, &str)], args: &[&str]) -> Command {
    common::git_env();
    let mut c = Command::new(bin);
    c.args(args)
        .current_dir(dir)
        .env("PATH", path())
        .env("XDG_STATE_HOME", tmp.join("state"))
        .env("XDG_CACHE_HOME", tmp.join("cache"))
        .env("VEGA_TOKEN_BURN", "/nonexistent/token-burn")
        .env("CLAUDE_CODE_SESSION_ID", "test-session")
        .env_remove("HERDR_PANE_ID")
        .env_remove("VEGA_GATE")
        .env_remove("VEGA_GATE_ROLE")
        .env_remove("VEGA_GATE_GIT");
    for (k, v) in fakes(tmp) {
        c.env(k, v);
    }
    for (k, v) in env {
        c.env(k, v);
    }
    c
}

fn shim(dir: &Path, tmp: &Path, env: &[(&str, &str)], args: &[&str]) -> Output {
    cmd(SHIM, dir, tmp, env, args).output().unwrap()
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

fn remote_head(work: &Path, branch: &str) -> Option<String> {
    let o = Command::new("git")
        .args(["ls-remote", "origin", &format!("refs/heads/{branch}")])
        .current_dir(work)
        .output()
        .unwrap();
    String::from_utf8_lossy(&o.stdout)
        .split_whitespace()
        .next()
        .map(String::from)
}

/// A gated repo with a branch `feat` holding one reviewed commit.
fn gated(tmp: &Path) -> PathBuf {
    let work = repo(tmp);
    git(&work, &["checkout", "-q", "-b", "feat"]);
    commit(&work, "src/a.rs", "fn a() { 1 }\n", "a1");
    work
}

#[test]
fn non_push_calls_are_a_plain_exec() {
    let tmp = tempfile::tempdir().unwrap();
    let work = gated(tmp.path());
    let cases: [&[&str]; 7] = [
        &["--version"],
        &["rev-parse", "--show-toplevel"],
        &["status", "--porcelain"],
        &["log", "-1", "--format=%H %s", "--grep", "push"],
        &["-C", ".", "--no-pager", "log", "-1", "--format=%an"],
        &["no-such-command"],
        &["rev-parse", "--verify", "no-such-ref"],
    ];
    for args in cases {
        let real = cmd("git", &work, tmp.path(), &[], args).output().unwrap();
        let via = shim(&work, tmp.path(), &[], args);
        assert_eq!(via.stdout, real.stdout, "stdout of {args:?}");
        assert_eq!(via.stderr, real.stderr, "stderr of {args:?}");
        assert_eq!(via.status.code(), real.status.code(), "exit of {args:?}");
    }
    // stdin passes through.
    let mut c = cmd(SHIM, &work, tmp.path(), &[], &["hash-object", "--stdin"]);
    let mut child = c
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(b"hello\n").unwrap();
    let o = child.wait_with_output().unwrap();
    assert_eq!(
        String::from_utf8_lossy(&o.stdout).trim(),
        "ce013625030ba8dba906f756967f9e9ca394464a"
    );
    // No bd, vega or herdr call, no state or cache written.
    for t in ["bd", "vega", "herdr"] {
        assert_eq!(log(tmp.path(), t), "", "{t} called");
    }
    assert!(!tmp.path().join("cache/vega").exists());
    assert!(!tmp.path().join("state/vega").exists());
}

#[test]
fn a_push_without_a_pass_starts_the_review_and_fails() {
    let tmp = tempfile::tempdir().unwrap();
    let work = gated(tmp.path());
    let o = shim(
        &work,
        tmp.path(),
        &[("HERDR_PANE_ID", "w1:p1")],
        &["push", "-u", "origin", "feat"],
    );
    let e = stderr(&o);
    assert_eq!(o.status.code(), Some(1), "{e}");
    assert!(
        e.contains("vega gate: review running in tab gate-feat (codex m-codex); verdict by mail; then push again"),
        "{e}"
    );
    assert!(e.contains("vega gate: not reviewed: direct "), "{e}");
    assert!(
        e.contains("hint: git push is gated in repos with .vega/gate.toml"),
        "{e}"
    );
    assert_eq!(remote_head(&work, "feat"), None);
    assert!(log(tmp.path(), "vega").contains("mux spawn gate-feat"));

    // The same push again finds the review running and spawns no second one.
    fs::write(
        tmp.path().join("bd-list.json"),
        r#"[{"id":"gate-1","status":"in_progress","metadata":{"verdict":"running"}}]"#,
    )
    .unwrap();
    fs::write(tmp.path().join("herdr-get.json"), r#"{"result":{}}"#).unwrap();
    let o = shim(
        &work,
        tmp.path(),
        &[("HERDR_PANE_ID", "w1:p1")],
        &["push", "origin", "feat"],
    );
    let e = stderr(&o);
    assert_eq!(o.status.code(), Some(1), "{e}");
    assert!(
        e.contains("review running in tab gate-feat (codex m-codex) since"),
        "{e}"
    );
    assert!(!e.contains("hint:"), "trailer once per session: {e}");
    assert_eq!(log(tmp.path(), "vega").matches("mux spawn").count(), 1);
}

#[test]
fn a_recorded_pass_lets_the_push_through() {
    let tmp = tempfile::tempdir().unwrap();
    let work = gated(tmp.path());
    fs::write(
        tmp.path().join("bd-list.json"),
        r#"[{"id":"gate-7","status":"closed","metadata":{"verdict":"pass"}}]"#,
    )
    .unwrap();
    let o = shim(
        &work,
        tmp.path(),
        &[("HERDR_PANE_ID", "w1:p1")],
        &["push", "-q", "origin", "feat"],
    );
    assert!(o.status.success(), "{}", stderr(&o));
    assert!(!stderr(&o).contains("vega gate:"));
    assert_eq!(
        remote_head(&work, "feat"),
        Some(git(&work, &["rev-parse", "HEAD"]))
    );
}

#[test]
fn skip_and_no_herdr_are_recorded_and_let_through() {
    let tmp = tempfile::tempdir().unwrap();
    let work = gated(tmp.path());
    let o = shim(
        &work,
        tmp.path(),
        &[("VEGA_GATE", "skip"), ("VEGA_GATE_REASON", "hotfix")],
        &["push", "-q", "origin", "feat"],
    );
    let e = stderr(&o);
    assert!(o.status.success(), "{e}");
    assert!(
        e.contains("vega gate: skipped (VEGA_GATE=skip), recorded as gate-1"),
        "{e}"
    );
    let bd = log(tmp.path(), "bd");
    assert!(bd.contains("\"verdict\":\"skipped\""), "{bd}");
    assert!(bd.contains("\"reason\":\"hotfix\""), "{bd}");
    assert!(remote_head(&work, "feat").is_some());

    commit(&work, "src/a.rs", "fn a() { 2 }\n", "a2");
    let o = shim(&work, tmp.path(), &[], &["push", "-q", "origin", "feat"]);
    let e = stderr(&o);
    assert!(o.status.success(), "{e}");
    assert!(
        e.contains("vega gate: no herdr, push not reviewed, recorded as gate-2"),
        "{e}"
    );
}

#[test]
fn gate_errors_fail_open_with_one_line() {
    let tmp = tempfile::tempdir().unwrap();
    let work = gated(tmp.path());
    let broken = common::script(
        tmp.path(),
        "broken-bd",
        "echo 'database locked' >&2\nexit 1\n",
    );
    let o = shim(
        &work,
        tmp.path(),
        &[
            ("VEGA_GATE_BD", broken.to_str().unwrap()),
            ("HERDR_PANE_ID", "w1:p1"),
        ],
        &["push", "-q", "origin", "feat"],
    );
    let e = stderr(&o);
    assert!(o.status.success(), "{e}");
    let gate_lines: Vec<&str> = e.lines().filter(|l| l.starts_with("vega gate:")).collect();
    assert_eq!(gate_lines.len(), 1, "{e}");
    assert!(
        gate_lines[0].contains("database locked; push not gated"),
        "{e}"
    );
    assert!(remote_head(&work, "feat").is_some());
}

#[test]
fn ungated_pushes_touch_no_gate() {
    let tmp = tempfile::tempdir().unwrap();
    let work = gated(tmp.path());
    for args in [
        &["push", "--dry-run", "origin", "feat"][..],
        &["push", "-n", "origin", "feat"],
        &["push", "origin", "--delete", "nothing"],
    ] {
        let o = shim(&work, tmp.path(), &[("HERDR_PANE_ID", "w1:p1")], args);
        assert!(
            !stderr(&o).contains("vega gate:"),
            "{args:?}: {}",
            stderr(&o)
        );
    }
    git(&work, &["tag", "v1"]);
    let o = shim(
        &work,
        tmp.path(),
        &[("HERDR_PANE_ID", "w1:p1")],
        &["push", "-q", "origin", "v1"],
    );
    assert!(o.status.success(), "{}", stderr(&o));
    assert!(!stderr(&o).contains("vega gate:"));
    assert_eq!(log(tmp.path(), "bd"), "");
    assert_eq!(remote_head(&work, "feat"), None);

    // No config on the target: no gate.
    let t2 = tempfile::tempdir().unwrap();
    let w2 = repo(t2.path());
    git(&w2, &["rm", "-q", ".vega/gate.toml"]);
    git(&w2, &["commit", "-q", "-m", "no gate"]);
    git(&w2, &["push", "-q", "origin", "main"]);
    commit(&w2, "src/a.rs", "fn a() { 9 }\n", "a9");
    let o = shim(
        &w2,
        tmp.path(),
        &[("HERDR_PANE_ID", "w1:p1")],
        &["push", "-q"],
    );
    assert!(o.status.success(), "{}", stderr(&o));
    assert!(!stderr(&o).contains("vega gate:"));
    assert_eq!(
        remote_head(&w2, "main"),
        Some(git(&w2, &["rev-parse", "HEAD"]))
    );
    assert_eq!(log(tmp.path(), "bd"), "");
}

#[test]
fn a_reviewer_may_not_push() {
    let tmp = tempfile::tempdir().unwrap();
    let work = gated(tmp.path());
    let o = shim(
        &work,
        tmp.path(),
        &[("VEGA_GATE_ROLE", "reviewer")],
        &["push", "origin", "feat"],
    );
    assert_eq!(o.status.code(), Some(1));
    assert!(stderr(&o).contains("vega gate: a gate reviewer does not push"));
    // A cwd inside a gate clone, without the env.
    let clone = tmp.path().join("cache/vega/gate/clones/abc");
    git(
        tmp.path(),
        &[
            "clone",
            "-q",
            "--shared",
            work.to_str().unwrap(),
            clone.to_str().unwrap(),
        ],
    );
    let o = shim(
        &clone,
        tmp.path(),
        &[],
        &["push", "origin", "HEAD:refs/heads/x"],
    );
    assert_eq!(o.status.code(), Some(1));
    assert!(stderr(&o).contains("a gate reviewer does not push"));
    assert_eq!(remote_head(&work, "feat"), None);
}
