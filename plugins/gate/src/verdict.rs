//! `vega gate verdict pass|fail [--findings <jsonl>]` (gate.md section 5).

use std::time::Duration;

use serde_json::Value;

use crate::state::{self, Review};
use crate::{record, run, run_ok, segments::short, start, tool};

#[derive(Debug, Clone, PartialEq)]
pub struct Finding {
    pub file: String,
    pub line: i64,
    pub rubric: String,
    pub principle: String,
    pub score: i64,
    pub fix: String,
}

impl Finding {
    /// The bead notes line: `file:line rubric/principle score: fix`.
    pub fn line(&self) -> String {
        format!(
            "{}:{} {}/{} {}: {}",
            self.file, self.line, self.rubric, self.principle, self.score, self.fix
        )
    }
}

/// JSON lines `{file, line, rubric, principle, score, fix}`; anything else
/// is an error naming the line. Prose is never recorded.
pub fn parse_findings(text: &str) -> Result<Vec<Finding>, String> {
    let mut out = Vec::new();
    for (n, l) in text
        .lines()
        .enumerate()
        .filter(|(_, l)| !l.trim().is_empty())
    {
        let v: Value =
            serde_json::from_str(l).map_err(|e| format!("findings line {}: {e}", n + 1))?;
        let s = |k: &str| -> Result<String, String> {
            match v.get(k) {
                Some(Value::String(s)) => Ok(s.clone()),
                Some(Value::Number(x)) => Ok(x.to_string()),
                _ => Err(format!("findings line {}: no {k}", n + 1)),
            }
        };
        let score = v
            .get("score")
            .and_then(Value::as_i64)
            .filter(|s| (0..=3).contains(s))
            .ok_or(format!("findings line {}: score must be 0-3", n + 1))?;
        out.push(Finding {
            file: s("file")?,
            line: v.get("line").and_then(Value::as_i64).unwrap_or(0),
            rubric: s("rubric")?,
            principle: s("principle")?,
            score,
            fix: s("fix").unwrap_or_default(),
        });
    }
    Ok(out)
}

/// The caller is the reviewer pane `gate start` spawned: a state file for
/// its `HERDR_PANE_ID`, and herdr's session id of that pane equal to the
/// caller's harness session id. A pi reviewer exports no session id; its
/// pane and state file must do.
pub fn authorize() -> Result<Review, String> {
    let pane = std::env::var("HERDR_PANE_ID").unwrap_or_default();
    let refuse = |why: &str| {
        let owners: Vec<String> = state::all().into_iter().map(|r| r.pane).collect();
        let who = if owners.is_empty() {
            "the reviewer pane".to_string()
        } else {
            format!("the reviewer pane {}", owners.join(" or "))
        };
        Err(format!("only {who} gives the verdict ({why})"))
    };
    if pane.is_empty() {
        return refuse("no HERDR_PANE_ID");
    }
    let Some(r) = state::load(&pane) else {
        return refuse(&format!("pane {pane} runs no gate review"));
    };
    let caller = ["CLAUDE_CODE_SESSION_ID", "CODEX_THREAD_ID"]
        .iter()
        .find_map(|k| std::env::var(k).ok().filter(|v| !v.is_empty()));
    let herdr = run_ok(
        &tool("herdr"),
        &["pane", "get", &pane],
        None,
        Some(Duration::from_secs(5)),
    )
    .ok()
    .and_then(|o| serde_json::from_str::<Value>(&o).ok())
    .and_then(|v| {
        v.pointer("/result/pane/agent_session/value")
            .and_then(Value::as_str)
            .map(String::from)
    });
    match (caller, herdr) {
        (Some(c), Some(h)) if c == h => Ok(r),
        (None, _) if r.kind == "pi" => Ok(r),
        (Some(c), Some(h)) => refuse(&format!("session {c} is not the pane's {h}")),
        (_, None) => refuse("herdr knows no session for this pane"),
        (None, Some(_)) => refuse("no harness session id"),
    }
}

