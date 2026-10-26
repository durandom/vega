//! `vega_uber_capture`: the Uber session's Claude Code hook for
//! `PostToolUse` (AskUserQuestion) and `UserPromptSubmit`
//! (docs/design/uber-uac.md §4). It appends the Slayer's input to
//! `$XDG_STATE_HOME/vega/uber/captures.jsonl` (`vega_uber_msg::capture`).
//!
//! A binary of its own, not a `vega uber` command: the Uber's model may run
//! `vega uber ...` without a prompt, and must not be able to write a
//! capture. It never fails the session and prints nothing on stdout (a
//! `UserPromptSubmit` hook's stdout would reach the model).

use std::io::Read;
use std::path::Path;

use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

fn main() {
    if std::env::args().nth(1).is_some() {
        eprintln!("vega_uber_capture: the Uber-UAC's capture hook; takes the hook payload on stdin, no arguments");
        return;
    }
    let mut input = String::new();
    if std::io::stdin().read_to_string(&mut input).is_err() {
        return;
    }
    let Some(dir) =
        vega_registry::uber_state::file().and_then(|f| f.parent().map(Path::to_path_buf))
    else {
        return;
    };
    let now = OffsetDateTime::now_utc()
        .replace_nanosecond(0)
        .ok()
        .and_then(|t| t.format(&Rfc3339).ok())
        .unwrap_or_default();
    if let Err(e) = vega_uber_msg::capture::hook(&dir, &input, &now) {
        eprintln!("vega_uber_capture: {e}");
    }
}
