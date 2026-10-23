//! Provider aborts (docs/design/vegad.md, attention `quota`): a turn that
//! ended on a rate limit, an overload or a used-up quota. Transient ones
//! (overloaded, rate limited, at capacity) clear within minutes; an
//! exhausted quota waits for its reset. The text is the agent's last error
//! from its session log (vega-transcript), else the end of its screen.
//!
//! No I/O here: the relay passes the text, the clock and token-burn's reset.

use vega_herdr::state::Quota;
use vega_transcript::{civil, one_line, truncate, Entry, Role};

/// Characters of the error kept in the record.
const ERROR_MAX: usize = 160;

/// Lines at the end of the screen, above the input box, that may hold the
/// error: wrapped, it spans a few.
const SCREEN_LINES: usize = 8;

/// When a quota resets, as the error says it.
#[derive(Debug, Clone, PartialEq)]
pub enum Reset {
    /// Seconds from now: `Resets in 2h33m`, `try again in 2 hours 3 minutes`.
    In(i64),
    /// Unix seconds: Claude's `usage limit reached|1760000000`.
    Unix(i64),
    /// A clock as shown: `resets 3pm (Europe/Berlin)`.
    Text(String),
}

/// A classified abort, before its reset is resolved to a clock.
#[derive(Debug, Clone, PartialEq)]
pub struct Abort {
    pub transient: bool,
    pub error: String,
    pub reset: Option<Reset>,
}

/// Phrases of a used-up quota or plan limit (lowercase).
const EXHAUSTED: &[&str] = &[
    "hit your limit",
    "hit your usage limit",
    "usage limit reached",
    "5-hour limit reached",
    "weekly limit reached",
    "session limit reached",
    "usage credits are required",
    "quota exceeded",
    "exceeded your current quota",
    "insufficient_quota",
    "resource_exhausted",
    "resource exhausted",
    "exhausted your capacity",
    "quota will reset",
    "quota reached",
    "run out of credits",
    "out of credits",
];

/// Phrases of a transient abort: retry soon (lowercase).
const TRANSIENT: &[&str] = &[
    "overloaded",
    "at capacity",
    "high demand",
    "rate limit reached",
    "rate limit exceeded",
    "rate limited",
    "rate_limit_error",
    "too many requests",
    "service temporarily unavailable",
    "503 service unavailable",
];

/// The abort `text` reports, if any. A reset duration next to a 424/429, a
/// quota or a limit counts as exhausted (agy: `424 … Resets in 2h33m`).
pub fn classify(text: &str) -> Option<Abort> {
    let flat = one_line(text);
    let low = flat.to_lowercase();
    let reset = reset_of(&flat);
    // `You've reached your Fable limit`, `reached your weekly limit`.
    let reached = low.find("reached your ").is_some_and(|i| {
        low[i..]
            .split_whitespace()
            .take(5)
            .any(|w| w.starts_with("limit"))
    });
    let exhausted = reached
        || EXHAUSTED.iter().any(|p| low.contains(p))
        || (matches!(reset, Some(Reset::In(_)))
            && ["424", "429", "quota", "limit"]
                .iter()
                .any(|p| low.contains(p)));
    let transient = !exhausted && TRANSIENT.iter().any(|p| low.contains(p));
    if !exhausted && !transient {
        return None;
    }
    Some(Abort {
        transient,
        error: truncate(&flat, ERROR_MAX),
        reset: reset.filter(|_| exhausted),
    })
}

/// The abort a session log ended on: its last entry is an error.
pub fn from_entries(entries: &[Entry]) -> Option<Abort> {
    let last = entries.last().filter(|e| e.role == Role::Error)?;
    classify(&last.text)
}

/// The abort at the end of a screen: the last lines above the input box
/// (Claude's `❯`, Codex's `›`) or, without one, of the screen.
pub fn from_screen(screen: &str) -> Option<Abort> {
    let lines: Vec<&str> = screen.lines().collect();
    let boxed = (0..lines.len())
        .rev()
        .find(|&i| lines[i].trim_start().starts_with(['❯', '›']));
    let above: Vec<&str> = lines[..boxed.unwrap_or(lines.len())]
        .iter()
        .copied()
        .filter(|l| {
            let t = l.trim();
            !t.is_empty() && !t.chars().all(|c| matches!(c, '─' | '━' | '-' | '═'))
        })
        .collect();
    let tail = above[above.len().saturating_sub(SCREEN_LINES)..]
        .iter()
        .map(|l| {
            l.trim()
                .trim_start_matches(['⏺', '•', '⎿', '■', '✗'])
                .trim()
        })
        .collect::<Vec<_>>()
        .join(" ");
    classify(&tail)
}