/// Records the verdict, mails the author, starts the cleanup. Returns the
/// lines to print.
pub fn give(pass: bool, findings: &[Finding], r: &Review) -> Result<Vec<String>, String> {
    if pass {
        if let Some(f) = findings.iter().find(|f| f.score == 0) {
            return Err(format!(
                "pass with a 0 finding ({}): a 0 blocks; give fail, or fix the score",
                f.line()
            ));
        }
    }
    let guard = state::lock(&r.pane)?;
    // Authorization may have raced with a new start. Never follow a pane's
    // state to another head/generation, or record against its replacement.
    if state::load(&r.pane).as_ref() != Some(r) {
        return Err(
            "review superseded or finished; only the current head/generation gives a verdict"
                .into(),
        );
    }
    let verdict = if pass { "pass" } else { "fail" };
    let notes: String = findings
        .iter()
        .map(|f| f.line())
        .collect::<Vec<_>>()
        .join("\n");
    let reason = format!(
        "{verdict} by {}/{} ({} findings), head {}",
        r.kind,
        r.model,
        findings.len(),
        short(&r.head)
    );
    for b in &r.beads {
        record::finish(&r.repo, b, verdict, &reason, Some(&notes))?;
    }
    // The review is over: `vega gate` must not list it while the pane closes.
    state::remove(&r.pane);
    let text = mail_text(pass, r, findings, start::is_sentinel(&r.author_pane));
    // As a tool, not as this pane: the pane closes, so the mail offers no reply.
    let mailed = !r.author_pane.is_empty()
        && run(
            &tool("vega"),
            &[
                "post",
                "send",
                "--from-tool",
                "vega-gate",
                &r.author_pane,
                &text,
            ],
            None,
            Some(Duration::from_secs(15)),
        )
        .is_ok_and(|o| o.ok);
    start::cleanup_detached(r);
    // A replacing start may now close this finished pane immediately: all
    // verdict side effects, including author mail, are complete.
    drop(guard);
    Ok(vec![
        format!("verdict: {verdict}"),
        format!("beads: {}", r.beads.join(", ")),
        format!("findings: {}", findings.len()),
        format!(
            "author: {}",
            if mailed {
                format!("{} mailed", r.author_pane)
            } else {
                format!("{} not mailed (vega post send failed)", r.author_pane)
            }
        ),
        "cleanup: this pane closes in 5 s; clone and state removed".into(),
    ])
}

/// The one verdict mail: the verdict, every finding inline (the reviewer's
/// files stay in its clone), and the author's next step by role.
pub fn mail_text(pass: bool, r: &Review, findings: &[Finding], sentinel: bool) -> String {
    let mut t = format!(
        "gate {} for {}@{} ({}/{}), {} findings",
        if pass { "pass" } else { "fail" },
        r.branch,
        short(&r.head),
        r.kind,
        r.model,
        findings.len()
    );
    for f in findings {
        t.push_str(&format!("\n- {}", f.line()));
    }
    let fix = findings.iter().any(|f| f.score <= 1);
    let next = match (pass, sentinel) {
        (true, true) if fix => "fix the score-1 findings in this change, commit, then vega gate start; name the verdict in your done report",
        (true, true) => "name the verdict in your done report",
        (true, false) if fix => "fix the score-1 findings in this change, then push again",
        (true, false) => "push again",
        (false, true) => "fix the findings, commit, then vega gate start again",
        (false, false) => "fix the findings, commit, then push again",
    };
    t.push_str(&format!(
        "\nNext: {next}. The reviewer's tab is closed; nothing to reply."
    ));
    t
}

#[cfg(test)]
mod tests {
    use super::*;

    fn review() -> Review {
        Review {
            pane: "w1:p3".into(),
            repo: "/r".into(),
            branch: "gate-fixes".into(),
            target: "origin/main".into(),
            head: "0123456789abcdef".into(),
            base: "a".into(),
            keys: vec![],
            beads: vec![],
            review: "rv".into(),
            author_pane: "w1:p2".into(),
            clone: "/c".into(),
            work: "/w".into(),
            kind: "codex".into(),
            model: "gpt-6-luna".into(),
            tab: "gate-fixes".into(),
            started_at: 0,
            timeout_secs: 60,
        }
    }

    #[test]
    fn one_mail_holds_the_findings_and_the_step_by_role() {
        let f = parse_findings(
            r#"{"file":"a.rs","line":3,"rubric":"vega","principle":"5","score":1,"fix":"add a test"}"#,
        )
        .unwrap();
        let sentinel = mail_text(true, &review(), &f, true);
        assert!(sentinel.starts_with("gate pass for gate-fixes@0123456 (codex/gpt-6-luna), 1 findings\n- a.rs:3 vega/5 1: add a test\n"));
        assert!(sentinel.contains("name the verdict in your done report"));
        assert!(!sentinel.contains("push"));
        assert!(mail_text(true, &review(), &[], false).contains("Next: push again."));
        let fail = mail_text(false, &review(), &f, true);
        assert!(fail.contains("then vega gate start again"), "{fail}");
        assert!(!fail.contains("push"));
        assert!(mail_text(false, &review(), &f, false).contains("then push again"));
    }

    #[test]
    fn findings_are_strict_json_lines() {
        let ok = r#"{"file":"a.rs","line":3,"rubric":"vega","principle":"5","score":1,"fix":"add a test"}

{"file":"b.rs","line":1,"rubric":"vega","principle":4,"score":3}"#;
        let f = parse_findings(ok).unwrap();
        assert_eq!(f.len(), 2);
        assert_eq!(f[0].line(), "a.rs:3 vega/5 1: add a test");
        assert_eq!(f[1].principle, "4");
        assert!(parse_findings("looks good to me").is_err());
        assert!(parse_findings(r#"{"file":"a","rubric":"r","principle":"1","score":7}"#).is_err());
        assert!(parse_findings(r#"{"rubric":"r","principle":"1","score":1}"#).is_err());
    }
}
