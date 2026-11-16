//! Shared mechanics of `vega browser` (the plugin) and the `agent-browser`
//! shim: the one rule's session-name parsing, owner records, the real
//! agent-browser binary, the CDP probe and the process table.
//!
//! Design: docs/design/browser.md.

pub mod cdp;
pub mod mark;
pub mod owner;
pub mod pane;
pub mod procs;
pub mod real;

use vega_herdr::Failure;

/// The one rule (design, decision 1): a call carries a session name, either
/// `--session <name>` or `--session=<name>` anywhere in the args, else a
/// non-empty `AGENT_BROWSER_SESSION`. None: the call is unnamed.
///
/// A valueless `--session` names nothing; agent-browser would reject it, and
/// the shim refuses before that with its own hint.
pub fn session_name(args: &[String]) -> Option<String> {
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == "--session" {
            return it
                .next()
                .and_then(|v| (!v.is_empty() && !v.starts_with('-')).then(|| v.clone()));
        }
        if let Some(v) = a.strip_prefix("--session=") {
            return (!v.is_empty()).then(|| v.to_owned());
        }
    }
    std::env::var("AGENT_BROWSER_SESSION")
        .ok()
        .filter(|s| !s.is_empty())
}

/// The refusal an unnamed call gets, from the shim and from `ab`.
pub fn name_failure() -> Failure {
    Failure::new(
        "name_your_session",
        "agent-browser: name your session: --session <name> or AGENT_BROWSER_SESSION=<name>",
        "(e.g. --session <project>-<task>); see `vega skills browser`",
    )
}

/// `agent-browser session list` output: the names under `Active sessions:`.
pub fn parse_session_list(stdout: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut header = false;
    for line in stdout.lines() {
        if !header {
            header = line.trim_end().ends_with(':');
            continue;
        }
        let name = line.trim();
        if !name.is_empty() {
            names.push(name.to_owned());
        }
    }
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(s: &str) -> Vec<String> {
        s.split_whitespace().map(String::from).collect()
    }

    #[test]
    fn session_names_come_from_flag_or_env() {
        // env::var is process-wide state; this is the only test touching it.
        std::env::remove_var("AGENT_BROWSER_SESSION");
        assert_eq!(session_name(&args("get url")), None);
        assert_eq!(
            session_name(&args("--session me get url")).as_deref(),
            Some("me")
        );
        assert_eq!(
            session_name(&args("get url --session=me")).as_deref(),
            Some("me")
        );
        assert_eq!(session_name(&args("--session= ")), None);
        assert_eq!(session_name(&args("--session")), None);
        assert_eq!(session_name(&args("--session --json get url")), None);
        std::env::set_var("AGENT_BROWSER_SESSION", "tile-clip-42");
        assert_eq!(
            session_name(&args("snapshot -i")).as_deref(),
            Some("tile-clip-42")
        );
        // The flag wins over the environment.
        assert_eq!(
            session_name(&args("--session me snapshot")).as_deref(),
            Some("me")
        );
        std::env::set_var("AGENT_BROWSER_SESSION", "");
        assert_eq!(session_name(&args("snapshot -i")), None);
        std::env::remove_var("AGENT_BROWSER_SESSION");
    }

    #[test]
    fn parses_active_sessions() {
        let out = "Active sessions:\n  review-t7m\n  classic-live\n\n  rc-s5wm\n";
        assert_eq!(
            parse_session_list(out),
            ["review-t7m", "classic-live", "rc-s5wm"]
        );
        assert_eq!(
            parse_session_list("Active sessions:\n"),
            Vec::<String>::new()
        );
        assert_eq!(parse_session_list(""), Vec::<String>::new());
    }
}
