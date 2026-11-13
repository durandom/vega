//! Agent-session shim for `git` (docs/design/gate.md, section 1).
//!
//! Calls are `exec`ed unchanged (P2), with optional Worktrunk stderr hints
//! for branch/worktree housekeeping in managed repos. A push in a repo with a
//! `.vega/gate.toml` on its target needs a recorded gate pass per segment;
//! without one the shim starts the review (`vega gate start`) and fails the
//! push (exit 1), a documented deviation from P2. The gate fails open on its
//! own errors: one stderr line, then the push. `VEGA_GATE=skip` skips it,
//! recorded; a gate reviewer may not push.

use std::env;
use std::panic;
use std::path::Path;
use std::process::{Command, ExitCode, Stdio};

use vega_gate::push::{self, Decision};

const TOOL: &str = "git";

fn main() -> ExitCode {
    let args: Vec<String> = env::args_os()
        .skip(1)
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    if !push::is_push(&args) {
        worktrunk_hint(&args);
        return vega_shim::exec_real(TOOL, None);
    }
    if let Some(code) = gate(&args) {
        return code;
    }
    vega_shim::exec_real(TOOL, None)
}

/// Only matching commands pay for a repository probe. Failures stay silent;
/// the real command still owns argv, stdin, stdout and the exit status.
fn worktrunk_hint(args: &[String]) {
    let Some(sub) = push::subcommand(args) else {
        return;
    };
    let rest = &args[sub + 1..];
    let options = || {
        let mut value = false;
        rest.iter()
            .take_while(|a| a.as_str() != "--")
            .filter(move |a| {
                if std::mem::take(&mut value) {
                    return false;
                }
                // These values may themselves look like the flags we hint on.
                value = matches!(
                    a.as_str(),
                    "--format"
                        | "--sort"
                        | "--set-upstream-to"
                        | "-u"
                        | "--conflict"
                        | "--pathspec-from-file"
                );
                true
            })
    };
    let hint = match args[sub].as_str() {
        "branch" if options().any(|a| a == "--delete" || short_flag(a, 'd')) => {
            "wt remove <branch> removes merged branches and their worktrees (including squash merges)"
        }
        "worktree" => match rest.first().map(String::as_str) {
            Some("add") => {
                "wt switch <branch> creates a worktree for an existing branch; wt switch -c <branch> creates a new branch and worktree with project hooks"
            }
            Some("remove" | "prune") => {
                "wt remove <branch-or-path> removes a worktree and its merged branch; use it for worktree cleanup"
            }
            _ => return,
        },
        "checkout" if options().any(|a| a.starts_with("-b")) => {
            "for parallel work, wt switch -c <branch> --base=@ creates a branch and worktree from HEAD with project hooks"
        }
        "switch"
            if options().any(|a| {
                a == "--create" || a.starts_with("--create=") || a.starts_with("-c")
            }) =>
        {
            "for parallel work, wt switch -c <branch> --base=@ creates a branch and worktree from HEAD with project hooks"
        }
        _ => return,
    };
    let me = env::current_exe().ok().and_then(|p| p.canonicalize().ok());
    let Some(real) = vega_shim::resolve_real(TOOL, None, me.as_deref()) else {
        return;
    };
    let Ok(output) = Command::new(real)
        // Keep non-UTF8 global option values intact, just like exec_real.
        .args(env::args_os().skip(1).take(sub))
        .args([
            "rev-parse",
            "--path-format=absolute",
            "--show-toplevel",
            "--git-common-dir",
        ])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
    else {
        return;
    };
    if !output.status.success() {
        return;
    }
    let Ok(paths) = std::str::from_utf8(&output.stdout) else {
        return;
    };
    let mut paths = paths.lines();
    let (Some(root), Some(common)) = (paths.next(), paths.next()) else {
        return;
    };
    if Path::new(root).join(".config/wt.toml").is_file() || Path::new(common).join("wt").is_dir() {
        eprintln!("hint: {hint}");
    }
}

fn short_flag(arg: &str, flag: char) -> bool {
    arg.starts_with('-') && !arg.starts_with("--") && arg[1..].contains(flag)
}

/// The push gate; `Some(exit code)` blocks the push, `None` lets it run.
fn gate(args: &[String]) -> Option<ExitCode> {
    // The gate's own git calls go to the real git, not through this shim.
    let me = env::current_exe().ok().and_then(|p| p.canonicalize().ok());
    let real = vega_shim::resolve_real(TOOL, None, me.as_deref())?;
    env::set_var("VEGA_GATE_GIT", real);
    if vega_shim::first_time(vega_shim::session_id().as_deref(), "git-push-gate") {
        eprintln!("hint: git push is gated in repos with .vega/gate.toml; vega skills gate");
    }
    let cwd = env::current_dir().unwrap_or_default();
    // A gate bug must never block work: a panic fails open like an error.
    panic::set_hook(Box::new(|_| {}));
    let decision = panic::catch_unwind(|| push::decide(args, &cwd))
        .unwrap_or_else(|_| Err("internal error".into()));
    match decision {
        Ok(Decision::Exec) => None,
        Ok(Decision::Note(m)) => {
            eprintln!("vega gate: {m}");
            None
        }
        Ok(Decision::Block(m)) => {
            eprintln!("vega gate: {m}");
            Some(ExitCode::from(1))
        }
        Err(e) => {
            eprintln!("vega gate: {e}; push not gated");
            None
        }
    }
}
