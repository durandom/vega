//! The Slayer's input on the Uber host (docs/design/uber-uac.md §4-5):
//! Claude Code hooks append it to `$XDG_STATE_HOME/vega/uber/captures.jsonl`,
//! and `vega uber answer <id> --capture <n>` builds `slayer_text` from line
//! `n` (1-based). The model never writes `slayer_text` and never chooses
//! the binding: an AskUserQuestion capture names the ask by the `#tag` in
//! its question, a typed prompt by the tags it contains. A capture counts
//! only if the Uber session's own log holds it (`verified`).
//!
//! A line is append-only and its number never changes, so the hook needs
//! no lock; `capture.sha256` is the sha256 of the line's bytes.

use std::io::Write;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::received::{Received, State};
use crate::shown::{self, Shown};
use crate::{Answer, Capture, Kind, Level, Refusal, Source};

/// `<uber_dir>/captures.jsonl`.
pub fn file(uber_dir: &Path) -> PathBuf {
    uber_dir.join("captures.jsonl")
}

/// Prompts that are not the Slayer's typing (the spike, §5): vegad
/// pointers, harness task notices, agent mail typed by `vega mux send`.
pub fn not_slayer(prompt: &str) -> bool {
    let p = prompt.trim_start();
    p.starts_with("[vega-post]")
        || p.starts_with("<task-notification>")
        || prompt
            .lines()
            .any(|l| l.trim_start().starts_with("[vega-mux] from "))
}

/// The capture record of one hook payload, or `None` when it captures
/// nothing: another tool or event, or a prompt that is not the Slayer's.
pub fn from_hook(payload: &Value, now: &str) -> Option<Value> {
    match payload["hook_event_name"].as_str()? {
        "PostToolUse" if payload["tool_name"] == "AskUserQuestion" => {
            let input = &payload["tool_input"];
            let response = &payload["tool_response"];
            let annotations = if response["annotations"].is_object() {
                &response["annotations"]
            } else {
                &input["annotations"]
            };
            Some(json!({
                "at": now,
                "source": "ask_user_question",
                "questions": input["questions"],
                "answers": response["answers"],
                "annotations": annotations,
                "tool_use_id": payload["tool_use_id"],
                "session_id": payload["session_id"],
            }))
        }
        "UserPromptSubmit" => {
            let prompt = payload["prompt"].as_str()?;
            (!not_slayer(prompt))
                .then(|| json!({"at": now, "source": "user_prompt", "prompt": prompt, "session_id": payload["session_id"]}))
        }
        _ => None,
    }
}

/// The hook's work: one payload, at most one line appended. Returns the
/// new line's number, `None` when the payload captures nothing.
pub fn hook(uber_dir: &Path, input: &str, now: &str) -> std::io::Result<Option<usize>> {
    let Ok(payload) = serde_json::from_str::<Value>(input) else {
        return Ok(None);
    };
    match from_hook(&payload, now) {
        Some(rec) => append(uber_dir, &rec).map(Some),
        None => Ok(None),
    }
}

/// Appends a record; returns its number.
pub fn append(uber_dir: &Path, record: &Value) -> std::io::Result<usize> {
    std::fs::create_dir_all(uber_dir)?;
    let mut line = serde_json::to_vec(record)?;
    line.push(b'\n');
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(file(uber_dir))?;
    f.write_all(&line)?;
    Ok(count(uber_dir))
}

/// The number of lines so far.
pub fn count(uber_dir: &Path) -> usize {
    std::fs::read(file(uber_dir))
        .map(|b| b.iter().filter(|&&c| c == b'\n').count())
        .unwrap_or(0)
}

/// Line `n` (1-based), without its line feed.
pub fn read(uber_dir: &Path, n: usize) -> Option<String> {
    let text = std::fs::read_to_string(file(uber_dir)).ok()?;
    text.lines().nth(n.checked_sub(1)?).map(String::from)
}

