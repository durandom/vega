//! `vega mux hook`: agent hooks that keep a spawned agent's questions with its
//! parent. A sentinel that asks in its own pane (Claude's AskUserQuestion,
//! codex's request_user_input) blocks until a human answers there; with a
//! recorded parent the PreToolUse hook denies the dialog and names the
//! `vega post send` command instead. Without a parent, or in a headed pane
//! (`vega mux mode`, the default of `vega uac adopt`), it does nothing, so
//! sessions the user drives keep their dialogs.
//!
//! Codex's request_user_input_async does not block: a headless child is
//! denied like the other dialogs; a headed one keeps the question bar and its
//! parent gets a copy by mail (docs/design/pane-control.md "Codex async
//! questions").
//!
//! `vega config apply hooks` installs it (units claude.hooks and codex.hooks,
//! plugins/config/src/merge.rs); `vega mux hook install` is an alias for that.
//! Every new codex session shows "Hooks need review" until the user trusts
//! the hook (per command and matcher, so again after either changes).

use std::io::Read;
use std::process::{Command, Stdio};

use serde_json::{json, Value};
use vega_herdr::state::{mode_of, parent_of, redirects_questions, Mode, Parent};
use vega_herdr::{agent_name, herdr_local, local_host, machine, me};

/// The dialog tools the hook intercepts, also the hook's matcher.
const CLAUDE_TOOL: &str = "AskUserQuestion";
const CODEX_TOOL: &str = "request_user_input";
const CODEX_ASYNC_TOOL: &str = "request_user_input_async";

/// The PreToolUse hook: reads the hook input on stdin and prints a deny
/// decision when this pane has a recorded parent and is headless; a headed
/// pane's async question goes to the parent as a copy. Fails open: any error
/// prints nothing, so the dialog stays allowed.
pub fn ask_parent() {
    let mut input = String::new();
    let _ = std::io::stdin().read_to_string(&mut input);
    let Ok(v) = serde_json::from_str::<Value>(&input) else {
        return;
    };
    let Some(tool) = v["tool_name"].as_str() else {
        return;
    };
    if !matches!(tool, CLAUDE_TOOL | CODEX_TOOL | CODEX_ASYNC_TOOL) {
        return;
    }
    let Ok(me) = me() else { return };
    let Ok(a) = herdr_local(&["agent", "get", &me.pane]) else {
        return;
    };
    let name = agent_name(&a["result"]["agent"]);
    let parent = parent_of(&me.pane, name);
    let mode = mode_of(machine(), &me.pane, name);
    match route(
        tool,
        parent.as_ref(),
        mode,
        &local_host(),
        &me.pane,
        &v["tool_input"],
    ) {
        Route::Allow => {}
        // In the background: the tool is not delayed, a failure stays silent.
        Route::Copy(args) => {
            let _ = Command::new("vega")
                .args(&args)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn();
        }
        Route::Deny(reason) => println!(
            "{}",
            json!({"hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "permissionDecision": "deny",
                "permissionDecisionReason": reason,
            }})
        ),
    }
}

/// What the hook does with one dialog.
#[derive(Debug, PartialEq)]
enum Route {
    /// Prints nothing: the dialog stays.
    Allow,
    /// Allows the dialog and runs `vega <args>` to mail the parent a copy.
    Copy(Vec<String>),
    /// Denies the dialog with this reason.
    Deny(String),
}

/// Routes `tool`'s dialog of pane `me` on host `local`: a headed async
/// question goes to the parent as a copy, a headless child's dialog is
/// denied, without a parent everything stays.
fn route(
    tool: &str,
    parent: Option<&Parent>,
    mode: Mode,
    local: &str,
    me: &str,
    input: &Value,
) -> Route {
    let Some(p) = parent else { return Route::Allow };
    let remote = p.host != local;
    let is_async = tool == CODEX_ASYNC_TOOL;
    if is_async && mode == Mode::Headed {
        let mut args = vec!["post".to_string(), "send".into()];
        if remote {
            args.extend(["--host".into(), p.host.clone()]);
        }
        let back = if remote {
            format!(" --host {local}")
        } else {
            String::new()
        };
        args.push(p.pane.clone());
        args.push(copy_text(me, &back, input));
        return Route::Copy(args);
    }
    if !redirects_questions(parent, mode) {
        return Route::Allow;
    }
    let host = if remote {
        format!(" --host {}", p.host)
    } else {
        String::new()
    };
    Route::Deny(deny_reason(&p.pane, &host, is_async, input))
}

/// Tells the agent to ask its parent with the exact command, and repeats the
/// questions it tried to ask so it can send them as they were. An async
/// question did not block, so the agent may go on meanwhile.
fn deny_reason(parent: &str, host: &str, is_async: bool, input: &Value) -> String {
    let then = if is_async {
        "then go on with work that does not depend on the answer"
    } else {
        "then end your turn and wait"
    };
    let mut reason = format!(
        "You were spawned by {parent}: ask it, never the user in your pane. Run: vega post send{host} {parent} \"<your question and its options>\", {then}; the answer arrives as a new message."
    );
    let questions = questions(input);
    if !questions.is_empty() {
        reason.push_str(" Your question: ");
        reason.push_str(&questions.join(" "));
    }
    reason
}

