//! Supervision (docs/design/vegad.md): the relay watches the children that
//! `vega mux spawn` recorded on this machine and wakes their parent.
//!
//! - A child that turns `working` took a prompt. If it then turns free
//!   (`idle`/`done`) and has neither reported (`vega post done`) nor
//!   replied to its parent since its last new work, it is `silent`.
//! - Only the parent settles a child: new work from the parent clears its
//!   report, reply and silence (state::settle). A child working on its own,
//!   e.g. when a background shell finishes, changes nothing.
//! - The parent gets one line when it is free and at least one of its
//!   children has a new attention; nothing repeats while the set of
//!   attentions is unchanged. Absence proves nothing: a child whose pane or
//!   status cannot be checked raises nothing from that.
//!
//! No I/O here: the relay feeds statuses in and writes the silent records.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::time::{Duration, Instant};

use vega_herdr::state::{for_user, Attention, Mode, Silence};

/// The attentions of one parent's children: (child pane, attention).
pub type Attentions = BTreeSet<(String, Attention)>;

#[derive(Default)]
pub struct Supervisor {
    last: HashMap<String, String>,
    /// Took a prompt since it was last free.
    busy: HashSet<String>,
    /// Panes herdr knows; meaningful once `synced`.
    live: HashSet<String>,
    synced: bool,
    notified: HashMap<String, Attentions>,
    /// Went free after a prompt without a report, since when: silent once
    /// `ripe` confirms it (the status bar needs a moment to show a
    /// background shell).
    suspects: HashMap<String, Instant>,
}

fn is_free(s: &str) -> bool {
    matches!(s, "idle" | "done")
}

fn is_busy(s: &str) -> bool {
    matches!(s, "working" | "blocked")
}

impl Supervisor {
    /// Full picture: every live pane.
    pub fn snapshot<'a>(&mut self, panes: impl IntoIterator<Item = &'a String>) {
        self.live = panes.into_iter().cloned().collect();
        self.synced = true;
    }

    pub fn gone(&mut self, pane: &str) {
        self.live.remove(pane);
        self.last.remove(pane);
        self.busy.remove(pane);
        self.suspects.remove(pane);
    }

    /// `None` until the first snapshot: absence proves nothing.
    pub fn alive(&self, pane: &str) -> Option<bool> {
        self.synced.then(|| self.live.contains(pane))
    }

    /// A child's status; `reported`: it has a done report or a reply to its
    /// parent since its last new work. True when it just went silent.
    pub fn observe(&mut self, pane: &str, status: &str, reported: bool) -> bool {
        self.live.insert(pane.to_string());
        if self.last.get(pane).map(String::as_str) == Some(status) {
            return false;
        }
        self.last.insert(pane.to_string(), status.to_string());
        if is_busy(status) {
            self.busy.insert(pane.to_string());
            self.suspects.remove(pane);
            false
        } else if is_free(status) && self.busy.remove(pane) && !reported {
            self.suspects.insert(pane.to_string(), Instant::now());
            true
        } else {
            false
        }
    }

    /// Suspects free for at least `after`, to confirm as silent now.
    pub fn ripe(&mut self, after: Duration) -> Vec<String> {
        let mut out: Vec<String> = self
            .suspects
            .iter()
            .filter(|(_, t)| t.elapsed() >= after)
            .map(|(p, _)| p.clone())
            .collect();
        out.sort();
        for p in &out {
            self.suspects.remove(p);
        }
        out
    }

    /// Whether `parent` has an attention it was not told about. Forgets
    /// attentions that went away, so they count as new if they come back.
    pub fn due(&mut self, parent: &str, current: &Attentions) -> bool {
        let told = self.notified.entry(parent.to_string()).or_default();
        told.retain(|a| current.contains(a));
        current.iter().any(|a| !told.contains(a))
    }

    /// What a parent was told before a relay restart, once per parent:
    /// a restart must not repeat the line for unchanged attentions.
    pub fn seed(&mut self, parent: &str, told: impl FnOnce() -> Attentions) {
        if !self.notified.contains_key(parent) {
            self.notified.insert(parent.to_string(), told());
        }
    }

    /// The parent got its line for `current`.
    pub fn woke(&mut self, parent: &str, current: &Attentions) {
        self.notified.insert(parent.to_string(), current.clone());
    }

    /// What the next image of an upgrading relay needs to tell a silent
    /// child: last status, who took a prompt, suspects with their age.
    /// Liveness comes back with the first snapshot, the wake memory from
    /// disk (`seed`).
    pub fn export(&self) -> serde_json::Value {
        let mut busy: Vec<&String> = self.busy.iter().collect();
        busy.sort();
        let suspects: serde_json::Map<String, serde_json::Value> = self
            .suspects
            .iter()
            .map(|(p, t)| (p.clone(), (t.elapsed().as_millis() as u64).into()))
            .collect();
        serde_json::json!({"last": self.last, "busy": busy, "suspects": suspects})
    }

    /// Takes over what `export` saved; anything unreadable is skipped.
    pub fn import(&mut self, v: &serde_json::Value) {
        for (p, s) in v["last"].as_object().into_iter().flatten() {
            if let Some(s) = s.as_str() {
                self.last.insert(p.clone(), s.to_string());
            }
        }
        for p in v["busy"].as_array().into_iter().flatten() {
            if let Some(p) = p.as_str() {
                self.busy.insert(p.to_string());
            }
        }
        for (p, age) in v["suspects"].as_object().into_iter().flatten() {
            let age = Duration::from_millis(age.as_u64().unwrap_or(0));
            let since = Instant::now().checked_sub(age).unwrap_or_else(Instant::now);
            self.suspects.insert(p.clone(), since);
        }
    }
}