/// The record: the reset as a local clock (`offset`: seconds east of UTC);
/// without one in the text, an exhausted quota takes `fallback` (seconds to
/// the provider's reset, from token-burn).
pub fn resolve(a: Abort, now: i64, offset: i64, fallback: Option<i64>) -> Quota {
    let resets = match a.reset {
        Some(Reset::In(s)) => Some(clock(now + s, now, offset)),
        Some(Reset::Unix(t)) => Some(clock(t, now, offset)),
        Some(Reset::Text(t)) => Some(t),
        None if !a.transient => {
            fallback.map(|s| format!("{} (token-burn)", clock(now + s, now, offset)))
        }
        None => None,
    };
    Quota {
        transient: a.transient,
        error: a.error,
        resets,
    }
}

/// `14:05` today, else `10-11 14:05`.
fn clock(t: i64, now: i64, offset: i64) -> String {
    let (_, mo, d, h, mi) = civil(t, offset);
    let (_, nmo, nd, _, _) = civil(now, offset);
    if (mo, d) == (nmo, nd) {
        format!("{h:02}:{mi:02}")
    } else {
        format!("{mo:02}-{d:02} {h:02}:{mi:02}")
    }
}

/// token-burn's provider for an aborted agent: by kind, for pi by the
/// error text (pi's provider is a spawn flag the relay does not keep).
pub fn provider(kind: &str, error: &str) -> Option<&'static str> {
    if kind != "pi" {
        return vega_herdr::capacity::provider_for(kind, &[]);
    }
    let low = error.to_lowercase();
    if low.contains("grok") || low.contains("x.ai") {
        Some("xai")
    } else if low.contains("codex") {
        Some("codex")
    } else if low.contains("z.ai") || low.contains("glm") {
        Some("zai")
    } else {
        None
    }
}

/// The reset an error names: a duration after `resets in`, `try again in`,
/// `reset after`; a clock after `resets`, `reset at`, `try again at`; or
/// Claude's `|<unix seconds>`.
fn reset_of(text: &str) -> Option<Reset> {
    let low = text.to_lowercase();
    for key in [
        "resets in ",
        "reset in ",
        "try again in ",
        "retry in ",
        "reset after ",
    ] {
        if let Some(s) = low.find(key).and_then(|i| duration(&low[i + key.len()..])) {
            return Some(Reset::In(s));
        }
    }
    if let Some(i) = low.find("limit reached|") {
        let digits: String = low[i + 14..]
            .chars()
            .take_while(char::is_ascii_digit)
            .collect();
        if let Ok(t) = digits.parse::<i64>() {
            return Some(Reset::Unix(t));
        }
    }
    // A bare `resets` only before a digit (`resets 3pm`), not in prose.
    for key in ["resets at ", "reset at ", "try again at ", "resets "] {
        let Some(rest) = low.find(key).and_then(|i| text.get(i + key.len()..)) else {
            continue;
        };
        let end = [rest.find(['·', '|']), rest.find(". "), rest.find(" /")]
            .into_iter()
            .flatten()
            .min()
            .unwrap_or(rest.len());
        let t = rest[..end].trim().trim_end_matches('.').trim();
        let lead = |c: char| c.is_ascii_digit() || (key != "resets " && c.is_ascii_alphabetic());
        if t.starts_with(lead) && t.len() <= 40 {
            return Some(Reset::Text(t.to_string()));
        }
    }
    None
}

