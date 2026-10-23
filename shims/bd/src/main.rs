//! Agent-session shim for `bd`: per-worktree claim identity
//! (docs/design/beads-sync.md, Claim identity; moved from the dotfiles'
//! `~/.local/libexec/bd-shim/bd`, spellkave sk-jay0.7.24).
//!
//! bd's claim is a compare-and-swap on `assignee`: a second claimant only
//! loses the race with a different actor string. bd's default actor is
//! `git config user.name`, the same in every worktree on every machine, so
//! the lock matched itself. The shim sets `BEADS_ACTOR=<host>/<worktree dir>`
//! from the directory bd runs in (not the one a shell started in), then
//! execs the real bd: stdout, stdin and the exit code are bd's own (P2).
//!
//! Deviations from P2 (docs/design/beads-sync.md): recursive invocation or
//! inability to identify this executable exits 126 before helpers or bd run.
//! Run the original bd by absolute path when identification fails; for
//! recursive wrappers, set BD_REAL to the original binary on the next call.
//! The manual push and pull guard is another deviation:
//! in a project vegad syncs (`vega bd init`), `bd dolt push|pull` by hand
//! races vegad, so the shim skips it with exit 0 and one stderr line; while
//! the project's sync is stopped it exits 1 with the reason (the UAC
//! resolves it). `VEGA_BD_MANUAL=1` and unprovisioned projects pass through.
//!
//! Hints (stderr only, docs/design/git-sync.md): before bd runs, one line
//! each when the project's beads sync is stopped, when this main checkout is
//! behind with vegad's fast-forward skipped, or when this worktree is behind
//! the upstream.
//!
//! Escape hatches: a caller-set `BEADS_ACTOR` wins (nothing is derived), an
//! explicit `--actor` beats both (bd's own precedence), and `BD_REAL` names
//! the real binary. Known gap: `bd -C <dir>` still derives the actor from the
//! current directory.

use std::env;
use std::io::{Read, Seek, SeekFrom};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::time::{Duration, Instant};

use vega_bd_sync::{store, Project, State};

const REENTRY: &str = "VEGA_BD_SHIM_ACTIVE";
const PROBE_LIMIT: Duration = Duration::from_secs(2);

fn main() -> ExitCode {
    // Before actor derivation or hints: even a copied shim/wrapper must stop.
    if env::var_os(REENTRY).is_some() {
        eprintln!("bd shim: recursive invocation blocked; set BD_REAL to the original bd binary");
        return ExitCode::from(126);
    }
    let Ok(me) = env::current_exe().and_then(|p| p.canonicalize()) else {
        eprintln!("bd shim: cannot identify this executable; invoke the original bd directly by absolute path");
        return ExitCode::from(126);
    };
    env::set_var(REENTRY, "1");
    env::set_var("PATH", vega_shim::path_without_self("bd", &me));
    if env::var_os("BEADS_ACTOR").is_none_or(|v| v.is_empty()) {
        let host = output("hostname", &["-s"]).unwrap_or_else(|| "unknown".into());
        let top = output("git", &["rev-parse", "--show-toplevel"]);
        // Edition 2021: set_var is safe; nothing else runs threads here.
        env::set_var("BEADS_ACTOR", actor(&host, top.as_deref()));
    }
    let args: Vec<String> = env::args_os()
        .skip(1)
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    let manual = env::var_os("VEGA_BD_MANUAL").is_some_and(|v| v == "1");
    if let Some((verb, dir)) = dolt_sync(&args).filter(|_| !manual) {
        let known = match synced(dir.as_deref()) {
            Ok(known) => known,
            Err(e) => {
                eprintln!("error: {}: {}; {}", e.code, e.message, e.hint);
                return ExitCode::from(1);
            }
        };
        match decide(verb, known) {
            Guard::Synced(msg) => {
                eprintln!("{msg}");
                return ExitCode::SUCCESS;
            }
            Guard::Stopped(msg) => {
                eprintln!("{msg}");
                return ExitCode::from(1);
            }
            Guard::Pass => {}
        }
    }
    for line in hints() {
        eprintln!("{line}");
    }
    vega_shim::exec_real("bd", Some("BD_REAL"))
}

