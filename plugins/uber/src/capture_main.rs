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

use vega_herdr::{asks, notes::Store};
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
    // Resolution is independent of capture persistence and the Uber claim.
    // Unknown pane identity leaves asks open; never trust a stale env hint.
    if let Err(e) = resolve_prompt(&input, t.unix_timestamp(), || {
        Some((
            Store::local()?,
            vega_herdr::state::post_host(),
            vega_herdr::me().ok()?.pane,
        ))
    }) {
        eprintln!("vega_uber_capture: resolve ask: {e}");
    }
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

/// Reuse capture classification before looking up the pane or touching notes.
/// Like `notify resolve --pane`, resolve every local open ask for that pane.
fn resolve_prompt(
    input: &str,
    now: i64,
    context: impl FnOnce() -> Option<(Store, String, String)>,
) -> std::io::Result<usize> {
    let payload = serde_json::from_str(input).unwrap_or_default();
    let typed = vega_uber_msg::capture::from_hook(&payload, "")
        .is_some_and(|rec| rec["source"] == "user_prompt");
    if !typed {
        return Ok(0);
    }
    let Some((store, host, pane)) = context() else {
        return Ok(0);
    };
    asks::change(&store, &host, now, false, |n| {
        n.pane.as_deref() == Some(pane.as_str())
    })
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
    fn typed_prompt_resolves_only_its_panes_local_asks() {
        use vega_herdr::notes::Note;
        let dir = tempfile::tempdir().unwrap();
        let store = Store {
            dir: dir.path().into(),
        };
        for (host, pane, key, kind) in [
            ("here", "w1:p1", "default", "ask"),
            ("here", "w1:p1", "custom", "ask"),
            ("here", "w1:p2", "other-pane", "ask"),
            ("remote", "w1:p1", "other-host", "ask"),
            ("here", "w1:p1", "ordinary", "info"),
        ] {
            store
                .add(
                    Note {
                        host: host.into(),
                        pane: Some(pane.into()),
                        key: Some(key.into()),
                        kind: kind.into(),
                        level: Some("action".into()),
                        ..Default::default()
                    },
                    1000,
                )
                .unwrap();
        }
        let context = || {
            Some((
                Store {
                    dir: store.dir.clone(),
                },
                "here".into(),
                "w1:p1".into(),
            ))
        };
        for prompt in [
            "[vega-post] 1 new message for you. Run: vega post inbox",
            "<task-notification>done</task-notification>",
            "Please proceed.\n---\n[vega-mux] from w2:p1 (claude)",
        ] {
            let input = serde_json::json!({"hook_event_name":"UserPromptSubmit", "prompt":prompt});
            assert_eq!(
                resolve_prompt(&input.to_string(), 10, || panic!(
                    "non-Slayer prompt looked up pane"
                ))
                .unwrap(),
                0
            );
            assert!(store.list().iter().all(|n| n.resolved.is_none()));
        }
        assert_eq!(resolve_prompt(PROMPT, 11, context).unwrap(), 2);
        for n in store.list() {
            let expected =
                (n.host == "here" && n.pane.as_deref() == Some("w1:p1") && n.kind == "ask")
                    .then_some(11);
            assert_eq!(n.resolved, expected, "{n:?}");
        }
        assert_eq!(resolve_prompt(PROMPT, 12, context).unwrap(), 0);
        assert_eq!(resolve_prompt(PROMPT, 12, || None).unwrap(), 0);
        for input in [PICK, "{", r#"{"hook_event_name":"UserPromptSubmit"}"#] {
            assert_eq!(
                resolve_prompt(input, 12, || panic!("non-prompt looked up pane")).unwrap(),
                0
            );
        }
    }

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