/// Whether an attention belongs in the set that wakes the parent. A parked
/// child waits on a decision: it leaves the set, so its earlier attention is
/// forgotten and `decided` counts as new when the bead closes. A waiting
/// child (silent, but `vega ci watch` mails it the verdict) leaves it too:
/// when the watch ends without a report, `silent` counts as new. A headed
/// child's question, silence or dialog is for the owner working in it
/// (state::for_user): it stays in `vega uac` but leaves the set.
pub fn wakes(a: Attention, mode: Mode) -> bool {
    !matches!(a, Attention::Parked | Attention::Waiting) && !for_user(a, mode)
}

/// Join an unanswered target only with resolved host and current mode evidence.
pub fn owes_reply(
    asks: &[vega_herdr::gather::Ask],
    child_host: Option<&str>,
    pane: &str,
    mode: Option<Mode>,
) -> bool {
    mode.is_some()
        && child_host
            .is_some_and(|host| asks.iter().any(|a| vega_herdr::gather::owes(a, host, pane)))
}

/// One actionable transition: a known idle child owes an explicit ask reply.
/// Existing input/decision/question/failure evidence retains precedence.
pub fn unanswered(a: Option<Attention>, status: Option<&str>, owes: bool) -> Option<Attention> {
    if owes
        && status.is_some_and(is_free)
        && matches!(
            a,
            None | Some(Attention::Silent | Attention::Completed | Attention::Waiting)
        )
    {
        Some(Attention::Unanswered)
    } else {
        a
    }
}

/// Whether the pane's status bar says a background shell runs: Claude Code
/// shows `· 1 shell ·` (or `2 shells`, `1 shell still running`) in its last
/// lines. Other harnesses: no sign, so false.
pub fn background_running(screen: &str) -> bool {
    let lines: Vec<&str> = screen.lines().filter(|l| !l.trim().is_empty()).collect();
    lines[lines.len().saturating_sub(6)..].iter().any(|l| {
        l.split('·').any(|seg| {
            let mut words = seg.split_whitespace();
            matches!(
                (words.next(), words.next()),
                (Some(n), Some(w)) if n.parse::<u32>().is_ok_and(|n| n > 0) && w.starts_with("shell")
            )
        })
    })
}

/// What the finished turn on a free pane ended with: its last output
/// paragraph (wrapped lines joined) and, when that ends with `?`, the
/// question. Reads the transcript above Claude Code's input box (the rule
/// line before the last `❯`), else above the last `›` line (Codex); the
/// box's draft never counts. Skips the turn's footer (`✻ Baked for 5s`) and
/// rule lines. A paragraph that is the user's prompt (`❯ …`) is no answer.
#[cfg(test)]
pub fn turn_end(screen: &str) -> Silence {
    read_turn_end(screen).unwrap_or_default()
}

