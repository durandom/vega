//! Agent-session shim for `git` (docs/design/gate.md, section 1).
//!
//! Every call but `git push` is `exec`ed at once (P2): the argv check is
//! pure, no config read, no bd call. A push in a repo with a
//! `.vega/gate.toml` on its target needs a recorded gate pass per segment;
//! without one the shim starts the review (`vega gate start`) and fails the
//! push (exit 1), a documented deviation from P2. The gate fails open on its
//! own errors: one stderr line, then the push. `VEGA_GATE=skip` skips it,
//! recorded; a gate reviewer may not push.

use std::env;
use std::panic;
use std::process::ExitCode;

use vega_gate::push::{self, Decision};

const TOOL: &str = "git";

fn main() -> ExitCode {
    let args: Vec<String> = env::args_os()
        .skip(1)
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    if !push::is_push(&args) {
        return vega_shim::exec_real(TOOL, None);
    }
    if let Some(code) = gate(&args) {
        return code;
    }
    vega_shim::exec_real(TOOL, None)
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
