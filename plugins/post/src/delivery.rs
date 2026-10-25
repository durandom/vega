//! When the relay types a pointer (docs/design/vegad.md, deliver when free).
//!
//! - Mail for a pane makes it pending. A pending pane gets a pointer only
//!   when its agent is free (`idle` or `done`); `working`, `blocked`,
//!   `unknown` and panes herdr does not know wait.
//! - One line covers everything pending: the pointer carries the unread count.
//! - After a pointer the pane is "pointed": no second line until the agent
//!   has been working again or has read its inbox (its ack floor moved).
//!   Polling can miss a short working phase; the ack floor cannot.
//! - Mail from the recipient's parent (its task channel) is also typed
//!   while the agent is `working`: Claude and Codex queue typed input as a
//!   follow-up, so the agent acts on it in its current turn. Never while
//!   `blocked` (a dialog) or `unknown`.
//! - Urgent mail (`send --now`) is typed at once, whatever the status.
//! - A turn that ends on a provider abort (quota.rs) may drop a pointer
//!   typed into it (pi does): the pointer no longer stands, and mail not
//!   read yet is pending again. After an exhausted quota no pointer goes in
//!   for STALE (it would only meet the next 429), unless the agent works
//!   again first (resumed, or switched to another model) or the mail is
//!   urgent; then one pointer probes, and a further abort holds it again.
//! - Safety net: mail whose last pointer was typed STALE ago and is still
//!   not read (abort undetected, input dropped, turn taken without reading)
//!   gets one more pointer when the agent is free, at most once per STALE.
//! - Each mail's arrival time is kept until read: `vega uac` shows how long
//!   the oldest unread mail waits (after a relay restart: since the restart).
//! - A pane that closes drops all of this. The senders of its unread mail
//!   (kept like the arrival times) become one bounce, which the relay checks
//!   against the broker and mails to each sender once (vega-lhc.129).
//!
//! No I/O here: the relay feeds events in and asks which panes to check.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

/// A pointed pane with new mail is re-checked (one broker call) at most this
/// often, unless an event touches it.
pub const RECHECK: Duration = Duration::from_secs(10);

/// How long a pointer may stand unread before the safety net types one
/// more, and how long an exhausted quota holds pointers. Five minutes: an
/// agent that got a pointer while free reads its inbox within a turn or
/// two; the stuck panes this guards against waited 15 to 52 minutes.
pub const STALE: Duration = Duration::from_secs(5 * 60);

#[derive(Default)]
pub struct Delivery {
    status: HashMap<String, String>,
    pending: HashSet<String>,
    urgent: HashSet<String>,
    /// Pending panes with mail from their parent: typed while working too.
    queued: HashSet<String>,
    /// Pane -> pointer state.
    pointed: HashMap<String, Pointed>,
    /// Pane -> arrival times (Unix ms) of its mail, oldest first; trimmed
    /// to the unread count at each check.
    arrivals: HashMap<String, Vec<u64>>,
    /// Pane -> when its last pointer was typed (Unix ms); outlives `pointed`
    /// until the mail is read, for the safety net.
    typed_at: HashMap<String, u64>,
    /// Pane -> when its turn aborted on an exhausted quota (Unix ms).
    exhausted: HashMap<String, u64>,
    /// Pane -> sender of each mail, oldest first, trimmed like `arrivals`;
    /// `None` for mail without a sender pane or from before a restart.
    origins: HashMap<String, Vec<Option<Origin>>>,
    /// Closed panes with unread mail, for the relay to bounce.
    bounces: Vec<Bounce>,
}

/// Who sent a mail: the bounce goes back there.
#[derive(Clone, Debug, PartialEq)]
pub struct Origin {
    pub host: String,
    /// Empty for the user (vega board) or a tool: no pane to mail.
    pub pane: String,
    pub agent: String,
}

/// A pane closed with mail it never read.
#[derive(Debug, PartialEq)]
pub struct Bounce {
    pub pane: String,
    /// Senders of the mail still kept, oldest first.
    pub origins: Vec<Option<Origin>>,
    /// Senders already told, so a retried bounce tells each once.
    pub done: Vec<Origin>,
}

impl Bounce {
    /// The senders of the `unread` newest mails, each once, oldest first.
    pub fn senders(&self, unread: u64) -> Vec<Origin> {
        let from = self.origins.len().saturating_sub(unread as usize);
        let mut out: Vec<Origin> = Vec::new();
        for o in self.origins[from..].iter().flatten() {
            if !out.iter().chain(&self.done).any(|x| x == o) {
                out.push(o.clone());
            }
        }
        out
    }
}

struct Pointed {
    /// Ack floor when the pointer was typed.
    floor: u64,
    /// Last check, `None`: check at once.
    checked: Option<Instant>,
    /// Typed while the agent worked: that turn is not a new one; the
    /// pointer stands until the agent was free and works again.
    busy: bool,
}

#[derive(Debug, PartialEq)]
pub enum Decision {
    /// Type the pointer.
    Type,
    /// Nothing unread: drop the pending state.
    Read,
    /// Keep waiting (pointed, not yet read).
    Wait,
}

pub fn is_free(status: &str) -> bool {
    matches!(status, "idle" | "done")
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

/// `since` (Unix ms) lies STALE or more before `now`.
fn older(since: u64, now: u64) -> bool {
    now.saturating_sub(since) >= STALE.as_millis() as u64
}

/// Who sent the mail, for when its pointer may be typed.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Sender {
    Other,
    /// The recipient's recorded parent: typed while working too.
    Parent,
    /// `send --now`: typed whatever the status.
    Urgent,
}

impl Delivery {
    pub fn mail(&mut self, pane: &str, from: Sender) {
        self.mail_from(pane, from, None);
    }

    /// A rescan found mail for `pane` (start, reconnect): pending again. A
    /// pane with arrivals keeps them: an extra sender-less arrival would
    /// trim the real sender off a later bounce (`decide` keeps the newest).
    pub fn rescanned(&mut self, pane: &str) {
        if self.arrivals.contains_key(pane) {
            self.pending.insert(pane.to_string());
            self.touch(pane);
        } else {
            self.mail(pane, Sender::Other);
        }
    }