/// Like `turn_end`, but `None` while no input box is on screen (the agent is
/// still redrawing): the caller reads again instead of taking the status
/// bar for the turn's last line.
pub fn read_turn_end(screen: &str) -> Option<Silence> {
    let lines: Vec<&str> = screen.lines().collect();
    let is_rule = |l: &str| {
        let t = l.trim();
        !t.is_empty() && t.chars().all(|c| matches!(c, '─' | '━' | '-' | '═'))
    };
    let prompt = |l: &str, p: char| l.trim_start().starts_with(p);
    let end = (0..lines.len())
        .rev()
        .find(|&i| prompt(lines[i], '❯') && i > 0 && is_rule(lines[i - 1]))
        .map(|i| i - 1)
        .or_else(|| (0..lines.len()).rev().find(|&i| prompt(lines[i], '›')))?;
    // The turn's footer (`✻ Baked for 5s`) and Claude's effort hint, shown
    // above the box for a moment after a turn (`◐ medium · /effort`).
    let footer = |l: &str| {
        l.trim_start()
            .starts_with(['✻', '✢', '✳', '✶', '✽', '◐', '◑', '◒', '◓'])
    };
    let body: Vec<&str> = lines[..end]
        .iter()
        .copied()
        .filter(|l| !footer(l) && !is_rule(l))
        .collect();
    let Some(last) = body.iter().rposition(|l| !l.trim().is_empty()) else {
        return Some(Silence::default());
    };
    let first = body[..last]
        .iter()
        .rposition(|l| l.trim().is_empty())
        .map_or(0, |i| i + 1);
    let para = &body[first..=last];
    let text = para
        .iter()
        .map(|l| l.trim().trim_start_matches(['⏺', '•']).trim())
        .collect::<Vec<_>>()
        .join(" ");
    let user = para[0].trim_start().starts_with(['❯', '›', '>']);
    let tail = para[para.len().saturating_sub(2)..]
        .iter()
        .map(|l| l.trim().trim_start_matches(['⏺', '•']).trim())
        .collect::<Vec<_>>()
        .join(" ");
    Some(Silence {
        question: (!user && text.ends_with('?')).then_some(text),
        last: tail,
        quota: None,
    })
}

/// The wake line's subject, e.g. `2 sentinels need you`, `all 3
/// sentinels done` or `1 sentinel asks: <question>`. `children`: how many
/// the parent has in all; `question`: an asking child's question.
pub fn summary(
    current: &Attentions,
    children: usize,
    question: impl Fn(&str) -> Option<String>,
) -> String {
    let n = current.len();
    let asks = current
        .iter()
        .find(|(_, a)| *a == Attention::Asking)
        .map(|(p, _)| question(p).unwrap_or_default());
    if let Some(q) = asks {
        let q = short(&q, 80);
        return if n == 1 {
            format!("1 sentinel asks: {q}")
        } else {
            format!("{n} sentinels need you; one asks: {q}")
        };
    }
    if current.iter().any(|(_, a)| *a == Attention::Unanswered) {
        return format!("{n} sentinel{} need attention; an idle sentinel owes a post ask reply (vega orchestration)",if n==1 {""} else {"s"});
    }
    if n > 0 && current.iter().all(|(_, a)| *a == Attention::Decided) {
        return if n == 1 {
            "a decision closed; resume your parked sentinel".into()
        } else {
            format!("{n} decisions closed; resume your parked sentinels")
        };
    }
    let all_done = n == children && current.iter().all(|(_, a)| *a == Attention::Completed);
    let s = if n == 1 { "" } else { "s" };
    if all_done {
        if n == 1 {
            "your sentinel is done".into()
        } else {
            format!("all {n} sentinels done")
        }
    } else {
        format!("{n} sentinel{s} need{} you", if n == 1 { "s" } else { "" })
    }
}

