//! Fixture repos and fake tools for the gate's tests.
#![allow(dead_code)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Once, OnceLock};

pub const CONFIG: &str = r#"timeout = "20m"
tier = "budget"
reviewers = ["codex", "pi:xai", "claude"]

[[review]]
name = "code"
paths = ["src/**"]
exclude = ["**/fixtures/**"]
rubric = "rubric"
"#;

/// git without the user's global config (hooks, signing, aliases).
pub fn git_env() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let empty = std::env::temp_dir().join("vega-gate-test-empty-gitconfig");
        fs::write(&empty, "").unwrap();
        for (k, v) in [
            ("GIT_AUTHOR_NAME", "t"),
            ("GIT_AUTHOR_EMAIL", "t@t"),
            ("GIT_COMMITTER_NAME", "t"),
            ("GIT_COMMITTER_EMAIL", "t@t"),
            ("GIT_CONFIG_NOSYSTEM", "1"),
        ] {
            std::env::set_var(k, v);
        }
        std::env::set_var("GIT_CONFIG_GLOBAL", &empty);
    });
}

pub fn git(dir: &Path, args: &[&str]) -> String {
    git_env();
    let o = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        o.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&o.stderr)
    );
    String::from_utf8_lossy(&o.stdout).trim().to_string()
}

pub fn write(dir: &Path, file: &str, body: &str) {
    let p = dir.join(file);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, body).unwrap();
}

pub fn commit(dir: &Path, file: &str, body: &str, msg: &str) -> String {
    write(dir, file, body);
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-q", "-m", msg]);
    git(dir, &["rev-parse", "HEAD"])
}

/// A bare origin and a clone `work` on main with the gate config, a rubric,
/// a source file and `.beads`, pushed. Returns (tmp root, work dir).
pub fn repo(tmp: &Path) -> PathBuf {
    let origin = tmp.join("origin.git");
    let work = tmp.join("work");
    git(
        tmp,
        &[
            "init",
            "-q",
            "--bare",
            "-b",
            "main",
            origin.to_str().unwrap(),
        ],
    );
    git(
        tmp,
        &[
            "clone",
            "-q",
            origin.to_str().unwrap(),
            work.to_str().unwrap(),
        ],
    );
    git(&work, &["checkout", "-q", "-b", "main"]);
    write(&work, ".vega/gate.toml", CONFIG);
    write(&work, "rubric/SKILL.md", "# rubric v1\n");
    write(&work, "src/a.rs", "fn a() {}\n");
    write(&work, "docs/x.md", "doc\n");
    fs::create_dir_all(work.join(".beads")).unwrap();
    write(&work, ".gitignore", ".beads/\n");
    git(&work, &["add", "-A"]);
    git(&work, &["commit", "-q", "-m", "init"]);
    git(&work, &["push", "-q", "-u", "origin", "main"]);
    git(&work, &["remote", "set-head", "origin", "main"]);
    work
}

/// Create once and warm before any budgeted invocation. macOS assesses newly
/// written executables on first exec; that latency is not fake-tool work.
pub fn script(dir: &Path, name: &str, body: &str) -> PathBuf {
    let p = dir.join(name);
    if !p.exists() {
        fs::write(
            &p,
            format!("#!/bin/sh\n[ \"$1\" = --vega-test-warmup ] && exit 0\n{body}"),
        )
        .unwrap();
        fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
        let status = warm_with(|| Command::new(&p).arg("--vega-test-warmup").status())
            .unwrap_or_else(|e| panic!("warm {}: {e}", p.display()));
        assert!(status.success(), "warm {}: {status}", p.display());
    }
    p
}

/// fs::write closes our writer before exec, but on Linux another thread's
/// fork may have inherited it until that child execs (rust-lang/rust#114554).
/// Retry only this transient spawn error, before any budgeted fixture work.
/// A persistent writer or any other error still fails the test.
pub fn warm_with(
    run: impl FnMut() -> std::io::Result<std::process::ExitStatus>,
) -> std::io::Result<std::process::ExitStatus> {
    warm_with_backoff(std::time::Duration::from_millis(20), run)
}

/// `warm_with` with the pause between attempts given (tests of the retry use zero).
pub fn warm_with_backoff(
    backoff: std::time::Duration,
    mut run: impl FnMut() -> std::io::Result<std::process::ExitStatus>,
) -> std::io::Result<std::process::ExitStatus> {
    for attempt in 0..50 {
        match run() {
            Err(e) if e.kind() == std::io::ErrorKind::ExecutableFileBusy && attempt < 49 => {
                std::thread::sleep(backoff);
            }
            result => return result,
        }
    }
    unreachable!()
}