    /// `mail` with its sender, for a bounce if the pane closes unread.
    pub fn mail_from(&mut self, pane: &str, from: Sender, origin: Option<Origin>) {
        self.arrivals
            .entry(pane.to_string())
            .or_default()
            .push(now_ms());
        self.origins
            .entry(pane.to_string())
            .or_default()
            .push(origin);
        self.pending.insert(pane.to_string());
        match from {
            Sender::Urgent => self.urgent.insert(pane.to_string()),
            Sender::Parent => self.queued.insert(pane.to_string()),
            Sender::Other => false,
        };
        self.touch(pane);
    }

    /// Returns true if the status changed for a pane with pending mail
    /// (worth a log line).
    pub fn status(&mut self, pane: &str, status: &str) -> bool {
        if is_free(status) {
            if let Some(p) = self.pointed.get_mut(pane) {
                p.busy = false;
            }
        } else if status != "unknown" && self.pointed.get(pane).is_some_and(|p| !p.busy) {
            // Working or blocked: the agent took its turn; a new line may follow.
            self.pointed.remove(pane);
        }
        if status == "working" {
            // The provider answers again (resumed, or another model).
            self.exhausted.remove(pane);
        }
        let old = self.status.insert(pane.to_string(), status.to_string());
        let changed = old.as_deref() != Some(status);
        if changed {
            self.touch(pane);
        }
        changed && self.pending.contains(pane)
    }

    /// The pane's turn ended on a provider abort; `exhausted`: a used-up
    /// quota, not an overload. Returns true if mail is pending again.
    pub fn aborted(&mut self, pane: &str, exhausted: bool) -> bool {
        self.pointed.remove(pane);
        if exhausted {
            self.exhausted.insert(pane.to_string(), now_ms());
        }
        if self.arrivals.get(pane).is_some_and(|a| !a.is_empty()) {
            self.pending.insert(pane.to_string());
        }
        self.pending.contains(pane)
    }

    /// Full picture from herdr; returns the gone panes that had mail state.
    pub fn snapshot(&mut self, panes: HashMap<String, String>) -> Vec<String> {
        let mut gone: Vec<String> = self
            .known()
            .into_iter()
            .filter(|p| !panes.contains_key(p))
            .collect();
        gone.retain(|p| self.gone(p));
        gone.sort();
        for (p, s) in panes {
            self.status(&p, &s);
        }
        gone
    }

    /// Returns whether the pane had mail state to drop.
    pub fn gone(&mut self, pane: &str) -> bool {
        self.status.remove(pane);
        self.urgent.remove(pane);
        self.queued.remove(pane);
        self.typed_at.remove(pane);
        self.exhausted.remove(pane);
        let had = self.pending.remove(pane);
        let unread = self.arrivals.remove(pane).is_some();
        if let Some(origins) = self.origins.remove(pane) {
            if origins.iter().any(Option::is_some) {
                self.bounces.push(Bounce {
                    pane: pane.to_string(),
                    origins,
                    done: Vec::new(),
                });
            }
        }
        self.pointed.remove(pane).is_some() || had || unread
    }

    /// The closed panes with mail from a sender pane, each once.
    pub fn take_bounces(&mut self) -> Vec<Bounce> {
        std::mem::take(&mut self.bounces)
    }

    /// Bounces the broker could not take yet, for a later try.
    pub fn keep_bounces(&mut self, mut later: Vec<Bounce>) {
        later.append(&mut self.bounces);
        self.bounces = later;
    }

    /// The bounces waiting, oldest first.
    pub fn bounces(&self) -> &[Bounce] {
        &self.bounces
    }

    /// Panes worth a broker call now: urgent ones; pending free ones
    /// (working too for parent mail) unless pointed and checked within
    /// RECHECK, or held by an exhausted quota; and free ones the safety net
    /// re-points. `ignore_status` (relay `--once`) treats every pending
    /// pane as free.
    pub fn candidates(&self, ignore_status: bool) -> Vec<String> {
        let now = now_ms();
        let panes: HashSet<&String> = self.pending.iter().chain(self.arrivals.keys()).collect();
        let mut out: Vec<String> = panes
            .into_iter()
            .filter(|p| {
                if self.urgent.contains(*p) {
                    return true;
                }
                let status = self.status_of(p);
                let held = self.exhausted.get(*p).is_some_and(|t| !older(*t, now));
                if !self.pending.contains(*p) {
                    return !ignore_status && is_free(status) && !held && self.stale(p, now);
                }
                let free = ignore_status
                    || is_free(status)
                    || (status == "working" && self.queued.contains(*p));
                free && !held
                    && self
                        .pointed
                        .get(*p)
                        .and_then(|p| p.checked)
                        .is_none_or(|t| t.elapsed() >= RECHECK)
            })
            .cloned()
            .collect();
        out.sort();
        out
    }

    /// Given the pane's unread count and ack floor, what to do.
    pub fn decide(&mut self, pane: &str, unread: u64, floor: u64) -> Decision {
        if unread == 0 {
            self.pending.remove(pane);
            self.urgent.remove(pane);
            self.queued.remove(pane);
            self.pointed.remove(pane);
            self.arrivals.remove(pane);
            self.origins.remove(pane);
            self.typed_at.remove(pane);
            return Decision::Read;
        }
        // The inbox reads oldest first: the unread ones are the newest.
        if let Some(a) = self.arrivals.get_mut(pane) {
            let read = a.len().saturating_sub(unread as usize);
            a.drain(..read);
        }
        if let Some(o) = self.origins.get_mut(pane) {
            let read = o.len().saturating_sub(unread as usize);
            o.drain(..read);
        }
        if self.urgent.contains(pane) {
            return Decision::Type;
        }
        let stale = is_free(self.status_of(pane)) && self.stale(pane, now_ms());
        match self.pointed.get_mut(pane) {
            Some(p) if floor > p.floor => {
                self.pointed.remove(pane); // read since the last pointer
                Decision::Type
            }
            Some(_) if stale => Decision::Type, // the safety net
            Some(p) => {
                p.checked = Some(Instant::now());
                Decision::Wait
            }
            None => Decision::Type,
        }
    }