/// The parent's copy of a headed pane's async question: the user may answer
/// it in the pane, the parent by mail, which arrives when the turn ends.
fn copy_text(pane: &str, host: &str, input: &Value) -> String {
    format!(
        "Copy of an async question in my headed pane {pane}; the user may answer it there, I keep working: {} Reply: vega post send{host} {pane} \"<answer>\" (arrives when my turn ends).",
        questions(input).join(" ")
    )
}

/// Each question with its options: `question` + `{label}` options (Claude,
/// request_user_input) or `title` + string options (request_user_input_async).
fn questions(input: &Value) -> Vec<String> {
    input["questions"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|q| {
            let text = q["question"].as_str().or(q["title"].as_str())?;
            let options: Vec<&str> = q["options"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|o| o.as_str().or(o["label"].as_str()))
                .collect();
            Some(if options.is_empty() {
                text.to_string()
            } else {
                format!("{text} ({})", options.join(" / "))
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deny_reason_names_command_and_question() {
        let input = json!({"questions": [{"question": "Which scope?", "options": [{"label": "A"}, {"label": "B"}]}]});
        let r = deny_reason("w3V:p1", "", false, &input);
        assert!(r.contains("vega post send w3V:p1 \""), "{r}");
        assert!(r.contains("end your turn and wait"), "{r}");
        assert!(r.ends_with("Your question: Which scope? (A / B)"), "{r}");
        let r = deny_reason("w3V:p1", " --host deimos", false, &json!({}));
        assert!(r.contains("vega post send --host deimos w3V:p1"), "{r}");
    }

    #[test]
    fn async_questions_parse_title_and_string_options() {
        // Shape of codex 0.160+ request_user_input_async arguments.
        let input = json!({"questions": [
            {"title": "May PR runs save?", "options": ["Allow", "Keep main-only"]},
            {"title": "Is ARC enabled?"},
        ]});
        assert_eq!(
            questions(&input),
            [
                "May PR runs save? (Allow / Keep main-only)",
                "Is ARC enabled?"
            ]
        );
        let r = deny_reason("w3V:p1", "", true, &input);
        assert!(r.contains("go on with work that does not depend"), "{r}");
        assert!(!r.contains("end your turn"), "{r}");
        assert!(r.ends_with("Is ARC enabled?"), "{r}");
    }

    #[test]
    fn copy_names_reply_command() {
        let input = json!({"questions": [{"title": "Is ARC enabled?"}]});
        let c = copy_text("wDR:p2", " --host ganymede", &input);
        assert!(c.contains("headed pane wDR:p2"), "{c}");
        assert!(c.contains("Is ARC enabled?"), "{c}");
        assert!(
            c.contains("Reply: vega post send --host ganymede wDR:p2 \"<answer>\""),
            "{c}"
        );
    }

    #[test]
    fn route_copies_headed_async_and_denies_headless() {
        let input = json!({"questions": [{"title": "Is ARC enabled?", "options": ["Yes", "No"]}]});
        let local = Parent {
            pane: "w3V:p1".into(),
            host: "proteus".into(),
        };
        let remote = Parent {
            pane: "w5S:p1".into(),
            host: "ganymede".into(),
        };
        let r = |tool, p: Option<&Parent>, mode| route(tool, p, mode, "proteus", "wDR:p2", &input);
        // Headed async: allowed, the parent gets a copy.
        let Route::Copy(args) = r(CODEX_ASYNC_TOOL, Some(&local), Mode::Headed) else {
            panic!("no copy");
        };
        assert_eq!(args[..3], ["post", "send", "w3V:p1"]);
        assert!(
            args[3].contains("Is ARC enabled? (Yes / No)"),
            "{}",
            args[3]
        );
        assert!(
            args[3].contains("Reply: vega post send wDR:p2 \""),
            "{}",
            args[3]
        );
        let Route::Copy(args) = r(CODEX_ASYNC_TOOL, Some(&remote), Mode::Headed) else {
            panic!("no copy");
        };
        assert_eq!(args[..5], ["post", "send", "--host", "ganymede", "w5S:p1"]);
        assert!(
            args[5].contains("Reply: vega post send --host proteus wDR:p2 \""),
            "{}",
            args[5]
        );
        // Headed blocking dialogs stay for the user; no parent: nothing.
        assert_eq!(r(CODEX_TOOL, Some(&local), Mode::Headed), Route::Allow);
        assert_eq!(r(CLAUDE_TOOL, Some(&local), Mode::Headed), Route::Allow);
        assert_eq!(r(CODEX_ASYNC_TOOL, None, Mode::Headed), Route::Allow);
        assert_eq!(r(CODEX_ASYNC_TOOL, None, Mode::Headless), Route::Allow);
        // Headless: denied with the post send command.
        let Route::Deny(reason) = r(CODEX_ASYNC_TOOL, Some(&remote), Mode::Headless) else {
            panic!("no deny");
        };
        assert!(
            reason.contains("vega post send --host ganymede w5S:p1"),
            "{reason}"
        );
        assert!(reason.contains("go on with work"), "{reason}");
    }
}