/// The tags a typed prompt contains: `#` and 1-4 letters or digits, not
/// inside a word; lowercased (a phone may capitalise).
pub fn tags(prompt: &str) -> Vec<String> {
    let chars: Vec<char> = prompt.chars().collect();
    let mut out = Vec::new();
    for (i, &c) in chars.iter().enumerate() {
        if c != '#' || (i > 0 && chars[i - 1].is_alphanumeric()) {
            continue;
        }
        let run: String = chars[i + 1..]
            .iter()
            .take_while(|c| c.is_ascii_alphanumeric())
            .collect();
        let word_ends = chars
            .get(i + 1 + run.len())
            .is_none_or(|c| !c.is_alphanumeric());
        let t = format!("#{}", run.to_ascii_lowercase());
        if word_ends && crate::valid_tag(&t) && !out.contains(&t) {
            out.push(t);
        }
    }
    out
}

fn no(code: &'static str, message: impl Into<String>) -> Refusal {
    Refusal {
        code,
        message: message.into(),
    }
}

/// The captured AskUserQuestion question for `tag`, compared with how the
/// ask must be shown: the model cannot pass reworded options under a tag.
fn shown_as_asked<'a>(rec: &'a Value, r: &Received) -> Result<(&'a str, Shown), Refusal> {
    let prefix = format!("{} ", r.tag);
    let mut found = rec["questions"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|q| {
            q["question"]
                .as_str()
                .is_some_and(|s| s.starts_with(&prefix))
        });
    let (Some(q), None) = (found.next(), found.next()) else {
        return Err(no(
            "capture_unbound",
            format!("the capture shows no single question tagged {}", r.tag),
        ));
    };
    let want = shown::render(&r.ask, &r.tag);
    let options: Option<Vec<(&str, &str)>> = q["options"].as_array().map(|os| {
        os.iter()
            .map(|o| {
                (
                    o["label"].as_str().unwrap_or(""),
                    o["description"].as_str().unwrap_or(""),
                )
            })
            .collect()
    });
    let same = q["header"].as_str() == Some(&want.header)
        && q["question"].as_str() == Some(&want.question)
        && !q["multiSelect"].as_bool().unwrap_or(false)
        && options
            == Some(
                want.options
                    .iter()
                    .map(|o| (o.label.as_str(), o.description.as_str()))
                    .collect(),
            );
    if !same {
        return Err(no(
            "capture_mismatch",
            format!("{} was not shown as asked (vega uber show)", r.tag),
        ));
    }
    Ok((q["question"].as_str().unwrap_or_default(), want))
}

/// The lines of a Claude Code session log that are the main thread's
/// (not a subagent's), parsed.
fn main_lines(transcript: &str) -> impl Iterator<Item = Value> + '_ {
    transcript
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter(|v| v["isSidechain"] != true)
}

fn blocks(v: &Value) -> impl Iterator<Item = &Value> {
    v["message"]["content"].as_array().into_iter().flatten()
}