/// stderr lines about the sync state of the checkout bd runs in
/// (docs/design/git-sync.md, Telling agents): its beads sync stopped, its
/// main checkout behind with the fast-forward skipped, or its worktree
/// behind the upstream. Nothing outside a known project.
fn hints() -> Vec<String> {
    let Ok(cwd) = env::current_dir() else {
        return Vec::new();
    };
    let Ok(resolved) = vega_herdr::project::resolve(&cwd, &vega_herdr::state::post_host()) else {
        return Vec::new();
    };
    let key = resolved.id.to_string();
    let common = resolved.location.common_dir;
    let top = resolved.location.checkout;
    let beads = store::project(&key, &common).map(|p| {
        let s = store::state(&key, &common);
        (p, s)
    });
    sync_hints(
        &key,
        beads,
        &vega_bd_sync::git::state(&key, &common),
        &top,
        vega_bd_sync::unix_now(),
    )
}

fn sync_hints(
    key: &str,
    beads: Option<(Project, State)>,
    git: &vega_bd_sync::git::GitState,
    top: &Path,
    now: i64,
) -> Vec<String> {
    let mut out = Vec::new();
    if let Some((p, s)) = beads.filter(|(p, s)| s.is_stopped(p)) {
        out.push(format!(
            "vega: beads sync of {} is stopped ({}); writes stay local until the UAC resolves it (vega bd resume). State: vega bd status",
            p.key,
            s.stop_reason.as_deref().unwrap_or("stopped")
        ));
    }
    out.extend(vega_bd_sync::git::hint(key, git, top, now));
    out
}

/// `<host>/<worktree dir>`, or `<host>` outside a git checkout.
fn actor(host: &str, toplevel: Option<&str>) -> String {
    let host = if host.is_empty() { "unknown" } else { host };
    match toplevel
        .and_then(|t| t.rsplit('/').next())
        .filter(|n| !n.is_empty())
    {
        Some(name) => format!("{host}/{name}"),
        None => host.to_string(),
    }
}

/// bd's global flags that take a value (`--actor x` or `--actor=x`).
const VALUE_FLAGS: [&str; 7] = [
    "--actor",
    "--database",
    "--db",
    "-C",
    "--directory",
    "--dolt-auto-commit",
    "--mem-profile",
];

/// `push` or `pull` for `bd [flags] dolt push|pull ...`, with the `-C` dir if
/// any. Not for `--help`, nor for `--global`, `--db` or `--database`
/// (another database than the project's).
fn dolt_sync(args: &[String]) -> Option<(&'static str, Option<PathBuf>)> {
    let mut words = Vec::new();
    let mut dir = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let (flag, inline) = match a.split_once('=') {
            Some((f, v)) if f.starts_with('-') => (f, Some(v.to_string())),
            _ => (a.as_str(), None),
        };
        if matches!(flag, "-h" | "--help" | "--global" | "--db" | "--database") {
            return None;
        }
        if VALUE_FLAGS.contains(&flag) {
            let value = inline.or_else(|| it.next().cloned());
            if matches!(flag, "-C" | "--directory") {
                dir = value.map(PathBuf::from);
            }
        } else if !a.starts_with('-') {
            words.push(a.as_str());
        }
    }
    let verb = match words.as_slice() {
        ["dolt", "push", ..] => "push",
        ["dolt", "pull", ..] => "pull",
        _ => return None,
    };
    Some((verb, dir))
}

