//! Agent-session shim for `agent-browser` (docs/design/browser.md, Shim).
//!
//! Ordinary headed calls and existing dev sessions use the project-scoped
//! CfT adapter. Other named calls retain upstream arguments and record their
//! owner. Explicit headed attachments also carry the session's window mark.
//! An unnamed call is refused with
//! a hint — a documented deviation from P2: default sessions are the
//! unnamed leftovers nobody can attribute, and `close` without a name would
//! close one of them. Exempt without a name: `--help`, `--version`,
//! `skills …`, `session list`.
//!
//! The shim `exec`s the real binary by absolute path (`VEGA_AGENT_BROWSER_BIN`,
//! then PATH), skipping vega shim builds.

use std::env;
use std::ffi::OsString;
use std::process::ExitCode;

use vega_browser::{dev, mark, owner, real, session_name};

const TOOL: &str = "agent-browser";

fn main() -> ExitCode {
    if let Err(code) = vega_shim::enter(TOOL, Some(real::ENV)) {
        return code;
    }
    let args: Vec<String> = env::args_os()
        .skip(1)
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    if let Some(endpoint) = env::var_os("VEGA_BROWSER_SHARE") {
        return match dev::share::remote(&endpoint.to_string_lossy(), &args) {
            Ok(code) => code,
            Err(error) => {
                if args.iter().any(|a| a == "--json") {
                    vega_herdr::print_failure_json(&error);
                } else {
                    vega_herdr::print_failure(&error);
                }
                ExitCode::FAILURE
            }
        };
    }
    if args == [dev::PROXY] {
        return if dev::serve().is_ok() {
            ExitCode::SUCCESS
        } else {
            ExitCode::FAILURE
        };
    }
    if raw_shared_endpoint(&args) {
        eprintln!("agent-browser: use vega browser ab --profile global|vpn --session <name> for shared Chromium");
        return ExitCode::from(2);
    }
    if exempt(&args) {
        return vega_shim::exec_real(TOOL, Some(real::ENV));
    }
    if let Some(session) = session_name(&args) {
        match dev::shared::bound(&session).and_then(|port| {
            if let Some(port) = port {
                return dev::shared::resume(&args, port).map(Some);
            }
            dev::routes(&session, &args)
                .and_then(|dev| dev.then(|| vega_browser::dev::command(&args)).transpose())
        }) {
            Ok(Some(code)) => return code,
            Ok(None) => {}
            Err(f) => {
                if args.iter().any(|a| a == "--json") {
                    vega_herdr::print_failure_json(&f);
                } else {
                    vega_herdr::print_failure(&f);
                }
                return ExitCode::from(if f.code == "usage" { 2 } else { 1 });
            }
        }
        // Best effort: a call whose record cannot be written still passes.
        let record = owner::record_from_env(vega_shim::unix_now());
        owner::upsert(&session, record.clone());
        let marked = mark::with_mark(&args, &session, record.project.as_deref());
        let argv: Vec<OsString> = std::iter::once(OsString::from(TOOL))
            .chain(marked.into_iter().map(OsString::from))
            .collect();
        let (arg0, rest) = argv.split_first().expect("argv is nonempty");
        return vega_shim::exec_real_with(TOOL, Some(real::ENV), arg0.clone(), rest.to_vec());
    }
    eprintln!("agent-browser: name your session: --session <name> or AGENT_BROWSER_SESSION=<name>");
    eprintln!("  For headed dev: vega browser dev --session <project>-<task> open <url>");
    eprintln!("  Reuse that name for later commands and see `vega skills browser`.");
    ExitCode::from(1)
}

fn raw_shared_endpoint(args: &[String]) -> bool {
    fn shared(value: &str) -> bool {
        let port = value.parse::<u16>().ok().or_else(|| {
            let url = tungstenite::http::Uri::try_from(value).ok()?;
            matches!(url.host()?, "127.0.0.1" | "localhost" | "[::1]")
                .then(|| url.port_u16())
                .flatten()
        });
        matches!(port, Some(9223 | 9224))
    }
    env::var("AGENT_BROWSER_CDP").is_ok_and(|v| shared(&v))
        || args.iter().enumerate().any(|(i, arg)| {
            arg.strip_prefix("--cdp=").is_some_and(shared)
                || (matches!(arg.as_str(), "--cdp" | "connect")
                    && args.get(i + 1).is_some_and(|v| shared(v)))
        })
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
