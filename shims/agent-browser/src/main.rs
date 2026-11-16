//! Agent-session shim for `agent-browser` (docs/design/browser.md, Shim).
//!
//! One rule: a named call (`--session <name>` anywhere, or a non-empty
//! `AGENT_BROWSER_SESSION`) passes through unchanged, every flag and
//! subcommand allowed, and upserts the owner record
//! (`$XDG_STATE_HOME/vega/browser/sessions.json`). An unnamed call is
//! refused with a hint — a documented deviation from P2: default sessions
//! are the unnamed leftovers nobody can attribute, and `close` without a
//! name would close one of them. Exempt without a name: `--help`,
//! `--version`, `skills …`, `session list`.
//!
//! The shim `exec`s the real binary by absolute path (`VEGA_AGENT_BROWSER_BIN`,
//! then PATH skipping itself), so it never recurses; `vega browser` resolves
//! the real binary the same way.

use std::env;
use std::process::ExitCode;

use vega_browser::{owner, real, session_name};

const TOOL: &str = "agent-browser";

fn main() -> ExitCode {
    let args: Vec<String> = env::args_os()
        .skip(1)
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    if let Some(session) = session_name(&args) {
        // Best effort: a call whose record cannot be written still passes.
        owner::upsert(&session, owner::record_from_env(vega_shim::unix_now()));
        return vega_shim::exec_real(TOOL, Some(real::ENV));
    }
    if exempt(&args) {
        return vega_shim::exec_real(TOOL, Some(real::ENV));
    }
    eprintln!("agent-browser: name your session: --session <name> or AGENT_BROWSER_SESSION=<name>");
    eprintln!("  (e.g. --session <project>-<task>); see `vega skills browser`");
    ExitCode::from(1)
}

/// Calls without a session name that still pass through: help and version,
/// the bundled guides, and the session listing `vega browser ls` relies on.
fn exempt(args: &[String]) -> bool {
    if args
        .iter()
        .any(|a| matches!(a.as_str(), "--help" | "-h" | "--version" | "-V"))
    {
        return true;
    }
    let mut words = args.iter().filter(|a| !a.starts_with('-'));
    match words.next().map(String::as_str) {
        Some("skills") => true,
        Some("session") => words.next().map(String::as_str) == Some("list"),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(s: &str) -> Vec<String> {
        s.split_whitespace().map(String::from).collect()
    }

    #[test]
    fn help_version_guides_and_session_list_pass_without_a_name() {
        assert!(exempt(&args("--help")));
        assert!(exempt(&args("-h")));
        assert!(exempt(&args("--version")));
        assert!(exempt(&args("skills")));
        assert!(exempt(&args("skills get core")));
        assert!(exempt(&args("session list")));
        assert!(exempt(&args("--json session list")));
        assert!(!exempt(&args("session")));
        assert!(!exempt(&args("session info")));
        assert!(!exempt(&args("close")));
        assert!(!exempt(&args("close --all")));
        assert!(!exempt(&args("open https://example.com")));
        assert!(!exempt(&args("--json snapshot -i")));
    }
}