/// At most `max` characters, the cut marked with `…`.
pub fn short(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(max - 1).collect::<String>())
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn unanswered_join_needs_a_resolved_host_and_current_mode() {
        use super::*;
        let mut ask = vega_herdr::gather::Ask {
            asker: "parent".into(),
            targets: vec!["io-wsl/child".into()],
            answered: vec![],
            deadline_ms: 1,
            question: "status?".into(),
            created_ms: 1,
        };
        for mode in [Mode::Headed, Mode::Headless] {
            assert!(owes_reply(
                &[ask.clone()],
                Some("io-wsl"),
                "child",
                Some(mode)
            ));
        }
        assert!(!owes_reply(&[ask.clone()], Some("io-wsl"), "child", None));
        assert!(!owes_reply(
            &[ask.clone()],
            None,
            "child",
            Some(Mode::Headless)
        ));
        assert!(!owes_reply(
            &[ask.clone()],
            Some("io"),
            "child",
            Some(Mode::Headless)
        ));
        assert!(!owes_reply(
            &[ask.clone()],
            Some("io-wsl"),
            "other",
            Some(Mode::Headless)
        ));
        ask.answered = ask.targets.clone();
        assert!(!owes_reply(
            &[ask],
            Some("io-wsl"),
            "child",
            Some(Mode::Headless)
        ));
    }
    use super::*;
    use Attention::*;

    #[test]
    fn unanswered_wakes_once_per_actionable_transition_and_preserves_headed_routing() {
        let mut sup = Supervisor::default();
        let current = |status, owes, mode| {
            unanswered(None, Some(status), owes)
                .filter(|a| wakes(*a, mode))
                .map(|a| set(&[("child", a)]))
                .unwrap_or_default()
        };
        let first = current("idle", true, Mode::Headless);
        assert!(sup.due("parent", &first));
        sup.woke("parent", &first);
        assert!(!sup.due("parent", &first));
        assert!(current("idle", true, Mode::Headed).is_empty());
        assert!(current("unknown", true, Mode::Headless).is_empty());
        assert!(!sup.due("parent", &current("working", true, Mode::Headless)));
        assert!(sup.due("parent", &first));
        assert_eq!(
            unanswered(Some(Attention::Parked), Some("idle"), true),
            Some(Attention::Parked)
        );
        assert_eq!(unanswered(None, Some("idle"), false), None);
    }

    #[test]
    fn export_import_keeps_silence_tracking() {
        let mut a = Supervisor::default();
        a.observe("w1:p2", "working", false);
        a.observe("w1:p3", "working", false);
        assert!(a.observe("w1:p3", "idle", false), "p3 went silent");
        let mut b = Supervisor::default();
        b.import(&a.export());
        assert!(
            !b.observe("w1:p2", "working", false),
            "same status: nothing new"
        );
        assert!(
            b.observe("w1:p2", "idle", false),
            "busy carried over: silent now"
        );
        assert_eq!(b.ripe(Duration::ZERO), vec!["w1:p2", "w1:p3"]);
    }

    #[test]
    fn headed_asking_does_not_wake_headless_unchanged() {
        for a in [Asking, Silent, Input] {
            assert!(wakes(a, Mode::Headless), "{a:?}");
            assert!(!wakes(a, Mode::Headed), "{a:?}");
        }
        for a in [Exited, Failure, Quota, Completed, Decided] {
            assert!(wakes(a, Mode::Headed), "{a:?}");
            assert!(wakes(a, Mode::Headless), "{a:?}");
        }
        for a in [Parked, Waiting] {
            assert!(!wakes(a, Mode::Headed) && !wakes(a, Mode::Headless));
        }
        // Three headed panes ask, one headless finished: only the latter is
        // due, and a later headed question adds nothing.
        let mut s = Supervisor::default();
        let modes = [
            ("a", Mode::Headed),
            ("b", Mode::Headed),
            ("c", Mode::Headless),
        ];
        let current = |items: &[(&str, Attention)]| -> Attentions {
            items
                .iter()
                .filter(|(p, a)| wakes(*a, modes.iter().find(|m| m.0 == *p).unwrap().1))
                .map(|(p, a)| (p.to_string(), *a))
                .collect()
        };
        let one = current(&[("a", Asking), ("b", Silent), ("c", Completed)]);
        assert_eq!(one, set(&[("c", Completed)]));
        assert!(s.due("p", &one));
        s.woke("p", &one);
        let two = current(&[("a", Asking), ("b", Asking), ("c", Completed)]);
        assert!(!s.due("p", &two));
        assert!(!s.due("q", &current(&[("a", Asking), ("b", Input)])));
    }

    fn set(items: &[(&str, Attention)]) -> Attentions {
        items.iter().map(|(p, a)| (p.to_string(), *a)).collect()
    }

    #[test]
    fn no_input_box_yet_reads_again() {
        // Seen live: 3 s after the turn the box was not drawn yet, and the
        // status bar became the "last line".
        let hint = "⏺ Soll ich rot oder blau nehmen?\n  ◐ medium · /effort\n────\n❯ \n────\n";
        let s = read_turn_end(hint).unwrap();
        assert_eq!(
            s.question.as_deref(),
            Some("Soll ich rot oder blau nehmen?")
        );
        let redrawing = "⏺ Soll ich rot oder blau nehmen?\n  ◐ medium · /effort\n";
        assert_eq!(read_turn_end(redrawing), None);
        let drawn = "⏺ Soll ich rot oder blau nehmen?\n✻ Sautéed for 2s\n────\n❯ \n────\n  ◐ medium · /effort\n";
        let s = read_turn_end(drawn).unwrap();
        assert_eq!(
            s.question.as_deref(),
            Some("Soll ich rot oder blau nehmen?")
        );
    }

    #[test]
    fn idle_after_a_prompt_without_report_is_silent() {
        let mut s = Supervisor::default();
        assert!(!s.observe("c", "idle", false)); // start: nothing known
        assert!(!s.observe("c", "working", false));
        assert!(!s.observe("c", "working", false));
        assert!(s.observe("c", "done", false));
        // Idle again without a new prompt: not a new silence.
        assert!(!s.observe("c", "idle", false));
    }

    #[test]
    fn a_report_or_reply_is_not_silent() {
        let mut s = Supervisor::default();
        s.observe("c", "working", false);
        assert!(!s.observe("c", "idle", true));
        // A background shell wakes it briefly: still reported, not silent.
        s.observe("c", "working", true);
        assert!(!s.observe("c", "idle", true));
    }

    #[test]
    fn background_shell_in_the_status_bar() {
        let bar =
            "❯\n──\n  🧠 Haiku 5.5  📁 vega\n  ⏵⏵ bypass permissions on · 1 shell · ← for agents\n";
        assert!(background_running(bar));
        assert!(background_running("x\n  2 shells still running\n"));
        assert!(!background_running(
            "  ⏵⏵ bypass permissions on · ← for agents\n"
        ));
        assert!(!background_running("  0 shells · ok\n"));
        // Only the last lines count: an old transcript line does not.
        let old = format!("· 1 shell ·\n{}", "line\n".repeat(10));
        assert!(!background_running(&old));
    }

    #[test]
    fn alive_is_unknown_until_synced() {
        let mut s = Supervisor::default();
        assert_eq!(s.alive("c"), None);
        s.snapshot(&["c".to_string()]);
        assert_eq!(s.alive("c"), Some(true));
        s.gone("c");
        assert_eq!(s.alive("c"), Some(false));
    }

    #[test]
    fn wake_once_per_new_attention() {
        let mut s = Supervisor::default();
        let one = set(&[("a", Completed)]);
        assert!(s.due("p", &one));
        s.woke("p", &one);
        assert!(!s.due("p", &one));
        let two = set(&[("a", Completed), ("b", Silent)]);
        assert!(s.due("p", &two));
        s.woke("p", &two);
        // b settled by new work, then silent again: new.
        assert!(!s.due("p", &one));
        assert!(s.due("p", &two));
        assert!(!s.due("q", &Attentions::new()));
    }

    /// One child's attention as the relay sees it: its silent record and
    /// whether it has an active `vega ci watch`.
    fn seen(pane: &str, silent: bool, watching: bool) -> Attentions {
        let silence = vega_herdr::state::Silence::default();
        let a = vega_herdr::state::attention(
            Some(true),
            Some("idle"),
            None,
            silent.then_some(&silence),
            None,
        );
        vega_herdr::state::waiting(a, watching)
            .filter(|a| wakes(*a, Mode::Headless))
            .map(|a| (pane.to_string(), a))
            .into_iter()
            .collect()
    }

    #[test]
    fn a_child_waiting_on_ci_does_not_wake_until_the_watch_ends() {
        let mut s = Supervisor::default();
        // Idle without a report, but watching CI: waiting, no wake.
        let waiting = seen("b", true, true);
        assert!(waiting.is_empty());
        assert!(!s.due("p", &waiting));
        // The verdict mail ended the watch; still idle, no report: silent.
        let silent = seen("b", true, false);
        assert_eq!(silent, set(&[("b", Silent)]));
        assert!(s.due("p", &silent));
        s.woke("p", &silent);
        assert!(!s.due("p", &silent));
        // It watches again (a new push): out of the set; silent once more
        // after that counts as new.
        assert!(!s.due("p", &seen("b", true, true)));
        assert!(s.due("p", &silent));
        // A report beats the watch: not silent, so nothing to wait on.
        assert!(seen("b", false, true).is_empty());
    }

    #[test]
    fn a_parked_child_stops_waking_until_its_bead_closes() {
        let mut s = Supervisor::default();
        let both = set(&[("a", Completed), ("b", Completed)]);
        s.woke("p", &both);
        // b parks: it leaves the set; a alone was told already.
        let parked: Attentions = [("a", Completed), ("b", Parked)]
            .into_iter()
            .filter(|(_, x)| wakes(*x, Mode::Headless))
            .map(|(p, x)| (p.to_string(), x))
            .collect();
        assert_eq!(parked, set(&[("a", Completed)]));
        assert!(!s.due("p", &parked));
        assert!(!s.due("p", &parked));
        // A restart while parked: the woke/ record still names b, unchanged.
        let mut r = Supervisor::default();
        r.seed("p", || both.clone());
        assert!(!r.due("p", &parked));
        // The bead closes: decided is new, once.
        let decided = set(&[("a", Completed), ("b", Decided)]);
        for sup in [&mut s, &mut r] {
            assert!(sup.due("p", &decided));
            sup.woke("p", &decided);
            assert!(!sup.due("p", &decided));
        }
    }

    #[test]
    fn summary_counts_and_all_done() {
        let none = |_: &str| None;
        assert_eq!(
            summary(&set(&[("a", Input)]), 2, none),
            "1 sentinel needs you"
        );
        assert_eq!(
            summary(&set(&[("a", Input), ("b", Completed)]), 2, none),
            "2 sentinels need you"
        );
        assert_eq!(
            summary(&set(&[("a", Completed), ("b", Completed)]), 2, none),
            "all 2 sentinels done"
        );
        assert_eq!(
            summary(&set(&[("a", Completed)]), 1, none),
            "your sentinel is done"
        );
        assert_eq!(
            summary(&set(&[("a", Completed)]), 2, none),
            "1 sentinel needs you"
        );
        assert_eq!(
            summary(&set(&[("a", Decided)]), 2, none),
            "a decision closed; resume your parked sentinel"
        );
        assert_eq!(
            summary(&set(&[("a", Decided), ("b", Completed)]), 2, none),
            "2 sentinels need you"
        );
    }

    #[test]
    fn summary_names_the_question() {
        let q = |p: &str| (p == "b").then(|| "Should I also cover Hamburg?".to_string());
        assert_eq!(
            summary(&set(&[("b", Asking)]), 2, q),
            "1 sentinel asks: Should I also cover Hamburg?"
        );
        assert_eq!(
            summary(&set(&[("a", Silent), ("b", Asking)]), 2, q),
            "2 sentinels need you; one asks: Should I also cover Hamburg?"
        );
    }

    #[test]
    fn a_turn_that_ends_with_a_question_asks() {
        // Real Claude Code screens (haiku sentinel, 2026-10-09).
        let s = turn_end(include_str!("../tests/fixtures/claude-asks.txt"));
        assert_eq!(s.question.as_deref(), Some("Should I also cover Hamburg?"));
        // After a tool call; the input box holds a draft that must not count.
        let s = turn_end(include_str!(
            "../tests/fixtures/claude-asks-after-tools.txt"
        ));
        assert_eq!(
            s.question.as_deref(),
            Some("Should I run date and reply to w5Z:p1?")
        );
    }

    #[test]
    fn a_turn_without_a_question_does_not_ask() {
        let s = turn_end(include_str!("../tests/fixtures/claude-reports.txt"));
        assert_eq!(s.question, None);
        assert!(!s.last.is_empty());
        // The user's own prompt ending with `?` is no question of the agent.
        let s = turn_end("❯ what is 2+2?\n\n──\n❯ \n──\n");
        assert_eq!(s.question, None);
        // Wrapped lines join; the footer and rules are skipped.
        let s =
            turn_end("⏺ Done. Should I also cover\n  Hamburg?\n\n✻ Baked for 5s\n\n───\n❯\n───\n");
        assert_eq!(
            s.question.as_deref(),
            Some("Done. Should I also cover Hamburg?")
        );
        assert_eq!(turn_end(""), Silence::default());
    }
}
