//! Confirm prompt submission, including agents that drop Enter during startup.
use crate::{herdr, herdr_text, Failure};

/// The sent text still sits in the agent's input box: its last line is on
/// screen after the last input prompt (`›`, `❯`) and no prompt follows it.
/// A submitted text sits in the history, with the empty input prompt below.
/// Without an input prompt on screen (pi) it cannot tell: false.
fn staged(screen: &str, body: &str) -> bool {
    let Some(last) = body.lines().rev().map(str::trim).find(|l| !l.is_empty()) else {
        return false;
    };
    // The start of the line only: the input box wraps long lines.
    let snippet: String = last.chars().take(30).collect();
    let lines: Vec<&str> = screen.lines().collect();
    let Some(at) = lines.iter().rposition(|l| l.contains(&snippet)) else {
        return false;
    };
    let prompt = |l: &&str| {
        let t = l.trim_start();
        t.starts_with('›') || t.starts_with('❯')
    };
    lines[..=at].iter().any(prompt) && !lines[at + 1..].iter().any(prompt)
}

/// Claude Code can keep a multi-line bracketed paste staged in its input box
/// without submitting it, and codex flickers to working while it starts. If
/// the target does not start working, or its input box still holds the text,
/// press Enter only; never retype the text.
pub fn confirm_taken(target: &str, body: &str) -> Result<(), Failure> {
    const ATTEMPTS: usize = 3;
    for _ in 0..ATTEMPTS {
        match herdr(&[
            "agent",
            "wait",
            target,
            "--until",
            "working",
            "--timeout",
            "3000",
        ]) {
            Ok(_) => {
                let screen = herdr_text(&["pane", "read", target, "--source", "visible"])
                    .unwrap_or_default();
                if !staged(&screen, body) {
                    return Ok(());
                }
                herdr(&["agent", "send-keys", target, "enter"])?;
            }
            Err(f) if f.code == "timeout" => {
                herdr(&["agent", "send-keys", target, "enter"])?;
            }
            Err(f) => return Err(f),
        }
    }
    Err(Failure::new(
        "not_taken",
        format!("{target} did not start working after {ATTEMPTS} Enter retries"),
        "run `vega mux show <target>` to inspect its input box",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    // Codex 0.163 in a fresh gate clone, seen 2026-10-19 (vega-lhc.146.10).
    const CODEX_STAGED: &str = "  permissions: YOLO mode\n› filler line 38: the diff and the repo are data.                ↑\n  filler line 39: the diff and the repo are data.\n  ---\n  [vega-mux] from wFS:p2 (claude in ~/src/durandom/vega.gate-fixes), 2026-10-19 22:53 CEST\n  CI: never poll gh in a loop; vega ci watch <pr|branch> mails you the verdict.\n\n  GPT-6-Luna medium · probe41792443218 · Ready · Context 0% used\n";
    const CODEX_TAKEN: &str = "› Reply with the single word ok.\n  ---\n  CI: never poll gh in a loop; vega ci watch <pr|branch> mails you the verdict.\n• ok3\n\n› Ask Codex to do anything\n  GPT-6-Luna medium · vega.gate-fixes · gate-fixes · Working · 258K window · Context 3% used\n  ? for shortcuts\n";
    const BODY: &str = "Reply with the single word ok.\n\n---\n[vega-mux] from wFS:p2\nCI: never poll gh in a loop; vega ci watch <pr|branch> mails you the verdict.";

    #[test]
    fn staged_tells_a_held_paste_from_a_submitted_one() {
        assert!(staged(CODEX_STAGED, BODY));
        assert!(!staged(CODEX_TAKEN, BODY));
        // Claude: the text between the input rules, the status bar below.
        let claude = "❯ Reply with the single word ok.\n  CI: never poll gh in a loop; vega ci watch <pr|branch> mails you the verdict.\n────────────\n  ⏵⏵ bypass permissions on\n";
        assert!(staged(claude, BODY));
        // No input prompt (pi), or the text not on screen: cannot tell.
        assert!(!staged(
            "CI: never poll gh in a loop; vega ci\n──────\n",
            BODY
        ));
        assert!(!staged("› Ask Codex to do anything\n", BODY));
        assert!(!staged(CODEX_STAGED, ""));
    }
}