/// The provisioned project of `dir` (else the current dir) and its sync
/// state; `None` when vegad does not sync it.
fn synced(dir: Option<&Path>) -> Result<Option<(Project, State)>, vega_herdr::Failure> {
    let Ok(cwd) = env::current_dir() else {
        return Ok(None);
    };
    let dir = dir.map_or(cwd.clone(), |d| cwd.join(d));
    let resolved = match vega_herdr::project::resolve(&dir, &vega_herdr::state::post_host()) {
        Ok(resolved) => resolved,
        Err(_) => return Ok(None), // preserve the real bd boundary when identity is unavailable
    };
    let key = resolved.id.as_str();
    let common = resolved.location.common_dir;
    let Some(p) = store::project(key, &common) else {
        if store::projects().iter().any(|p| p.common == common) {
            return Err(vega_herdr::Failure::new(
                "project_identity_changed",
                "A provisioned location changed project identity",
                "quiesce writers and perform the journaled rekey",
            ));
        }
        return Ok(None);
    };
    let s = store::state(key, &common);
    Ok(Some((p, s)))
}

#[derive(Debug, PartialEq)]
enum Guard {
    /// vegad syncs the project: skip, exit 0.
    Synced(String),
    /// Its sync is stopped: exit 1.
    Stopped(String),
    Pass,
}

fn decide(verb: &str, synced: Option<(Project, State)>) -> Guard {
    let Some((p, s)) = synced else {
        return Guard::Pass;
    };
    if s.is_stopped(&p) {
        let reason = s.stop_reason.as_deref().unwrap_or("stopped");
        return Guard::Stopped(format!(
            "error: bd dolt {verb} skipped: vegad's sync of {} is stopped ({reason}); the UAC resolves it (vega bd resume), do not {verb} by hand. State: vega bd status",
            p.key
        ));
    }
    Guard::Synced(format!(
        "bd dolt {verb} skipped: vegad syncs {} (pushes within ~5 s of a write, pulls every {} s). State: vega bd status",
        p.key,
        vega_bd_sync::sync::PULL_EVERY
    ))
}

fn output(cmd: &str, args: &[&str]) -> Option<String> {
    let mut command = Command::new(cmd);
    command.args(args);
    probe(command, PROBE_LIMIT)
}

