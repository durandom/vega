//! `vega_uber_capture`: the Uber session's Claude Code hook for
//! `PostToolUse` (AskUserQuestion) and `UserPromptSubmit`
//! (docs/design/uber-uac.md §4). It appends the Slayer's input to
//! `$XDG_STATE_HOME/vega/uber/captures.jsonl` (`vega_uber_msg::capture`).
//!
//! Each capture also opens the engaged window (vega-lhc.209.12): it writes
//! its time to `engaged.json` (`vega_registry::uber_engaged`).
//!
//! A binary of its own, not a `vega uber` command: the Uber's model may run
//! `vega uber ...` without a prompt, and must not be able to write a
//! capture. It never fails the session. On stdout (a `UserPromptSubmit`
//! hook's stdout reaches the model) it prints only the one-line notice when
//! a typed prompt opens a window: the Uber tells the Slayer once that it was
//! idle, then pulls the brief.

use std::io::Read;
use std::path::Path;

use vega_registry::{uber_engaged, uber_state};

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
    let t = OffsetDateTime::now_utc();
    let now = t
        .replace_nanosecond(0)
        .ok()
        .and_then(|t| t.format(&Rfc3339).ok())
        .unwrap_or_default();
    match vega_uber_msg::capture::hook(&dir, &input, &now) {
        Ok(Some(_)) => engage(&dir, &input, t.unix_timestamp()),
        Ok(None) => {}
        Err(e) => eprintln!("vega_uber_capture: {e}"),
    }
}

/// Opens the engaged window; prints the notice for a typed prompt that
/// opens a new one.
fn engage(dir: &Path, input: &str, now: i64) {
    let file = dir.join(uber_engaged::INPUT);
    let since = uber_state::file()
        .and_then(|f| uber_state::local(&f, &vega_herdr::state::post_host()))
        .and_then(|c| c["at"].as_i64());
    let last = uber_engaged::read_at(&file).filter(|at| Some(*at) >= since);
    let seen = uber_engaged::read_seen(&dir.join(uber_engaged::SEEN));
    let before = uber_engaged::decide(last, seen.as_ref().map(|(h, a)| (h.as_str(), *a)), now);
    if let Err(e) = uber_engaged::write_at(&file, now) {
        eprintln!("vega_uber_capture: {e}");
    }
    let typed = serde_json::from_str::<serde_json::Value>(input)
        .is_ok_and(|p| p["hook_event_name"] == "UserPromptSubmit");
    if let Some(n) = uber_engaged::notice(&before).filter(|_| typed) {
        println!("{n}");
    }
}