    /// The pointer was typed (or failed: `typed` false keeps it pending).
    pub fn typed(&mut self, pane: &str, floor: u64, typed: bool) {
        if typed {
            self.pending.remove(pane);
            self.urgent.remove(pane);
            self.queued.remove(pane);
            self.exhausted.remove(pane);
            self.typed_at.insert(pane.to_string(), now_ms());
            let busy = !is_free(self.status_of(pane));
            self.pointed.insert(
                pane.to_string(),
                Pointed {
                    floor,
                    checked: None,
                    busy,
                },
            );
        }
    }

    /// A pointer was typed before and its mail is not known read: the next
    /// one is a repeat (for the log).
    pub fn repeat(&self, pane: &str) -> bool {
        self.typed_at.contains_key(pane)
    }

    /// The last pointer stood STALE and mail is still kept for the pane.
    fn stale(&self, pane: &str, now: u64) -> bool {
        self.arrivals.get(pane).is_some_and(|a| !a.is_empty())
            && self.typed_at.get(pane).is_some_and(|t| older(*t, now))
    }

    /// Mail waits for this pane: pending, or a pointer it has not read yet.
    pub fn has_mail(&self, pane: &str) -> bool {
        self.pending.contains(pane) || self.pointed.contains_key(pane)
    }

    /// When the oldest unread mail for this pane arrived (Unix ms).
    pub fn unread_since(&self, pane: &str) -> Option<u64> {
        self.has_mail(pane)
            .then(|| self.arrivals.get(pane)?.first().copied())
            .flatten()
    }

    /// Panes herdr reported, with any status.
    pub fn panes(&self) -> Vec<String> {
        self.status.keys().cloned().collect()
    }

    pub fn status_of(&self, pane: &str) -> &str {
        self.status.get(pane).map_or("unknown", String::as_str)
    }

    fn known(&self) -> HashSet<String> {
        self.status
            .keys()
            .chain(&self.pending)
            .chain(self.pointed.keys())
            .chain(self.arrivals.keys())
            .cloned()
            .collect()
    }

    /// The mail state for the next image of an upgrading relay. Statuses
    /// are not kept: the first snapshot brings them back.
    pub fn export(&self) -> Value {
        let sorted = |s: &HashSet<String>| {
            let mut v: Vec<&String> = s.iter().collect();
            v.sort();
            json!(v)
        };
        let pointed: serde_json::Map<String, Value> = self
            .pointed
            .iter()
            .map(|(p, s)| (p.clone(), json!({"floor": s.floor, "busy": s.busy})))
            .collect();
        json!({
            "pending": sorted(&self.pending),
            "urgent": sorted(&self.urgent),
            "queued": sorted(&self.queued),
            "pointed": pointed,
            "arrivals": self.arrivals,
            "typed_at": self.typed_at,
            "exhausted": self.exhausted,
            "origins": self.origins.iter().map(|(p, os)| {
                let os: Vec<Value> = os.iter().map(|o| match o {
                    Some(o) => json!({"host": o.host, "pane": o.pane, "agent": o.agent}),
                    None => Value::Null,
                }).collect();
                (p.clone(), Value::from(os))
            }).collect::<serde_json::Map<String, Value>>(),
        })
    }

    /// Takes over what `export` saved; anything unreadable is skipped.
    pub fn import(&mut self, v: &Value) {
        let set = |k: &str| -> HashSet<String> {
            v[k].as_array()
                .into_iter()
                .flatten()
                .filter_map(|p| p.as_str().map(String::from))
                .collect()
        };
        let times = |k: &str| -> HashMap<String, u64> {
            v[k].as_object()
                .into_iter()
                .flatten()
                .filter_map(|(p, t)| Some((p.clone(), t.as_u64()?)))
                .collect()
        };
        self.pending.extend(set("pending"));
        self.urgent.extend(set("urgent"));
        self.queued.extend(set("queued"));
        self.typed_at.extend(times("typed_at"));
        self.exhausted.extend(times("exhausted"));
        for (p, s) in v["pointed"].as_object().into_iter().flatten() {
            if let Some(floor) = s["floor"].as_u64() {
                let busy = s["busy"].as_bool().unwrap_or(false);
                self.pointed.insert(
                    p.clone(),
                    Pointed {
                        floor,
                        checked: None,
                        busy,
                    },
                );
                // From an image without `typed_at`: the safety net counts
                // from the upgrade.
                self.typed_at.entry(p.clone()).or_insert_with(now_ms);
            }
        }
        for (p, a) in v["arrivals"].as_object().into_iter().flatten() {
            let times: Vec<u64> = a
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_u64)
                .collect();
            self.arrivals.insert(p.clone(), times);
        }
        for (p, os) in v["origins"].as_object().into_iter().flatten() {
            let os: Vec<Option<Origin>> = os
                .as_array()
                .into_iter()
                .flatten()
                .map(|o| {
                    Some(Origin {
                        host: o["host"].as_str()?.into(),
                        pane: o["pane"].as_str()?.into(),
                        agent: o["agent"].as_str()?.into(),
                    })
                })
                .collect();
            self.origins.insert(p.clone(), os);
        }
    }

    /// An event for a pointed pane makes the next check immediate.
    fn touch(&mut self, pane: &str) {
        if let Some(p) = self.pointed.get_mut(pane) {
            p.checked = None;
        }
    }
}

/// Input prompt glyphs: Claude Code (`❯`) and Codex (`›`).
const PROMPTS: [char; 2] = ['❯', '›'];

/// What the user has typed in the input box on this screen (`pane read
/// --format ansi`): a pointer would join it and submit it.
#[derive(Debug, PartialEq)]
pub enum Draft {
    /// An empty input box.
    None,
    /// Text the relay can stash and type back: lines joined with `\n` (a
    /// soft wrap reads as a line break, which keeps the words).
    Text(String),
    /// Text the screen does not hold faithfully: a paste or image
    /// placeholder, or several lines outside Claude Code.
    Opaque,
    /// No input box this parser knows (another harness, a full-screen
    /// view): the user may be typing, so it is never read as empty.
    Unknown,
}

