//! Agent-session shim for `gh` (docs/design/ci-watch.md, gh shim).
//!
//! Every call passes through to the real `gh` by `exec` (P2), except the two
//! watch loops, `gh run watch` and `gh pr checks --watch`: they poll every 3 s
//! and 10 s and drain the rate limit that every agent and CI job of the user
//! shares. The shim denies them with a pointer to `vega ci watch` (a
//! documented deviation from P2). It counts calls per pane for `vega uac
//! status` and hints once per session at `vega ci watch` when a session polls
//! checks by hand, and at `vega ci watch --run` after `gh workflow run`.

use std::env;
use std::process::ExitCode;

use vega_shim::calls;

const TOOL: &str = "gh";
/// The third poll within this window gets the hint.
const POLL_WINDOW: i64 = 600;
const POLLS_BEFORE_HINT: usize = 3;
/// One-shot commands agents repeat in a loop to wait for CI.
const POLLS: [&str; 3] = ["pr checks", "run view", "run list"];

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
    match decide(&args, cmd.as_deref(), polls) {
        Action::Deny(msg) => {
            eprintln!("{msg}");
            return ExitCode::from(1);
        }
        Action::Hint(key, msg) => {
            if vega_shim::first_time(vega_shim::session_id().as_deref(), key) {
                eprintln!("{msg}");
            }
        }
        Action::Pass => {}
    }
    vega_shim::exec_real(TOOL, None)
}

#[derive(Debug, PartialEq)]
enum Action {
    Deny(String),
    Hint(&'static str, String),
    Pass,
}

/// The first two words when both are subcommand names (`pr checks`), else
/// the first (`api`). gh takes no global flags before the command.
fn command(args: &[String]) -> Option<String> {
    let mut words = args.iter().take_while(|a| !a.starts_with('-'));
    let first = words.next()?;
    Some(match words.next() {
        Some(second) if first != "api" => format!("{first} {second}"),
        _ => first.clone(),
    })
}

fn decide(args: &[String], cmd: Option<&str>, polls: impl Fn(&str) -> usize) -> Action {
    let help = args.iter().any(|a| a == "--help" || a == "-h");
    match cmd {
        _ if help => Action::Pass,
        Some("run watch") => Action::Deny(deny("gh run watch", "3 s")),
        Some("pr checks") if watches(args) => Action::Deny(deny("gh pr checks --watch", "10 s")),
        Some("workflow run") => Action::Hint(
            "gh-dispatch",
            "hint: waiting for this run? `vega ci watch --run <id>` (the id ends the run URL gh prints) or `vega ci watch --workflow <file> --ref <ref>` mails you its verdict; a PR or branch watch mails once per head and can miss it".into(),
        ),
        Some(c) if POLLS.contains(&c) && polls(c) >= POLLS_BEFORE_HINT => Action::Hint(
            "gh-poll",
            format!(
                "hint: waiting for CI? `vega ci watch [<pr|branch>]` mails you the result (green, red, failed jobs); do not poll `gh {c}`"
            ),
        ),
        _ => Action::Pass,
    }
}

/// `--watch`, or `--watch=<true>`; `--watch=false` is no watch.
fn watches(args: &[String]) -> bool {
    args.iter().any(|a| {
        a == "--watch"
            || a.strip_prefix("--watch=")
                .is_some_and(|v| !matches!(v, "false" | "0" | "f" | "F" | "FALSE" | "False"))
    })
}

fn deny(what: &str, every: &str) -> String {
    format!(
        "error: `{what}` is off in agent sessions: it polls every {every} and drains the GitHub rate limit that every agent and CI job of this user shares.\n\
         use: vega ci watch [<pr|branch>]   # one poller per host (vegad); the result arrives as mail\n\
         one look: gh pr checks <pr> or gh run view <id> (no watch); cached state: vega ci"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(s: &str) -> Vec<String> {
        s.split_whitespace().map(String::from).collect()
    }

    fn decide_str(s: &str, polls: usize) -> Action {
        let a = args(s);
        decide(&a, command(&a).as_deref(), |_| polls)
    }

    #[test]
    fn command_is_two_words_or_api() {
        assert_eq!(
            command(&args("pr checks 12 --watch")).as_deref(),
            Some("pr checks")
        );
        assert_eq!(
            command(&args("api repos/o/r/pulls/1")).as_deref(),
            Some("api")
        );
        assert_eq!(command(&args("--version")), None);
        assert_eq!(command(&args("status")).as_deref(), Some("status"));
    }

    #[test]
    fn watch_loops_are_denied() {
        assert!(
            matches!(decide_str("run watch 123", 0), Action::Deny(m) if m.contains("vega ci watch"))
        );
        assert!(matches!(
            decide_str("pr checks 12 --watch", 0),
            Action::Deny(_)
        ));
        assert!(matches!(
            decide_str("pr checks --watch=true", 0),
            Action::Deny(_)
        ));
        assert_eq!(decide_str("pr checks --watch=false", 0), Action::Pass);
        assert_eq!(decide_str("pr checks 12", 0), Action::Pass);
        assert_eq!(decide_str("run watch --help", 0), Action::Pass);
        assert_eq!(decide_str("run view 123", 0), Action::Pass);
    }

    #[test]
    fn a_dispatch_gets_the_run_watch_hint() {
        assert!(
            matches!(decide_str("workflow run deploy.yml --ref main", 0), Action::Hint("gh-dispatch", m) if m.contains("vega ci watch --run"))
        );
        assert_eq!(decide_str("workflow run --help", 0), Action::Pass);
        assert_eq!(decide_str("workflow list", 0), Action::Pass);
    }

    #[test]
    fn repeated_polls_get_the_hint() {
        assert_eq!(decide_str("pr checks 12", 2), Action::Pass);
        assert!(
            matches!(decide_str("pr checks 12", 3), Action::Hint("gh-poll", m) if m.contains("gh pr checks"))
        );
        assert!(matches!(decide_str("run list", 5), Action::Hint(..)));
        assert_eq!(decide_str("pr view 12", 9), Action::Pass);
    }
}