/// Best-effort metadata only: never bound the user's actual bd invocation.
/// A file avoids waiting for pipe EOF from grandchildren; a separate process
/// group lets a timeout kill helpers as well as their immediate parent.
fn probe(mut cmd: Command, limit: Duration) -> Option<String> {
    let mut capture = tempfile::tempfile().ok()?;
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(capture.try_clone().ok()?)
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .ok()?;
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                if !status.success() {
                    return None;
                }
                break;
            }
            Ok(None) if start.elapsed() < limit => {
                std::thread::sleep(Duration::from_millis(10));
            }
            _ => {
                // SAFETY: the child leads its own process group; a negative
                // pid addresses only that group, never the caller's group.
                unsafe { libc::kill(-(child.id() as i32), libc::SIGKILL) };
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    capture.seek(SeekFrom::Start(0)).ok()?;
    let mut buf = String::new();
    capture.take(64 * 1024).read_to_string(&mut buf).ok()?;
    let s = buf.trim().to_string();
    (!s.is_empty()).then_some(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probes_are_bounded_and_kill_descendants() {
        let tmp = tempfile::tempdir().unwrap();
        let marker = tmp.path().join("survived");
        let mut cmd = Command::new("/bin/sh");
        cmd.args(["-c", "(sleep 0.3; echo survived > \"$1\") & wait", "probe"])
            .arg(&marker);
        let start = Instant::now();
        assert!(probe(cmd, Duration::from_millis(50)).is_none());
        assert!(start.elapsed() < Duration::from_secs(1));
        std::thread::sleep(Duration::from_millis(400));
        assert!(!marker.exists(), "probe grandchild survived timeout");
    }

    #[test]
    fn actor_is_host_and_worktree_dir() {
        assert_eq!(
            actor("proteus", Some("/Users/m/src/vega.bd-sync")),
            "proteus/vega.bd-sync"
        );
        assert_eq!(actor("proteus", None), "proteus");
        assert_eq!(actor("", Some("/x/y")), "unknown/y");
    }

    fn args(s: &str) -> Vec<String> {
        s.split_whitespace().map(String::from).collect()
    }

    #[test]
    fn dolt_push_and_pull_are_found_past_global_flags() {
        assert_eq!(dolt_sync(&args("dolt push")), Some(("push", None)));
        assert_eq!(dolt_sync(&args("dolt pull --json")), Some(("pull", None)));
        assert_eq!(
            dolt_sync(&args("--actor x -C ../w dolt push")),
            Some(("push", Some("../w".into())))
        );
        assert_eq!(
            dolt_sync(&args("--directory=/w dolt pull")),
            Some(("pull", Some("/w".into())))
        );
        for pass in [
            "dolt push --help",
            "dolt commit",
            "dolt remote list",
            "list",
            "--global dolt push",
            "--db=/x dolt pull",
            "create --title dolt push",
            "",
        ] {
            assert_eq!(dolt_sync(&args(pass)), None, "{pass}");
        }
    }

    fn project() -> Project {
        Project {
            key: "github.com/team/x".into(),
            common: "/x/.git".into(),
            dir: "/x".into(),
            database: "X".into(),
            remote: "http://r/X".into(),
            bd: "/bin/bd".into(),
            data: "/x/.beads/dolt".into(),
            since: 1,
            resumed: 0,
        }
    }

    #[test]
    fn synced_skips_stopped_fails_unprovisioned_passes() {
        assert_eq!(decide("push", None), Guard::Pass);
        let ok = decide("push", Some((project(), State::default())));
        assert!(
            matches!(&ok, Guard::Synced(m) if m.contains("vega bd status") && !m.contains('\n'))
        );
        let stopped = State {
            stopped: Some(5),
            stop_reason: Some("push rejected: behind".into()),
            ..State::default()
        };
        assert!(matches!(
            decide("pull", Some((project(), stopped.clone()))),
            Guard::Stopped(m) if m.contains("push rejected: behind") && m.contains("UAC")
        ));
        // Resumed after the stop: synced again.
        let mut p = project();
        p.resumed = 6;
        assert!(matches!(
            decide("pull", Some((p, stopped))),
            Guard::Synced(_)
        ));
    }

    #[test]
    fn hints_name_a_stopped_sync_and_a_lagging_worktree() {
        use vega_bd_sync::git::{GitState, Lag};
        let none = sync_hints("k", None, &GitState::default(), Path::new("/w"), 100);
        assert!(none.is_empty());
        let stopped = State {
            stopped: Some(5),
            stop_reason: Some("pull conflict".into()),
            ..State::default()
        };
        let git = GitState {
            fetched: 90,
            branch: Some("main".into()),
            worktrees: vec![Lag {
                dir: "/nonexistent/w".into(),
                branch: Some("fix".into()),
                behind: 3,
            }],
            ..GitState::default()
        };
        let h = sync_hints(
            "k",
            Some((project(), stopped)),
            &git,
            Path::new("/nonexistent/w"),
            100,
        );
        assert_eq!(h.len(), 2, "{h:?}");
        assert!(h[0].contains("pull conflict") && h[0].contains("vega bd resume"));
        assert!(h[1].contains("3 commit(s) behind origin/main"));
        assert!(h.iter().all(|l| !l.contains('\n')));
    }

    // The only test that touches the env (XDG_STATE_HOME is process-wide).
    #[test]
    fn synced_finds_the_project_of_a_checkout() {
        let tmp = tempfile::tempdir().unwrap();
        env::set_var("XDG_STATE_HOME", tmp.path().join("state"));
        let repo = tmp.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let init = Command::new("git")
            .args(["init", "-q"])
            .current_dir(&repo)
            .status()
            .unwrap();
        assert!(init.success());
        assert_eq!(synced(Some(&repo)).unwrap(), None);
        let resolved =
            vega_herdr::project::resolve(&repo, &vega_herdr::state::post_host()).unwrap();
        let key = resolved.id.to_string();
        let p = Project {
            key: key.clone(),
            common: resolved.location.common_dir,
            ..project()
        };
        assert!(store::save_project(&p));
        assert_eq!(synced(Some(&repo)).unwrap().map(|(p, _)| p.key), Some(key));
    }
}