/// The capture as the harness logged it in the Uber session's transcript
/// (`transcript`, the session log of the live Uber pane): a record the
/// model cannot write. An AskUserQuestion capture needs the `tool_use` with
/// the same questions and its `tool_result` line; the answers and notes are
/// then taken from that line. A typed prompt needs a user line with exactly
/// that text. Shape of Claude Code 2.1: `type`, `message.content`,
/// `toolUseResult {questions, answers, annotations}`.
fn verified(rec: &Value, transcript: &str, n: usize) -> Result<Value, Refusal> {
    let unverified = |what: &str| {
        no(
            "capture_unverified",
            format!("capture {n}: {what} is not in the Uber session's transcript"),
        )
    };
    match rec["source"].as_str() {
        Some("ask_user_question") => {
            let want_id = rec["tool_use_id"].as_str();
            let ids: Vec<String> = main_lines(transcript)
                .filter(|v| v["type"] == "assistant")
                .flat_map(|v| blocks(&v).cloned().collect::<Vec<_>>())
                .filter(|b| {
                    b["type"] == "tool_use"
                        && b["name"] == "AskUserQuestion"
                        && b["input"]["questions"] == rec["questions"]
                        && want_id.is_none_or(|id| b["id"] == id)
                })
                .filter_map(|b| b["id"].as_str().map(String::from))
                .collect();
            let result = main_lines(transcript)
                .filter(|v| {
                    v["type"] == "user" && v["toolUseResult"]["questions"] == rec["questions"]
                })
                .find(|v| {
                    blocks(v).any(|b| {
                        b["type"] == "tool_result"
                            && b["tool_use_id"]
                                .as_str()
                                .is_some_and(|id| ids.iter().any(|i| i == id))
                    })
                })
                .ok_or_else(|| unverified("the AskUserQuestion answer"))?;
            let logged = &result["toolUseResult"];
            if logged["answers"] != rec["answers"] {
                return Err(unverified("these answers"));
            }
            let mut rec = rec.clone();
            rec["annotations"] = logged["annotations"].clone();
            Ok(rec)
        }
        Some("user_prompt") => {
            let prompt = rec["prompt"].as_str().unwrap_or_default();
            let typed = |v: &Value| match &v["message"]["content"] {
                Value::String(s) => s == prompt,
                Value::Array(bs) => bs
                    .iter()
                    .any(|b| b["type"] == "text" && b["text"] == prompt),
                _ => false,
            };
            main_lines(transcript)
                .any(|v| {
                    v["type"] == "user"
                        && v["isMeta"] != true
                        && v["toolUseResult"].is_null()
                        && typed(&v)
                })
                .then(|| rec.clone())
                .ok_or_else(|| unverified("the prompt"))
        }
        _ => Err(no(
            "capture_malformed",
            format!("capture {n} has no source"),
        )),
    }
}