/// Claude Code: the last line that starts with `❯`, right under a rule and
/// with a rule below. Codex: the last line that starts with `›`, with its
/// status line (`  GPT-… · dir · …` or `? for shortcuts`) below. pi: the box
/// between the last two rules (`─`), without a glyph (pi-tui's editor); a
/// glyph elsewhere is transcript. Any other screen is `Unknown`, also a bare
/// glyph (a shell prompt such as starship's `❯`).
pub fn draft(screen: &str) -> Draft {
    let lines: Vec<&str> = screen.lines().collect();
    let rule = |i: usize| is_rule(&plain(lines[i], true));
    let rules: Vec<usize> = (0..lines.len()).filter(|&i| rule(i)).collect();
    let prompt = lines
        .iter()
        .rposition(|l| plain(l, true).trim_start().starts_with(PROMPTS));
    if let Some(at) = prompt {
        let glyph = plain(lines[at], true).trim_start().chars().next();
        let claude =
            glyph == Some('❯') && at > 0 && rule(at - 1) && rules.last().is_some_and(|&r| r > at);
        let codex = glyph == Some('›')
            && lines[at + 1..]
                .iter()
                .any(|l| codex_status(&plain(l, true)));
        if claude || codex {
            return prompt_draft(&lines[at..]);
        }
    }
    match rules[..] {
        [.., top, bottom] => pi_draft(&lines[top..=bottom]),
        _ => Draft::Unknown,
    }
}

/// A line of Codex's footer: `  GPT-6.1-Sol medium · vega · Ready` or
/// `  ? for shortcuts` (seen live).
fn codex_status(line: &str) -> bool {
    line.starts_with("  ") && (line.contains(" · ") || line.trim_end().ends_with("for shortcuts"))
}

/// A full-width rule: only `─`, or pi's scroll indicator (`─── ↑ 3 more ─`).
fn is_rule(line: &str) -> bool {
    let l = line.trim();
    l.starts_with("───") && l.ends_with('─')
}

/// The box from its top to its bottom rule. pi's draft is restored only as
/// one line: ctrl+c clears its editor, but twice within 0.5 s quits pi, and
/// a soft wrap cannot be told from a line break.
fn pi_draft(boxed: &[&str]) -> Draft {
    let (top, bottom) = (plain(boxed[0], true), plain(boxed[boxed.len() - 1], true));
    let mut text: Vec<String> = boxed[1..boxed.len() - 1]
        .iter()
        .map(|l| plain(l, false).trim_end().to_string())
        .collect();
    while text.last().is_some_and(|l| l.trim().is_empty()) {
        text.pop();
    }
    while text.first().is_some_and(|l| l.trim().is_empty()) {
        text.remove(0);
    }
    if text.is_empty() {
        return Draft::None;
    }
    let text = text.join("\n");
    if text.contains('\n')
        || text.contains("[paste #")
        || top.contains(" more ")
        || bottom.contains(" more ")
    {
        Draft::Opaque
    } else {
        Draft::Text(text)
    }
}

/// From the prompt line on: the box ends at the next rule (`─`),
/// continuation lines are indented two columns. Dim text is the agent's
/// suggestion, not the user's.
fn prompt_draft(lines: &[&str]) -> Draft {
    let first = plain(lines[0], false);
    let first = first.trim_start();
    let claude = first.starts_with('❯');
    let first = first
        .trim_start_matches(PROMPTS)
        .trim_start_matches([' ', '\u{a0}']);
    // Codex has no rule under its box: the status lines below (`GPT-… ·`,
    // `? for shortcuts`) are no draft. Only its prompt line counts, and a
    // draft there is never stashed (its further lines are not knowable).
    if !claude {
        return if first.trim().is_empty() {
            Draft::None
        } else {
            Draft::Opaque
        };
    }
    let mut text: Vec<String> = vec![first.trim_end().to_string()];
    text.extend(
        lines[1..]
            .iter()
            .map(|l| plain(l, false))
            .take_while(|l| !l.trim_start().starts_with('─'))
            .map(|l| l.strip_prefix("  ").unwrap_or(&l).trim_end().to_string()),
    );
    while text.last().is_some_and(|l| l.trim().is_empty()) {
        text.pop();
    }
    let text = text.join("\n");
    if text.trim().is_empty() {
        Draft::None
    } else if text.contains("[Pasted text #") || text.contains("[Image #") {
        Draft::Opaque
    } else {
        Draft::Text(text)
    }
}

/// The text of an ANSI line; `keep_dim`: also the dim (SGR 2) text.
fn plain(line: &str, keep_dim: bool) -> String {
    let mut out = String::new();
    let mut dim = false;
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        if c != '\x1b' {
            if keep_dim || !dim {
                out.push(c);
            }
            continue;
        }
        if chars.next() != Some('[') {
            continue;
        }
        let mut params = String::new();
        let mut last = None;
        for p in chars.by_ref() {
            if ('@'..='~').contains(&p) {
                last = Some(p);
                break;
            }
            params.push(p);
        }
        if last == Some('m') {
            dim = sgr_dim(&params, dim);
        }
    }
    out
}

