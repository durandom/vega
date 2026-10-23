//! Agent-session shim for `tea`, the Forgejo/Gitea CLI (docs/design/ci-watch.md,
//! tea shim).
//!
//! Every call passes through to the real `tea` by `exec` (P2); nothing is
//! denied, as no polling loop is known for tea yet. The shim counts calls per
//! pane for `vega uac status` and hints once per session at `vega ci watch`
//! when a session polls pulls or the API by hand.

use std::env;
use std::process::ExitCode;

use vega_shim::calls;

const TOOL: &str = "tea";
/// The third poll within this window gets the hint.
const POLL_WINDOW: i64 = 600;
const POLLS_BEFORE_HINT: usize = 3;
/// Commands agents repeat in a loop to wait for CI.
const POLLS: [&str; 2] = ["pulls", "api"];

fn main() -> ExitCode {
    let args: Vec<String> = env::args_os()
        .skip(1)
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    let pane = vega_shim::pane_key();
    let now = vega_shim::unix_now();
    let cmd = command(&args);
    if let Some(c) = &cmd {
        calls::record(TOOL, &pane, c, now);
    }
    let polls = |c: &str| calls::count_of(TOOL, &pane, c, POLL_WINDOW, now);
    if let Some((key, msg)) = hint(&args, cmd.as_deref(), polls) {
        if vega_shim::first_time(vega_shim::session_id().as_deref(), key) {
            eprintln!("{msg}");
        }
    }
    if vega_shim::resolve_real(TOOL, None, self_path().as_deref()).is_none() {
        // tea is optional (only Forgejo/Gitea hosts need it): say how to get it.
        eprintln!("{INSTALL_HINT}");
    }
    vega_shim::exec_real(TOOL, None)
}

const INSTALL_HINT: &str = "hint: tea is not installed; Forgejo/Gitea hosts need it: brew install tea, or mise use -g go:gitea.dev/tea@0.16.0 (needs Go), then tea login add";

fn self_path() -> Option<std::path::PathBuf> {
    env::current_exe().ok().and_then(|p| p.canonicalize().ok())
}

/// The first word, with tea's aliases of `pulls` (`pull`, `pr`) folded in.
/// Only the command, never its arguments, reaches the call log.
fn command(args: &[String]) -> Option<String> {
    let first = args.iter().find(|a| !a.starts_with('-'))?;
    Some(match first.as_str() {
        "pull" | "pr" => "pulls".into(),
        c => c.into(),
    })
}

fn hint(
    args: &[String],
    cmd: Option<&str>,
    polls: impl Fn(&str) -> usize,
) -> Option<(&'static str, String)> {
    if args.iter().any(|a| a == "--help" || a == "-h") {
        return None;
    }
    let c = cmd.filter(|c| POLLS.contains(c))?;
    (polls(c) >= POLLS_BEFORE_HINT).then(|| {
        (
            "tea-poll",
            format!(
                "hint: waiting for CI? `vega ci watch [<pr|branch>]` mails you the result (green, red, failed jobs); do not poll `tea {c}`"
            ),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(s: &str) -> Vec<String> {
        s.split_whitespace().map(String::from).collect()
    }

    fn hint_str(s: &str, polls: usize) -> Option<String> {
        let a = args(s);
        hint(&a, command(&a).as_deref(), |_| polls).map(|(_, m)| m)
    }

    #[test]
    fn command_is_the_first_word_with_aliases_folded() {
        assert_eq!(command(&args("pulls list")).as_deref(), Some("pulls"));
        assert_eq!(command(&args("pr 12")).as_deref(), Some("pulls"));
        assert_eq!(command(&args("api /repos/o/r")).as_deref(), Some("api"));
        assert_eq!(command(&args("--version")), None);
    }

    #[test]
    fn repeated_polls_get_the_hint() {
        assert_eq!(hint_str("pulls list", 2), None);
        assert!(hint_str("pulls list", 3).is_some_and(|m| m.contains("vega ci watch")));
        assert!(hint_str("api /repos/o/r/commits/x/status", 4).is_some());
        assert_eq!(hint_str("api --help", 9), None);
        assert_eq!(hint_str("issues list", 9), None);
    }
}
