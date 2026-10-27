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
        Ok(Some(_)) => {
            let since = uber_state::file()
                .and_then(|f| uber_state::local(&f, &vega_herdr::state::post_host()))
                .and_then(|c| c["at"].as_i64());
            match engage(&dir, &input, t.unix_timestamp(), since) {
                Ok(Some(n)) => println!("{n}"),
                Ok(None) => {}
                Err(e) => eprintln!("vega_uber_capture: {e}"),
            }
        }
        Ok(None) => {}
        Err(e) => eprintln!("vega_uber_capture: {e}"),
    }
}

/// Opens the engaged window: writes `now` to `engaged.json`. Returns the
/// notice when a typed prompt opens a new window (`since`: the claim's
/// `at`; earlier input does not count), and only after the write.
fn engage(
    dir: &Path,
    input: &str,
    now: i64,
    since: Option<i64>,
) -> std::io::Result<Option<String>> {
    let file = dir.join(uber_engaged::INPUT);
    let last = uber_engaged::read_at(&file).filter(|at| Some(*at) >= since);
    let seen = uber_engaged::read_seen(&dir.join(uber_engaged::SEEN));
    let before = uber_engaged::decide(last, seen.as_ref().map(|(h, a)| (h.as_str(), *a)), now);
    uber_engaged::write_at(&file, now)?;
    let typed = serde_json::from_str::<serde_json::Value>(input)
        .is_ok_and(|p| p["hook_event_name"] == "UserPromptSubmit");
    Ok(uber_engaged::notice(&before).filter(|_| typed))
}

#[cfg(test)]
mod tests {
    use super::*;

    const PROMPT: &str = r#"{"hook_event_name":"UserPromptSubmit","prompt":"brief"}"#;
    const PICK: &str = r#"{"hook_event_name":"PostToolUse","tool_name":"AskUserQuestion"}"#;

    #[test]
    fn a_typed_prompt_that_opens_a_window_prints_one_notice() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        let n = engage(d, PROMPT, 1000, Some(900)).unwrap().unwrap();
        assert!(n.contains("first Slayer input"), "{n}");
        assert_eq!(
            uber_engaged::read_at(&d.join(uber_engaged::INPUT)),
            Some(1000)
        );
        // While engaged: nothing.
        assert_eq!(engage(d, PROMPT, 1100, Some(900)).unwrap(), None);
        assert_eq!(
            uber_engaged::read_at(&d.join(uber_engaged::INPUT)),
            Some(1100)
        );
        // Laptop input after the last capture: idle again, one notice.
        uber_engaged::write_seen(&d.join(uber_engaged::SEEN), "proteus", 1200).unwrap();
        let n = engage(d, PROMPT, 1300, Some(900)).unwrap().unwrap();
        assert!(
            n.contains("you were idle (Slayer input in herdr on proteus"),
            "{n}"
        );
        // Input before the claim does not count.
        let n = engage(d, PROMPT, 1400, Some(1350)).unwrap().unwrap();
        assert!(n.contains("first Slayer input"), "{n}");
    }

    #[test]
    fn an_ask_user_question_capture_opens_the_window_silently() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        assert_eq!(engage(d, PICK, 1000, Some(900)).unwrap(), None);
        assert_eq!(
            uber_engaged::read_at(&d.join(uber_engaged::INPUT)),
            Some(1000)
        );
        // A prompt right after it finds the window open.
        assert_eq!(engage(d, PROMPT, 1010, Some(900)).unwrap(), None);
    }

    #[test]
    fn no_notice_without_the_write() {
        let dir = tempfile::tempdir().unwrap();
        // engaged.json cannot be written: a directory stands in its way.
        std::fs::create_dir_all(dir.path().join(uber_engaged::INPUT).join("x")).unwrap();
        assert!(engage(dir.path(), PROMPT, 1000, None).is_err());
    }
}