/// The dim state after an SGR sequence; skips 38/48 colour arguments.
fn sgr_dim(params: &str, mut dim: bool) -> bool {
    let mut it = params.split(';');
    while let Some(p) = it.next() {
        match p {
            "" | "0" | "22" => dim = false,
            "2" => dim = true,
            "38" | "48" | "58" => match it.next() {
                Some("5") => {
                    it.next();
                }
                Some("2") => {
                    it.nth(2);
                }
                _ => {}
            },
            _ => {}
        }
    }
    dim
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn export_import_keeps_pointers_and_mail_age() {
        let mut a = Delivery::default();
        a.status("w1:p1", "idle");
        a.mail("w1:p1", Sender::Other);
        assert_eq!(a.decide("w1:p1", 1, 4), Decision::Type);
        a.typed("w1:p1", 4, true);
        a.mail("w1:p2", Sender::Parent);
        a.mail("w1:p3", Sender::Urgent);
        let since = a.unread_since("w1:p1");
        let mut b = Delivery::default();
        b.import(&a.export());
        b.status("w1:p1", "idle");
        assert_eq!(b.unread_since("w1:p1"), since, "mail age survives");
        assert_eq!(
            b.decide("w1:p1", 1, 4),
            Decision::Wait,
            "pointer stands: no second line"
        );
        assert_eq!(
            b.decide("w1:p1", 1, 5),
            Decision::Type,
            "read since: a new line"
        );
        b.status("w1:p2", "working");
        assert_eq!(
            b.candidates(false),
            vec!["w1:p2", "w1:p3"],
            "parent and urgent mail kept"
        );
    }

    #[test]
    fn draft_is_undimmed_text_after_the_last_prompt() {
        let grey = "\x1b[0m\x1b[38;2;153;153;153m❯\u{a0}\x1b[0m";
        let rule = "────────";
        let line = |s: &str| Draft::Text(s.into());
        // Claude's box: a rule above and below its input line.
        let claude = |input: &str| format!("answer\n{rule}\n{input}\n{rule}\n  status");
        // Empty input; an earlier prompt in the transcript does not count.
        assert_eq!(
            draft(&format!("❯ old question\n{}", claude(grey))),
            Draft::None
        );
        // The agent's dim suggestion is not a draft.
        assert_eq!(
            draft(&claude("❯\u{a0} \x1b[0m\x1b[2mwann sind die rennen\x1b[0m")),
            Draft::None
        );
        assert_eq!(
            draft(&claude(&format!("{grey}\x1b[0mhalf a sentence"))),
            line("half a sentence")
        );
        // Seen live: codex's box, empty (dim placeholder) and with a draft.
        let status = "\n\n  \x1b[0m\x1b[38;5;6mGPT-6.1-Sol medium\x1b[0m · \x1b[0m\x1b[38;5;2mvega\x1b[0m · \x1b[0m\x1b[38;5;6mReady\x1b[0m";
        let codex = format!(
            "\x1b[0m\x1b[1m› \x1b[0m\x1b[2mAsk Codex to do anything\x1b[0m{status}\n  \x1b[0m\x1b[1m? \x1b[0mfor shortcuts\n"
        );
        assert_eq!(draft(&codex), Draft::None);
        assert_eq!(
            draft(&format!("\x1b[0m\x1b[1m› \x1b[0mhalf a sentence{status}")),
            Draft::Opaque
        );
        assert_eq!(
            draft(&format!("› codex\n  two lines{status}")),
            Draft::Opaque
        );
        // A truecolor argument of 2 is not dim.
        assert_eq!(
            draft(&claude("\x1b[38;2;2;2;2m❯ typed\x1b[0m")),
            line("typed")
        );
        assert_eq!(
            draft(&format!("{rule}\n❯ first\n  second\n\n{rule}")),
            line("first\nsecond")
        );
        assert_eq!(
            draft(&claude("❯ see [Pasted text #1 +20 lines]")),
            Draft::Opaque
        );
        assert_eq!(draft("no prompt on screen"), Draft::Unknown);
    }

    /// pi's screen as `pane read --format ansi` shows it (seen live, rules
    /// shortened): transcript, a separator, the editor box, the footer.
    fn pi(boxed: &str) -> String {
        let rule = |c: &str| format!("\x1b[0m\x1b[38;2;{c}m────────────\x1b[0m");
        let foot = "\x1b[0m\x1b[38;2;127;132;156m";
        format!(
            "Welcome to pi\n - pi-subagents\n{}\n\n{}\n{boxed}\n{}\n\
             {foot}~/src/vega (main)\x1b[0m\n\
             {foot}$0.000 (sub) 0.0%/500k (auto)   (xai) grok-4.7 • medium\x1b[0m\n\
             {foot}◆ \x1b[0m\x1b[38;2;166;173;200m1 checkpoint\x1b[0m",
            rule("249;226;175"),
            rule("148;226;213"),
            rule("148;226;213"),
        )
    }

    #[test]
    fn pi_draft_is_the_text_in_its_editor_box() {
        let cursor = "\x1b[0m\x1b[7m \x1b[0m";
        assert_eq!(draft(&pi(&format!("{cursor}            "))), Draft::None);
        assert_eq!(
            draft(&pi(&format!("half a sentence{cursor}      "))),
            Draft::Text("half a sentence".into())
        );
        // Several lines (or one soft-wrapped): not restorable, held.
        assert_eq!(
            draft(&pi(&format!("half a sentence   \nsecond line{cursor}  "))),
            Draft::Opaque
        );
        assert_eq!(draft(&pi("see [paste #1 +20 lines]")), Draft::Opaque);
        // A Claude prompt in pi's transcript is not the input line.
        let quoted = pi(&format!("my draft{cursor}")).replace(
            "Welcome to pi",
            "\x1b[38;2;153;153;153m❯\u{a0}\x1b[0mquoted claude screen",
        );
        assert_eq!(draft(&quoted), Draft::Text("my draft".into()));
        // Scrolled: lines above the box are hidden.
        let scrolled = pi("last line").replacen("────────────", "─── ↑ 3 more ───", 2);
        assert_eq!(draft(&scrolled), Draft::Opaque);
    }

    #[test]
    fn unknown_harness_screen_is_never_empty() {
        // A box drawn without rules, a bare shell prompt, nothing at all.
        for screen in [
            "╭──────╮\n│ > typing here │\n╰──────╯\n  esc to cancel",
            "user@host ~/src $ half a command",
            // A bare glyph is no known box: an empty or typed shell prompt.
            "~/src/vega main\n❯ ",
            "~/src/vega main\n❯ half a command",
            "› ",
            "",
        ] {
            assert_eq!(draft(screen), Draft::Unknown, "{screen:?}");
        }
    }

    fn d(status: &str) -> Delivery {
        let mut d = Delivery::default();
        d.status("w1:p1", status);
        d
    }

    #[test]
    fn mail_waits_while_working_then_one_line() {
        let mut d = d("working");
        d.mail("w1:p1", Sender::Other);
        d.mail("w1:p1", Sender::Other);
        d.mail("w1:p1", Sender::Other);
        assert!(d.candidates(false).is_empty());
        d.status("w1:p1", "idle");
        assert_eq!(d.candidates(false), vec!["w1:p1"]);
        assert_eq!(d.decide("w1:p1", 3, 0), Decision::Type);
        d.typed("w1:p1", 0, true);
        assert!(d.candidates(false).is_empty());
    }

    #[test]
    fn blocked_and_unknown_wait() {
        for s in ["blocked", "unknown"] {
            let mut d = d(s);
            d.mail("w1:p1", Sender::Other);
            assert!(d.candidates(false).is_empty(), "{s}");
        }
        let mut d = Delivery::default();
        d.mail("w9:p9", Sender::Other); // herdr does not know the pane
        assert!(d.candidates(false).is_empty());
        assert_eq!(d.candidates(true), vec!["w9:p9"]);
    }

    #[test]
    fn no_second_line_until_working_again() {
        let mut d = d("idle");
        d.mail("w1:p1", Sender::Other);
        assert_eq!(d.decide("w1:p1", 1, 0), Decision::Type);
        d.typed("w1:p1", 0, true);
        // More mail, agent still idle, inbox not read: wait.
        d.mail("w1:p1", Sender::Other);
        assert_eq!(d.candidates(false), vec!["w1:p1"]);
        assert_eq!(d.decide("w1:p1", 2, 0), Decision::Wait);
        // Checked just now: not a candidate until RECHECK or an event.
        assert!(d.candidates(false).is_empty());
        d.status("w1:p1", "working");
        d.status("w1:p1", "done");
        assert_eq!(d.decide("w1:p1", 2, 0), Decision::Type);
    }

    #[test]
    fn read_inbox_allows_the_next_line() {
        let mut d = d("idle");
        d.mail("w1:p1", Sender::Other);
        d.decide("w1:p1", 1, 5);
        d.typed("w1:p1", 5, true);
        d.mail("w1:p1", Sender::Other);
        // Ack floor moved: the agent read its inbox (working phase missed).
        assert_eq!(d.decide("w1:p1", 1, 6), Decision::Type);
    }

    #[test]
    fn nothing_unread_drops_pending() {
        let mut d = d("idle");
        d.mail("w1:p1", Sender::Other);
        assert_eq!(d.decide("w1:p1", 0, 3), Decision::Read);
        assert!(d.candidates(false).is_empty());
        assert!(d.candidates(true).is_empty());
    }

    #[test]
    fn urgent_goes_through_busy_and_pointed() {
        let mut d = d("working");
        d.mail("w1:p1", Sender::Urgent);
        assert_eq!(d.candidates(false), vec!["w1:p1"]);
        assert_eq!(d.decide("w1:p1", 1, 0), Decision::Type);
        d.typed("w1:p1", 0, true);
        d.mail("w1:p1", Sender::Urgent);
        assert_eq!(d.decide("w1:p1", 2, 0), Decision::Type);
    }

    #[test]
    fn parent_mail_is_queued_while_working_never_blocked() {
        for s in ["blocked", "unknown"] {
            let mut d = d(s);
            d.mail("w1:p1", Sender::Parent);
            assert!(d.candidates(false).is_empty(), "{s}");
        }
        let mut d = d("working");
        d.mail("w1:p1", Sender::Other);
        assert!(d.candidates(false).is_empty());
        d.mail("w1:p1", Sender::Parent);
        assert_eq!(d.candidates(false), vec!["w1:p1"]);
        // Blocked meanwhile (a dialog): wait, then working again: go.
        d.status("w1:p1", "blocked");
        assert!(d.candidates(false).is_empty());
        d.status("w1:p1", "working");
        assert_eq!(d.decide("w1:p1", 2, 0), Decision::Type);
        d.typed("w1:p1", 0, true);
        assert!(d.candidates(false).is_empty());
    }

    #[test]
    fn queued_pointer_stands_for_the_rest_of_the_turn() {
        let mut d = d("working");
        d.mail("w1:p1", Sender::Parent);
        d.typed("w1:p1", 0, true);
        // More parent mail in the same turn, not read yet: no second line,
        // even when herdr repeats `working` (a snapshot).
        d.mail("w1:p1", Sender::Parent);
        d.snapshot(HashMap::from([("w1:p1".into(), "working".into())]));
        assert_eq!(d.decide("w1:p1", 2, 0), Decision::Wait);
        // Read it: the next mail gets a line at once.
        assert_eq!(d.decide("w1:p1", 1, 2), Decision::Type);
        d.typed("w1:p1", 2, true);
        // Turn ends unread, then a new turn starts: a new line may follow.
        d.status("w1:p1", "done");
        d.mail("w1:p1", Sender::Other);
        assert_eq!(d.decide("w1:p1", 1, 2), Decision::Wait);
        d.status("w1:p1", "working");
        d.status("w1:p1", "idle");
        assert_eq!(d.decide("w1:p1", 1, 2), Decision::Type);
    }

    #[test]
    fn unread_since_is_the_oldest_unread_arrival() {
        let mut d = d("idle");
        assert_eq!(d.unread_since("w1:p1"), None);
        d.mail("w1:p1", Sender::Other);
        let first = d.unread_since("w1:p1").unwrap();
        d.arrivals.get_mut("w1:p1").unwrap()[0] -= 60_000;
        d.mail("w1:p1", Sender::Other);
        assert_eq!(d.unread_since("w1:p1"), Some(first - 60_000));
        // One read: the newer one is left.
        assert_eq!(d.decide("w1:p1", 1, 1), Decision::Type);
        assert!(d.unread_since("w1:p1").unwrap() >= first);
        assert_eq!(d.decide("w1:p1", 0, 2), Decision::Read);
        assert_eq!(d.unread_since("w1:p1"), None);
    }

    /// Moves the pane's pointer and abort times back by `ms`.
    fn age(d: &mut Delivery, pane: &str, ms: u64) {
        for t in [d.typed_at.get_mut(pane), d.exhausted.get_mut(pane)]
            .into_iter()
            .flatten()
        {
            *t -= ms;
        }
    }

    const STALE_MS: u64 = STALE.as_millis() as u64;

    #[test]
    fn provider_abort_drops_the_pointer_typed_into_the_turn() {
        // The repro (w8Q:p2): parent mail typed while working, the turn
        // aborts on an overload, the pane stays idle.
        let mut d = d("working");
        d.mail("w1:p1", Sender::Parent);
        assert_eq!(d.decide("w1:p1", 1, 0), Decision::Type);
        d.typed("w1:p1", 0, true);
        d.status("w1:p1", "idle");
        assert!(d.candidates(false).is_empty(), "pointer stands");
        assert!(d.aborted("w1:p1", false));
        assert_eq!(d.candidates(false), vec!["w1:p1"]);
        assert_eq!(d.decide("w1:p1", 1, 0), Decision::Type);
        // Also when the turn's working phase already took the pointer.
        let mut d = d_typed("idle");
        d.status("w1:p1", "working");
        d.status("w1:p1", "idle");
        assert!(d.aborted("w1:p1", false));
        assert_eq!(d.decide("w1:p1", 1, 0), Decision::Type);
        // Nothing kept for the pane: nothing pending.
        let mut e = Delivery::default();
        e.status("w1:p1", "idle");
        assert!(!e.aborted("w1:p1", false));
        assert!(e.candidates(false).is_empty());
    }

    fn d_typed(status: &str) -> Delivery {
        let mut d = d(status);
        d.mail("w1:p1", Sender::Other);
        d.typed("w1:p1", 0, true);
        d
    }

    #[test]
    fn exhausted_quota_holds_pointers_until_working_or_stale() {
        let mut d = d_typed("idle");
        assert!(d.aborted("w1:p1", true));
        d.mail("w1:p1", Sender::Parent);
        assert!(d.candidates(false).is_empty(), "held: exhausted");
        // Works again (resumed, another model): parent mail goes in.
        d.status("w1:p1", "working");
        assert_eq!(d.candidates(false), vec!["w1:p1"]);
        // Exhausted again: held, urgent mail still goes through.
        d.status("w1:p1", "idle");
        d.aborted("w1:p1", true);
        assert!(d.candidates(false).is_empty());
        d.mail("w1:p1", Sender::Urgent);
        assert_eq!(d.candidates(false), vec!["w1:p1"]);
        // Without a working phase (a model switched by hand): one probe
        // after STALE.
        let mut d = d_typed("idle");
        d.aborted("w1:p1", true);
        age(&mut d, "w1:p1", STALE_MS - 1_000);
        assert!(d.candidates(false).is_empty());
        age(&mut d, "w1:p1", 1_000);
        assert_eq!(d.candidates(false), vec!["w1:p1"]);
        assert_eq!(d.decide("w1:p1", 1, 0), Decision::Type);
        d.typed("w1:p1", 0, true);
        // The probe meets the quota again: held again.
        d.aborted("w1:p1", true);
        assert!(d.candidates(false).is_empty());
        // An overload does not hold.
        let mut d = d_typed("idle");
        d.aborted("w1:p1", false);
        assert_eq!(d.candidates(false), vec!["w1:p1"]);
    }

    #[test]
    fn stale_pointer_is_repeated_once_per_bound() {
        // Pointed while idle, no turn, nothing read (no new mail either).
        let mut d = d_typed("idle");
        assert!(d.candidates(false).is_empty());
        age(&mut d, "w1:p1", STALE_MS - 1_000);
        assert!(d.candidates(false).is_empty(), "within the bound");
        assert_eq!(d.decide("w1:p1", 1, 0), Decision::Wait);
        age(&mut d, "w1:p1", 1_000);
        assert_eq!(d.candidates(false), vec!["w1:p1"], "done or idle: re-point");
        assert!(d.repeat("w1:p1"));
        assert_eq!(d.decide("w1:p1", 1, 0), Decision::Type);
        d.typed("w1:p1", 0, true);
        assert!(d.candidates(false).is_empty(), "once per bound");
        d.mail("w1:p1", Sender::Other);
        assert_eq!(d.decide("w1:p1", 2, 0), Decision::Wait);
        // A turn taken without reading (the pointer no longer stands):
        // still re-pointed after the bound.
        d.status("w1:p1", "working");
        d.status("w1:p1", "done");
        d.decide("w1:p1", 2, 0);
        d.typed("w1:p1", 0, true);
        d.status("w1:p1", "working");
        d.status("w1:p1", "done");
        age(&mut d, "w1:p1", STALE_MS);
        assert_eq!(d.candidates(false), vec!["w1:p1"]);
        // Busy or blocked panes wait; read meanwhile: the check drops it.
        d.status("w1:p1", "blocked");
        assert!(d.candidates(false).is_empty());
        d.status("w1:p1", "idle");
        assert_eq!(d.decide("w1:p1", 0, 3), Decision::Read);
        assert!(!d.repeat("w1:p1"));
        assert!(d.candidates(false).is_empty());
    }

    #[test]
    fn upgrade_keeps_pointer_age_and_quota_hold() {
        let mut a = d_typed("idle");
        age(&mut a, "w1:p1", STALE_MS);
        a.mail("w1:p2", Sender::Other);
        a.typed("w1:p2", 0, true);
        a.aborted("w1:p2", true);
        let mut b = Delivery::default();
        b.import(&a.export());
        b.status("w1:p1", "idle");
        b.status("w1:p2", "idle");
        assert_eq!(b.candidates(false), vec!["w1:p1"]);
        // An older image without pointer times: counted from the upgrade.
        let mut v = a.export();
        v["typed_at"] = json!({});
        let mut c = Delivery::default();
        c.import(&v);
        c.status("w1:p1", "idle");
        assert!(c.candidates(false).is_empty());
    }

    #[test]
    fn failed_pointer_stays_pending() {
        let mut d = d("idle");
        d.mail("w1:p1", Sender::Other);
        d.typed("w1:p1", 0, false);
        assert_eq!(d.candidates(false), vec!["w1:p1"]);
    }

    #[test]
    fn closed_pane_drops_state() {
        let mut d = d("idle");
        d.mail("w1:p1", Sender::Other);
        d.typed("w1:p1", 0, true);
        d.mail("w1:p1", Sender::Other);
        assert!(d.gone("w1:p1"));
        assert!(!d.gone("w1:p1"));
        assert!(d.candidates(true).is_empty());
        assert_eq!(d.status_of("w1:p1"), "unknown");
    }

    #[test]
    fn snapshot_reports_missing_panes_as_gone() {
        let mut d = d("working");
        d.mail("w1:p1", Sender::Other);
        d.mail("w2:p1", Sender::Other);
        d.status("w3:p1", "idle"); // no mail: gone silently
        let gone = d.snapshot(HashMap::from([("w2:p1".into(), "idle".into())]));
        assert_eq!(gone, vec!["w1:p1".to_string()]);
        assert_eq!(d.candidates(false), vec!["w2:p1"]);
    }

    fn origin(host: &str, pane: &str) -> Option<Origin> {
        Some(Origin {
            host: host.into(),
            pane: pane.into(),
            agent: "claude".into(),
        })
    }

    #[test]
    fn a_closed_pane_bounces_its_unread_mail_once_per_sender() {
        let mut d = d("idle");
        d.mail_from("w30:p1", Sender::Other, origin("ganymede", "w5S:p1"));
        d.mail_from("w30:p1", Sender::Other, origin("ganymede", "w5S:p1"));
        d.mail_from("w30:p1", Sender::Other, origin("proteus", "w3V:p2"));
        d.mail("w30:p1", Sender::Other); // tool mail: nobody to bounce to
        assert!(d.gone("w30:p1"));
        let b = d.take_bounces();
        assert_eq!(b.len(), 1);
        assert_eq!(b[0].pane, "w30:p1");
        // The broker says all four are unread: two senders, each once.
        let s = b[0].senders(4);
        assert_eq!(
            s,
            vec![
                origin("ganymede", "w5S:p1").unwrap(),
                origin("proteus", "w3V:p2").unwrap()
            ]
        );
        // Only the newest two unread: the tool mail and proteus.
        assert_eq!(b[0].senders(2), vec![origin("proteus", "w3V:p2").unwrap()]);
        assert!(b[0].senders(0).is_empty());
        // Once: the state is gone with the pane.
        assert!(!d.gone("w30:p1"));
        assert!(d.take_bounces().is_empty());
    }

    #[test]
    fn a_bounce_the_broker_did_not_take_waits_for_the_next_try() {
        let mut d = Delivery::default();
        let origin = |pane: &str| Origin {
            host: "h".into(),
            pane: pane.into(),
            agent: "claude".into(),
        };
        d.mail_from("w1:p1", Sender::Other, Some(origin("w9:p1")));
        d.gone("w1:p1");
        let first = d.take_bounces();
        d.mail_from("w2:p1", Sender::Other, Some(origin("w9:p1")));
        d.gone("w2:p1");
        d.keep_bounces(first);
        let panes: Vec<&str> = d.bounces().iter().map(|b| b.pane.as_str()).collect();
        assert_eq!(panes, ["w1:p1", "w2:p1"], "oldest first");
        let mut b = d.take_bounces().remove(0);
        assert_eq!(b.senders(1).len(), 1);
        b.done.push(origin("w9:p1"));
        assert!(b.senders(1).is_empty(), "a told sender is not told again");
    }

    #[test]
    fn a_rescan_keeps_the_sender_of_unread_mail() {
        let mut d = Delivery::default();
        let from = Origin {
            host: "h".into(),
            pane: "w9:p1".into(),
            agent: "claude".into(),
        };
        d.mail_from("w1:p1", Sender::Other, Some(from.clone()));
        d.rescanned("w1:p1"); // the reconnect after an outage
        d.rescanned("w2:p1"); // mail from before this relay ran
        d.decide("w1:p1", 1, 0);
        d.gone("w1:p1");
        let b = d.take_bounces();
        assert_eq!(b.len(), 1);
        assert_eq!(b[0].senders(1), vec![from]);
        assert!(d.candidates(true).contains(&"w2:p1".to_string()));
    }

    #[test]
    fn read_mail_is_not_bounced() {
        let mut d = d("idle");
        d.mail_from("w1:p1", Sender::Other, origin("ganymede", "w5S:p1"));
        d.mail_from("w1:p1", Sender::Other, origin("proteus", "w3V:p2"));
        // The inbox read the older one: only proteus's mail is left.
        assert_eq!(d.decide("w1:p1", 1, 1), Decision::Type);
        d.gone("w1:p1");
        let b = d.take_bounces();
        assert_eq!(b[0].senders(9), vec![origin("proteus", "w3V:p2").unwrap()]);
        // All read: nothing to bounce.
        d.mail_from("w2:p1", Sender::Other, origin("ganymede", "w5S:p1"));
        assert_eq!(d.decide("w2:p1", 0, 1), Decision::Read);
        d.gone("w2:p1");
        assert!(d.take_bounces().is_empty());
    }

    #[test]
    fn senders_survive_an_upgrade() {
        let mut a = d("idle");
        a.mail_from("w1:p1", Sender::Other, origin("ganymede", "w5S:p1"));
        a.mail("w1:p1", Sender::Other);
        let mut b = Delivery::default();
        b.import(&a.export());
        b.gone("w1:p1");
        assert_eq!(
            b.take_bounces(),
            vec![Bounce {
                pane: "w1:p1".into(),
                origins: vec![origin("ganymede", "w5S:p1"), None],
                done: vec![],
            }]
        );
    }
}