/// Like `script`, in one directory shared by every test and test binary.
/// macOS assesses each new executable on its first exec, seconds per script
/// on a loaded host, so the fakes are written once and only rewritten when
/// their text changes (atomically: test binaries may run in parallel).
pub fn shared_script(dir: &str, name: &str, body: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(dir);
    fs::create_dir_all(&dir).unwrap();
    let p = dir.join(name);
    let text = format!("#!/bin/sh\n[ \"$1\" = --vega-test-warmup ] && exit 0\n{body}");
    if fs::read_to_string(&p).ok().as_deref() != Some(text.as_str()) {
        let tmp = dir.join(format!(".{name}.{}", std::process::id()));
        fs::write(&tmp, &text).unwrap();
        fs::set_permissions(&tmp, fs::Permissions::from_mode(0o755)).unwrap();
        fs::rename(&tmp, &p).unwrap();
        let status = warm_with(|| Command::new(&p).arg("--vega-test-warmup").status())
            .unwrap_or_else(|e| panic!("warm {}: {e}", p.display()));
        assert!(status.success(), "warm {}: {status}", p.display());
    }
    p
}

/// Fake bd, vega and herdr writing their argv to `<log>/<tool>.log`, where
/// `<log>` is `dir`, passed as `FAKE_LOG`.
/// bd list answers `<log>/bd-list.json` (else `[]`); bd create numbers ids;
/// vega pins `m-<lane>` and spawns fresh panes starting at w9:p5; a
/// `<log>/spawn-fail-<kind>` file makes that kind's spawn fail; fake harness binaries
/// sit in a shared dir (`VEGA_GATE_HARNESS_PATH`); herdr's `pane get` answers
/// `<log>/herdr-get.json`.
pub fn fakes(dir: &Path) -> Vec<(String, PathBuf)> {
    static TOOLS: OnceLock<Vec<(String, PathBuf)>> = OnceLock::new();
    let mut env = vec![("FAKE_LOG".to_string(), dir.to_path_buf())];
    env.extend(TOOLS.get_or_init(fake_tools).iter().cloned());
    env
}

fn fake_tools() -> Vec<(String, PathBuf)> {
    let bd = shared_script(
        "gate-fakes",
        "bd",
        r#"L=${FAKE_LOG:?}
printf '%s\n' "$*" >> $L/bd.log
if [ -f $L/fail-supersede ] && [ "$1" = update ] && [ "$4" = verdict=superseded ]; then
  echo "injected storage failure" >&2
  exit 1
fi
case "$1" in
  list) if [ -f "$L/bd-key-$3.json" ]; then cat "$L/bd-key-$3.json"; else cat $L/bd-list.json 2>/dev/null || echo '[]'; fi ;;
  create) n=$(( $(cat $L/bd-n 2>/dev/null || echo 0) + 1 )); echo $n > $L/bd-n; echo "gate-$n" ;;
  show) echo '[{"status":"closed"}]' ;;
esac
"#,
    );
    let vega = shared_script(
        "gate-fakes",
        "vega",
        r#"L=${FAKE_LOG:?}
printf '%s\n' "$*" >> $L/vega.log
case "$1 $2" in
  "lanista pin") echo "model: m-$3" ;;
  "post send") if [ -f $L/block-mail ]; then
    touch $L/mail-entered
    n=0
    while [ ! -f $L/release-mail ]; do
      n=$((n + 1)); [ "$n" -lt 500 ] || exit 1
      sleep 0.01
    done
  fi ;;
  "mux spawn") if [ -f $L/old-pane-alive ]; then echo "error: name_taken"; exit 1; fi; if [ -f $L/spawn-fail-$5 ]; then printf "error{code,message,hint}:\n  timeout,\"last output: zsh: command not found: $5\",\"check --kind\"\n"; exit 1; fi; n=$(( $(cat $L/spawn-n 2>/dev/null || echo 4) + 1 )); echo $n > $L/spawn-n; echo "spawned: x (codex)"; echo "pane: w9:p$n" ;;
esac
"#,
    );
    let herdr = shared_script(
        "gate-fakes",
        "herdr",
        r#"L=${FAKE_LOG:?}
printf '%s\n' "$*" >> $L/herdr.log
case "$1 $2" in
  "pane current") echo '{"result":{"pane":{"agent":"claude","agent_session":{"value":"s-author"},"cwd":"/nonexistent"}}}' ;;
  "pane close") rm -f $L/old-pane-alive ;;
  "pane get") if [ -f $L/revoke-state ]; then rm -f "$(cat $L/revoke-state)"; fi; cat $L/herdr-get.json 2>/dev/null || exit 1 ;;
esac
"#,
    );
    let mut harness = PathBuf::new();
    for kind in ["codex", "claude", "pi", "agy"] {
        harness = shared_script("gate-harness", kind, "exit 0\n");
    }
    vec![
        ("VEGA_GATE_BD".into(), bd),
        (
            "VEGA_GATE_HARNESS_PATH".into(),
            harness.parent().unwrap().into(),
        ),
        ("VEGA_GATE_VEGA".into(), vega),
        ("VEGA_GATE_HERDR".into(), herdr),
    ]
}

pub fn log(dir: &Path, tool: &str) -> String {
    fs::read_to_string(dir.join(format!("{tool}.log"))).unwrap_or_default()
}