/// Seconds of a leading duration: `2h33m`, `4h13m8s`, `2 hours 3 minutes`,
/// `~42 min`, `5 days`. `None` when it does not start with one.
fn duration(s: &str) -> Option<i64> {
    let s = s
        .trim_start_matches(['~', ' '])
        .trim_start_matches("about ");
    let mut total = 0;
    let mut found = false;
    let mut rest = s;
    loop {
        let r = rest
            .trim_start_matches([' ', ','])
            .trim_start_matches("and ");
        let n: String = r.chars().take_while(char::is_ascii_digit).collect();
        if n.is_empty() {
            break;
        }
        let after = r[n.len()..].trim_start();
        let unit: String = after
            .chars()
            .take_while(char::is_ascii_alphabetic)
            .collect();
        let secs = match unit.as_str() {
            "d" | "day" | "days" => 86400,
            "h" | "hr" | "hrs" | "hour" | "hours" => 3600,
            "m" | "min" | "mins" | "minute" | "minutes" => 60,
            "s" | "sec" | "secs" | "second" | "seconds" => 1,
            _ => break,
        };
        total += n.parse::<i64>().ok()? * secs;
        found = true;
        rest = &after[unit.len()..];
    }
    found.then_some(total)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Real errors, trimmed and without secrets: from local Claude Code,
    /// Codex and pi session logs, and (agy, Claude's limit) from the
    /// harnesses' known messages.
    const CLAUDE_OVERLOADED: &str = "API Error: 529 Overloaded. This is a server-side issue, usually temporary — try again in a moment. If it persists, check https://status.claude.com.";
    const CLAUDE_OVERLOADED_JSON: &str = r#"API Error: 529 {"type":"error","error":{"type":"overloaded_error","message":"Overloaded"},"request_id":"req_x"}"#;
    const CLAUDE_RATE: &str = "API Error: Rate limit reached";
    const CLAUDE_MODEL_LIMIT: &str =
        "You've reached your Fable limit. Run /usage-credits to continue or switch models with /model.";
    const CLAUDE_CREDITS: &str = "Usage credits are required for this model.";
    const CLAUDE_HIT_LIMIT: &str = "You've hit your limit · resets 3pm (Europe/Berlin)";
    const CLAUDE_EPOCH: &str = "Claude AI usage limit reached|1791640800";
    const CODEX_USAGE: &str = "You've hit your usage limit for GPT-5.3-Codex-Spark. Switch to another model now, or try again at May 4th, 2026 12:48 AM.";
    const CODEX_USAGE_IN: &str = "You've hit your usage limit. Upgrade to Pro (https://openai.com/chatgpt/pricing) or try again in 2 hours 3 minutes.";
    const CODEX_RETRY: &str =
        "exceeded retry limit, last status: 429 Too Many Requests, request id: x";
    const PI_CODEX_OVERLOADED: &str =
        "Codex error: Our servers are currently overloaded. Please try again later.";
    const PI_XAI_CAPACITY: &str = "Error Code null: The model is currently at capacity due to high demand. Please try again in a few minutes, or use a higher service tier for priority processing: https://docs.x.ai/developers/advanced-api-usage/priority-processing";
    const PI_XAI_CREDITS: &str = r#"OpenAI API error (403): 403 "You have run out of credits or need a Grok subscription. Add credits at https://grok.com/?_s=usage or upgrade at https://grok.com/supergrok.""#;
    const PI_COPILOT_QUOTA: &str = "429 quota exceeded\n";
    const AGY_424: &str = "Error 424: model quota reached. Resets in 2h33m.";
    const AGY_EXHAUSTED: &str =
        "You have exhausted your capacity on this model. Your quota will reset after 4h13m8s.";

    /// Ordinary failures: no quota.
    const ORDINARY: &[&str] = &[
        "Prompt is too long",
        "Not logged in · Please run /login",
        "API Error: 500 Internal server error. This is a server-side issue, usually temporary — try again in a moment.",
        "API Error: Connection closed mid-response. The response above may be incomplete.",
        "API Error: Extra usage is required for 1M context · run /extra-usage to enable, or /model to switch to standard context",
        "<html><head><title>504 Gateway Time-out</title></head></html>",
        "upstream connect error or disconnect/reset before headers. reset reason: connection termination",
        "Codex error: Your input exceeds the context window of this model. Please adjust your input and try again.",
        "WebSocket error",
        "terminated",
        "Tests pass; I committed the fix.",
    ];

    fn abort(text: &str) -> Abort {
        classify(text).unwrap_or_else(|| panic!("no abort in {text:?}"))
    }

    #[test]
    fn overload_and_rate_limits_are_transient() {
        for t in [
            CLAUDE_OVERLOADED,
            CLAUDE_OVERLOADED_JSON,
            CLAUDE_RATE,
            CODEX_RETRY,
            PI_CODEX_OVERLOADED,
            PI_XAI_CAPACITY,
        ] {
            let a = abort(t);
            assert!(a.transient, "{t}");
            assert_eq!(a.reset, None, "{t}");
        }
    }

    #[test]
    fn used_up_quotas_are_exhausted_with_their_reset() {
        let cases = [
            (
                CLAUDE_HIT_LIMIT,
                Some(Reset::Text("3pm (Europe/Berlin)".into())),
            ),
            (CLAUDE_EPOCH, Some(Reset::Unix(1791640800))),
            (CLAUDE_MODEL_LIMIT, None),
            (CLAUDE_CREDITS, None),
            (
                CODEX_USAGE,
                Some(Reset::Text("May 4th, 2026 12:48 AM".into())),
            ),
            (CODEX_USAGE_IN, Some(Reset::In(2 * 3600 + 3 * 60))),
            (PI_XAI_CREDITS, None),
            (PI_COPILOT_QUOTA, None),
            (AGY_424, Some(Reset::In(2 * 3600 + 33 * 60))),
            (AGY_EXHAUSTED, Some(Reset::In(4 * 3600 + 13 * 60 + 8))),
        ];
        for (t, reset) in cases {
            let a = abort(t);
            assert!(!a.transient, "{t}");
            assert_eq!(a.reset, reset, "{t}");
        }
    }

    #[test]
    fn ordinary_failures_are_no_quota() {
        for t in ORDINARY {
            assert_eq!(classify(t), None, "{t}");
        }
    }

    #[test]
    fn errors_are_one_trimmed_line() {
        assert_eq!(abort(PI_COPILOT_QUOTA).error, "429 quota exceeded");
        assert!(abort(PI_XAI_CAPACITY).error.chars().count() <= ERROR_MAX);
    }

    #[test]
    fn durations_in_several_spellings() {
        assert_eq!(duration("2h33m."), Some(9180));
        assert_eq!(duration("~42 min"), Some(2520));
        assert_eq!(duration("about 12s"), Some(12));
        assert_eq!(duration("5 days on Sep 1"), Some(5 * 86400));
        assert_eq!(duration("1 hour and 5 minutes"), Some(3900));
        assert_eq!(duration("a moment"), None);
        assert_eq!(duration("a few minutes"), None);
    }

    #[test]
    fn resolve_turns_resets_into_local_clocks() {
        // 2026-10-10 08:00 UTC, 10:00 in UTC+2.
        let now = 1791619200;
        let off = 7200;
        let q = resolve(abort(AGY_424), now, off, Some(60));
        assert_eq!(q.resets.as_deref(), Some("12:33"));
        let q = resolve(abort(CLAUDE_EPOCH), now, off, None);
        assert_eq!(q.resets.as_deref(), Some("16:00"));
        // No reset in the text: token-burn's, marked.
        let q = resolve(abort(CLAUDE_MODEL_LIMIT), now, off, Some(20 * 3600));
        assert_eq!(q.resets.as_deref(), Some("10-11 06:00 (token-burn)"));
        assert_eq!(resolve(abort(CLAUDE_CREDITS), now, off, None).resets, None);
        // Transient: retry soon, no reset.
        assert_eq!(resolve(abort(CLAUDE_RATE), now, off, Some(60)).resets, None);
    }

    #[test]
    fn the_session_log_counts_only_when_it_ends_on_the_error() {
        let e = |role, text: &str| Entry {
            role,
            time: None,
            text: text.into(),
        };
        let ended = [e(Role::User, "fix it"), e(Role::Error, CLAUDE_OVERLOADED)];
        assert!(from_entries(&ended).is_some_and(|a| a.transient));
        let recovered = [
            e(Role::Error, CLAUDE_OVERLOADED),
            e(Role::Assistant, "Done."),
        ];
        assert_eq!(from_entries(&recovered), None);
        assert_eq!(from_entries(&[e(Role::Error, "Prompt is too long")]), None);
    }

    #[test]
    fn screens_of_aborted_turns() {
        let claude = "\
⏺ Running the tests now.

⏺ Bash(cargo test)
  ⎿  API Error: 529 Overloaded. This is a server-side issue, usually
     temporary — try again in a moment. If it persists, check
     https://status.claude.com.

────────────────────────────────────────
❯
────────────────────────────────────────
  ⏵⏵ bypass permissions on · 1 shell";
        assert!(from_screen(claude).is_some_and(|a| a.transient));
        let limit = "\
⏺ I'll continue with the parser.
  ⎿  You've hit your limit · resets 3pm (Europe/Berlin)
     /upgrade to increase your usage limit.

────────────────────────────────────────
❯ ";
        let a = from_screen(limit).unwrap();
        assert_eq!(
            (a.transient, a.reset),
            (false, Some(Reset::Text("3pm (Europe/Berlin)".into())))
        );
        let codex = "\
• Updated the parser.

■ You've hit your usage limit. Upgrade to Pro (https://openai.com/chatgpt/pricing) or try
again in 2 hours 3 minutes.

› Implement {feature}

  100% context left · ? for shortcuts";
        let a = from_screen(codex).unwrap();
        assert_eq!((a.transient, a.reset), (false, Some(Reset::In(7380))));
        // A normal end, and a draft in the box that quotes an error.
        let fine = "⏺ All tests pass.\n\n───\n❯ API Error: 529 Overloaded\n───";
        assert_eq!(from_screen(fine), None);
    }

    #[test]
    fn pi_provider_from_the_error() {
        assert_eq!(provider("claude", ""), Some("claude"));
        assert_eq!(provider("agy", ""), Some("antigravity"));
        assert_eq!(provider("pi", PI_XAI_CREDITS), Some("xai"));
        assert_eq!(provider("pi", PI_CODEX_OVERLOADED), Some("codex"));
        assert_eq!(provider("pi", PI_COPILOT_QUOTA), None);
    }
}