/// The answer to ask `r` from capture line `n`, under the claim `epoch`.
/// `transcript` is the live Uber session's log: the capture must be in it.
/// Refuses a closed ask, a used, older, unbound or unlogged capture, and
/// options that were not shown as asked.
pub fn build(
    r: &Received,
    n: usize,
    line: &str,
    epoch: u64,
    transcript: &str,
) -> Result<Answer, Refusal> {
    if r.state != State::Open {
        let state = serde_json::to_value(r.state).unwrap_or_default();
        return Err(no(
            "not_open",
            format!("{} is {}", r.tag, state.as_str().unwrap_or("closed")),
        ));
    }
    if r.used.contains(&n) {
        return Err(no(
            "capture_used",
            format!("capture {n} was used for {}", r.tag),
        ));
    }
    if n <= r.floor {
        return Err(no(
            "capture_unbound",
            format!("capture {n} is older than {}", r.tag),
        ));
    }
    let rec: Value = serde_json::from_str(line)
        .map_err(|e| no("capture_malformed", format!("capture {n}: {e}")))?;
    let rec = verified(&rec, transcript, n)?;
    let at = rec["at"].as_str().unwrap_or_default().to_string();
    let sha256 = shown::hex256(line.as_bytes());
    let (source, pick, notes, slayer_text, shown_sha256) = match rec["source"].as_str() {
        Some("ask_user_question") => {
            let (question, view) = shown_as_asked(&rec, r)?;
            let Some(said) = rec["answers"][question].as_str() else {
                return Err(no(
                    "capture_unbound",
                    format!("capture {n} holds no answer for {}", r.tag),
                ));
            };
            let notes = rec["annotations"][question]["notes"]
                .as_str()
                .filter(|s| !s.trim().is_empty())
                .map(String::from);
            let pick = view.pick(&r.ask, said);
            let base = pick.clone().unwrap_or_else(|| said.to_string());
            let text = match &notes {
                Some(t) => format!("{base} — {t}"),
                None => base,
            };
            (
                Source::AskUserQuestion,
                pick,
                notes,
                text,
                Some(shown::sha256(&r.ask, &r.tag)),
            )
        }
        Some("user_prompt") => {
            let prompt = rec["prompt"].as_str().unwrap_or_default();
            if !tags(prompt).contains(&r.tag) {
                return Err(no(
                    "capture_unbound",
                    format!("prompt {n} does not name {}", r.tag),
                ));
            }
            (Source::UserPrompt, None, None, prompt.to_string(), None)
        }
        _ => {
            return Err(no(
                "capture_malformed",
                format!("capture {n} has no source"),
            ))
        }
    };
    let answer = Answer {
        id: r.ask.id.clone(),
        tag: r.tag.clone(),
        bead: r.ask.bead.clone(),
        epoch,
        // L4 approval only from an exact option label picked in the dialog.
        approval: r.ask.level == Level::L4 && source == Source::AskUserQuestion && pick.is_some(),
        pick,
        notes,
        slayer_text,
        shown_sha256,
        capture: Capture { source, at, sha256 },
    };
    let body = serde_json::to_vec(&answer).unwrap_or_default();
    crate::validate(Kind::Answer, &body)?;
    Ok(answer)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::received::{self, Received};
    use crate::{Applied, Ask, Msg, Opt, Withdraw};
    use std::path::Path;

    const NOW: &str = "2026-10-26T13:00:00Z";

    thread_local! {
        /// The session log the harness writes for this test's payloads.
        static LOG: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
    }

    /// The transcript lines Claude Code writes for a hook payload.
    fn logged(p: &Value) -> String {
        let lines = match p["hook_event_name"].as_str() {
            Some("PostToolUse") => {
                let id = p["tool_use_id"].as_str().unwrap_or("toolu_x");
                vec![
                    json!({"type": "assistant", "isSidechain": false, "message": {"content": [
                        {"type": "tool_use", "id": id, "name": "AskUserQuestion", "input": p["tool_input"]}]}}),
                    json!({"type": "user", "isSidechain": false, "message": {"content": [
                        {"type": "tool_result", "tool_use_id": id, "content": "User has answered"}]},
                        "toolUseResult": {"questions": p["tool_input"]["questions"],
                            "answers": p["tool_response"]["answers"],
                            "annotations": p["tool_response"]["annotations"]}}),
                ]
            }
            _ => vec![json!({"type": "user", "message": {"content": p["prompt"]}})],
        };
        lines.iter().map(|l| format!("{l}\n")).collect()
    }

    fn build_t(r: &Received, n: usize, line: &str, epoch: u64) -> Result<Answer, Refusal> {
        LOG.with(|t| build(r, n, line, epoch, &t.borrow()))
    }

    fn ask(id: &str, level: Level) -> Ask {
        Ask {
            id: id.into(),
            project: "github.com/durandom/vega".into(),
            bead: crate::id_bead(id).unwrap().into(),
            level,
            urgent: None,
            question: "Push main?".into(),
            context: "Gate green.".into(),
            options: vec![
                Opt {
                    label: "yes".into(),
                    detail: "push".into(),
                },
                Opt {
                    label: "no".into(),
                    detail: "wait".into(),
                },
            ],
            recommend: 0,
            why: "Tested.".into(),
            deadline: "no-rush".into(),
        }
    }

    fn receive(dir: &Path, level: Level) -> Received {
        let id = crate::new_id("vega-lhc.209").unwrap();
        let ev =
            received::ingest(dir, &Msg::Ask(ask(&id, level)), "proteus", "w3V:p1", NOW).unwrap();
        assert_eq!(ev.event, "received");
        received::load(dir, &id).unwrap()
    }

    /// The PostToolUse payload of the dialog as the model showed it.
    fn dialog(shown: &[Shown], answers: Value) -> Value {
        let qs: Vec<Value> = shown
            .iter()
            .map(|s| json!({"header": s.header, "question": s.question, "options": s.options, "multiSelect": false}))
            .collect();
        json!({
            "hook_event_name": "PostToolUse", "tool_name": "AskUserQuestion",
            "tool_input": {"questions": qs}, "tool_response": {"answers": answers},
        })
    }

    fn capture(dir: &Path, payload: Value) -> (usize, String) {
        LOG.with(|t| t.borrow_mut().push_str(&logged(&payload)));
        let rec = from_hook(&payload, NOW).expect("captured");
        let n = append(dir, &rec).unwrap();
        (n, read(dir, n).unwrap())
    }

    #[test]
    fn a_pick_in_the_dialog_answers_and_approves_l4() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        let r = receive(d, Level::L4);
        let other = receive(d, Level::L3);
        assert_ne!(r.tag, other.tag);
        let s = shown::render(&r.ask, &r.tag);
        let s2 = shown::render(&other.ask, &other.tag);
        let answers =
            json!({ s.question.clone(): "yes (Recommended)", s2.question.clone(): "Pizza please" });
        let mut p = dialog(&[s.clone(), s2], answers);
        p["tool_response"]["annotations"] = json!({ s.question.clone(): {"notes": "after lunch"} });
        let (n, line) = capture(d, p);
        let a = build_t(&r, n, &line, 7).unwrap();
        assert_eq!(a.pick.as_deref(), Some("yes"));
        assert!(a.approval);
        assert_eq!(a.slayer_text, "yes — after lunch");
        assert_eq!(a.notes.as_deref(), Some("after lunch"));
        assert_eq!(a.epoch, 7);
        assert_eq!(a.shown_sha256, Some(shown::sha256(&r.ask, &r.tag)));
        assert_eq!(a.capture.sha256, shown::hex256(line.as_bytes()));
        assert_eq!(a.capture.source, Source::AskUserQuestion);
        // "Other" text is verbatim, no pick, no approval.
        let b = build_t(&other, n, &line, 7).unwrap();
        assert_eq!(
            (b.pick, b.approval, b.slayer_text.as_str()),
            (None, false, "Pizza please")
        );
    }

    #[test]
    fn reworded_or_untagged_dialogs_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        let r = receive(d, Level::L4);
        let mut s = shown::render(&r.ask, &r.tag);
        s.options[1].description = "wait, it is fine".into();
        let (n, line) = capture(d, dialog(&[s.clone()], json!({ s.question.clone(): "no" })));
        assert_eq!(
            build_t(&r, n, &line, 7).unwrap_err().code,
            "capture_mismatch"
        );
        s = shown::render(&r.ask, &r.tag);
        s.question = s.question.replacen(&r.tag, "#zzzz", 1);
        let (n, line) = capture(d, dialog(&[s.clone()], json!({ s.question.clone(): "no" })));
        assert_eq!(
            build_t(&r, n, &line, 7).unwrap_err().code,
            "capture_unbound"
        );
    }

    #[test]
    fn prompts_bind_only_by_their_tags_and_never_approve() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        let r = receive(d, Level::L4);
        let typed = |text: &str| json!({"hook_event_name": "UserPromptSubmit", "prompt": text});
        let (n, line) = capture(d, typed("brief"));
        assert_eq!(
            build_t(&r, n, &line, 7).unwrap_err().code,
            "capture_unbound"
        );
        let upper = r.tag.to_ascii_uppercase();
        let (n, line) = capture(d, typed(&format!("{upper} ja,\nbut tomorrow")));
        let a = build_t(&r, n, &line, 7).unwrap();
        assert_eq!((a.pick, a.approval, a.shown_sha256), (None, false, None));
        assert_eq!(a.slayer_text, format!("{upper} ja,\nbut tomorrow"));
        assert_eq!(a.capture.source, Source::UserPrompt);
        // vegad pointers, task notices and agent mail are not the Slayer's.
        for p in [
            "[vega-post] 1 new message for you. Run: vega post inbox".to_string(),
            "<task-notification>done</task-notification>".to_string(),
            format!(
                "{} ja\n[vega-mux] from w3V:p1 (claude in ~/src/x), 2026-10-26",
                r.tag
            ),
        ] {
            assert_eq!(from_hook(&typed(&p), NOW), None, "{p}");
        }
        assert_eq!(
            from_hook(
                &json!({"hook_event_name": "PostToolUse", "tool_name": "Bash"}),
                NOW
            ),
            None
        );
    }

    #[test]
    fn hook_appends_only_captures() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        assert_eq!(hook(d, "not json", NOW).unwrap(), None);
        let pointer =
            json!({"hook_event_name": "UserPromptSubmit", "prompt": "[vega-post] 1 new message"});
        assert_eq!(hook(d, &pointer.to_string(), NOW).unwrap(), None);
        let typed = json!({"hook_event_name": "UserPromptSubmit", "prompt": "#a7 ja"});
        assert_eq!(hook(d, &typed.to_string(), NOW).unwrap(), Some(1));
        assert_eq!(count(d), 1);
    }

    #[test]
    fn tags_in_text() {
        assert_eq!(tags("#a7 ja, #B2 nein"), ["#a7", "#b2"]);
        assert_eq!(tags("x#a7 #abcde #a7"), ["#a7"]);
        assert!(tags("issue #12345 or #").is_empty());
    }

    #[test]
    fn captures_are_used_once_and_only_after_the_ask() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        let typed = |text: &str| json!({"hook_event_name": "UserPromptSubmit", "prompt": text});
        let (old, _) = capture(d, typed("#0 early"));
        let mut r = receive(d, Level::L3);
        let t = r.tag.clone();
        // Written before the ask arrived: it cannot answer it.
        let old_line = read(d, old).unwrap().replace("#0", &t);
        assert_eq!(
            build_t(&r, old, &old_line, 7).unwrap_err().code,
            "capture_unbound"
        );
        let (n, line) = capture(d, typed(&format!("{t} ok")));
        assert!(build_t(&r, n, &line, 7).is_ok());
        r.used.push(n);
        assert_eq!(build_t(&r, n, &line, 7).unwrap_err().code, "capture_used");
        r.used.clear();
        r.state = State::Sent;
        assert_eq!(build_t(&r, n, &line, 7).unwrap_err().code, "not_open");
    }

    #[test]
    fn ingest_tracks_resend_withdraw_and_applied() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        let mut r = receive(d, Level::L3);
        let id = r.ask.id.clone();
        // A forged withdraw from another host changes nothing.
        let w = Msg::Withdraw(Withdraw {
            id: id.clone(),
            reason: "obsolete".into(),
        });
        assert_eq!(
            received::ingest(d, &w, "titan", "x:p1", NOW).unwrap().event,
            "ignored"
        );
        r.state = State::Sent;
        received::save(d, &r).unwrap();
        let refused = Msg::Applied(Applied {
            id: id.clone(),
            result: "refused:answered".into(),
        });
        assert_eq!(
            received::ingest(d, &refused, "proteus", "-", NOW)
                .unwrap()
                .event,
            "refused"
        );
        assert_eq!(received::load(d, &id).unwrap().state, State::Refused);
        // The resend after a final refusal reopens the ask under the same tag.
        let ev = received::ingest(d, &Msg::Ask(r.ask.clone()), "proteus", "w3V:p9", NOW).unwrap();
        assert_eq!(ev.event, "resent");
        let again = received::load(d, &id).unwrap();
        assert_eq!(
            (again.state, again.tag.as_str(), again.from_pane.as_str()),
            (State::Open, r.tag.as_str(), "w3V:p9")
        );
        assert_eq!(
            received::ingest(d, &w, "proteus", "w3V:p9", NOW)
                .unwrap()
                .event,
            "withdrawn"
        );
        assert_eq!(received::load(d, &id).unwrap().state, State::Withdrawn);
        let events: Vec<String> = crate::audit::last(d, 10)
            .into_iter()
            .map(|e| e.event)
            .collect();
        assert_eq!(
            events,
            ["received", "ignored", "refused", "resent", "withdrawn"]
        );
        let line = crate::audit::last(d, 1)[0].line();
        assert_eq!(
            line,
            format!("10-26 13:00Z withdrawn {} vega-lhc.209: obsolete", r.tag)
        );
    }

    #[test]
    fn a_retryable_refusal_reopens_the_ask_with_its_capture() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        let mut r = receive(d, Level::L3);
        let typed =
            json!({"hook_event_name": "UserPromptSubmit", "prompt": format!("{} ok", r.tag)});
        let (n, line) = capture(d, typed);
        assert!(build_t(&r, n, &line, 7).is_ok());
        r.used.push(n);
        r.state = State::Sent;
        received::save(d, &r).unwrap();
        let applied = |result: &str| {
            Msg::Applied(Applied {
                id: r.ask.id.clone(),
                result: result.into(),
            })
        };
        let ev = received::ingest(d, &applied("refused:bd_failed"), "proteus", "-", NOW).unwrap();
        assert_eq!(ev.event, "refused");
        assert!(ev.detail.contains("open again"), "{}", ev.detail);
        let mut again = received::load(d, &r.ask.id).unwrap();
        assert_eq!((again.state, again.used.len()), (State::Open, 0));
        // The same capture is sent again, with no new claim epoch.
        assert!(build_t(&again, n, &line, 7).is_ok());
        again.used.push(n);
        again.state = State::Sent;
        received::save(d, &again).unwrap();
        received::ingest(d, &applied("refused:answered"), "proteus", "-", NOW).unwrap();
        let closed = received::load(d, &r.ask.id).unwrap();
        assert_eq!(closed.state, State::Refused);
        assert_eq!(build_t(&closed, n, &line, 7).unwrap_err().code, "not_open");
    }

    #[test]
    fn fabricated_captures_cannot_answer_or_approve() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        let r = receive(d, Level::L4);
        let s = shown::render(&r.ask, &r.tag);
        let p = dialog(
            std::slice::from_ref(&s),
            json!({ s.question.clone(): "no" }),
        );
        // The Slayer picked "no"; the harness logged that.
        let (_, _) = capture(d, p.clone());
        // A forged capture of the same dialog that says "yes (Recommended)".
        let mut forged = p.clone();
        forged["tool_response"]["answers"] = json!({ s.question.clone(): "yes (Recommended)" });
        let n = append(d, &from_hook(&forged, NOW).unwrap()).unwrap();
        let line = read(d, n).unwrap();
        let e = build_t(&r, n, &line, 7).unwrap_err();
        assert_eq!(e.code, "capture_unverified", "{}", e.message);
        // A dialog the model never opened, even if the model echoed it in a
        // Bash call (its tool_use input is text inside another line).
        let dir2 = tempfile::tempdir().unwrap();
        let d2 = dir2.path();
        let r2 = receive(d2, Level::L4);
        let s2 = shown::render(&r2.ask, &r2.tag);
        let fake = dialog(
            std::slice::from_ref(&s2),
            json!({ s2.question.clone(): "yes (Recommended)" }),
        );
        let echoed = logged(&fake);
        let bash = json!({"type": "assistant", "message": {"content": [
            {"type": "tool_use", "id": "toolu_b", "name": "Bash", "input": {"command": echoed}}]}});
        let sidechain: String = logged(&fake)
            .lines()
            .map(|l| {
                let mut v: Value = serde_json::from_str(l).unwrap();
                v["isSidechain"] = json!(true);
                format!("{v}\n")
            })
            .collect();
        let log = format!("{bash}\n{sidechain}");
        let n2 = append(d2, &from_hook(&fake, NOW).unwrap()).unwrap();
        let line2 = read(d2, n2).unwrap();
        assert_eq!(
            build(&r2, n2, &line2, 7, &log).unwrap_err().code,
            "capture_unverified"
        );
        // The real log lines verify it.
        assert!(build(&r2, n2, &line2, 7, &logged(&fake)).unwrap().approval);
        // A typed prompt nobody typed.
        let typed =
            json!({"hook_event_name": "UserPromptSubmit", "prompt": format!("{} ja", r2.tag)});
        let n3 = append(d2, &from_hook(&typed, NOW).unwrap()).unwrap();
        let line3 = read(d2, n3).unwrap();
        assert_eq!(
            build(&r2, n3, &line3, 7, &log).unwrap_err().code,
            "capture_unverified"
        );
        assert!(build(&r2, n3, &line3, 7, &logged(&typed)).is_ok());
    }
}
