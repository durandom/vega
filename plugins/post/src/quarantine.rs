//! Quarantine (vega-lhc.83): an inbox-only host (deimos, corp-managed) may
//! reply, but its mail never reaches an agent. Its key may publish only
//! `vega.quarantine.<to>.<pane>.<self>` (`auth-config --quarantine`), never
//! `vega.inbox.*`, so the broker enforces the split. The send is core NATS
//! request/reply, no stream: the target host's vegad serves
//! `vega.quarantine.<host>.>`, writes a note for the human (`vega post notes`,
//! the board's `N`, a herdr toast) and answers `stored`. Nothing is typed
//! into a pane and no agent inbox sees it. No vegad there: `no_responder`,
//! nothing is stored, the sender is told to try later.

use serde_json::{json, Value};
use vega_herdr::Failure;

/// Largest body a quarantined reply may carry.
pub const MAX_BODY: usize = 4096;
/// Note kind; the board and `vega post notes` show it.
pub const KIND: &str = "quarantine";

pub fn subject(to: &str, pane: &str, from: &str) -> String {
    format!("vega.quarantine.{to}.{pane}.{from}")
}

/// `vega.quarantine.<to>.<pane>.<from>` -> (to, pane, from). The from-host is
/// enforced by the sender's publish permission.
pub fn parse_subject(s: &str) -> Option<(&str, &str, &str)> {
    let mut t = s.strip_prefix("vega.quarantine.")?.split('.');
    match (t.next(), t.next(), t.next(), t.next()) {
        (Some(to), Some(pane), Some(from), None)
            if !to.is_empty() && !pane.is_empty() && !from.is_empty() =>
        {
            Some((to, pane, from))
        }
        _ => None,
    }
}

/// The note the target host writes: title and body for the human. The
/// sender's headers are claims; only the from-host is broker-enforced.
pub fn note(
    from_host: &str,
    to_pane: &str,
    from_agent: &str,
    from_pane: &str,
    text: &str,
) -> (String, String) {
    let first: String = text
        .lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("")
        .chars()
        .take(60)
        .collect();
    let title = format!("{from_host} (quarantine) to {to_pane}: {first}");
    let who = match (from_agent, from_pane) {
        ("", "") => from_host.to_string(),
        (a, "") => format!("{a} on {from_host}"),
        (a, p) => format!("{a} in {p} on {from_host}"),
    };
    let body = format!(
        "{text}\n\n(from {who}, an inbox-only host: for the human only; no agent was told, and agents never act on it)"
    );
    (title, body)
}

/// The vegad answer to one quarantined reply.
pub fn answer(ok: bool, code: &str, message: &str) -> Vec<u8> {
    json!({"ok": ok, "code": code, "message": message})
        .to_string()
        .into_bytes()
}

/// Checks a request before it becomes a note: an error answer, or None.
pub fn refuse(subject: &str, my_host: &str, payload: &[u8]) -> Option<Vec<u8>> {
    let Some((to, pane, _)) = parse_subject(subject) else {
        return Some(answer(false, "denied", "not a quarantine subject"));
    };
    if to != my_host {
        return Some(answer(false, "denied", "not for this host"));
    }
    if !crate::valid_pane_id(pane) {
        return Some(answer(false, "denied", "not a pane id"));
    }
    if payload.len() > MAX_BODY {
        return Some(answer(false, "denied", "larger than 4 KiB"));
    }
    if std::str::from_utf8(payload).is_err() {
        return Some(answer(false, "denied", "not UTF-8 text"));
    }
    None
}

/// Reads the vegad answer on the sender's side.
pub fn read_answer(payload: &[u8], to_host: &str) -> Result<(), Failure> {
    let v: Value = serde_json::from_slice(payload).unwrap_or_default();
    if v["ok"] == true {
        return Ok(());
    }
    Err(Failure::new(
        v["code"].as_str().unwrap_or("bad_reply"),
        format!(
            "{to_host} refused the quarantined reply: {}",
            v["message"].as_str().unwrap_or("")
        ),
        "fix the reply (pane id, at most 4 KiB of text) and send again",
    ))
}

/// The failure for a denied publish or subscribe, by the server's message.
/// A host the broker keeps out of agent inboxes hears that it is inbox-only
/// by design, never "check the broker config": that hint invited asking for
/// the send rights the Slayer refused.
pub fn denied(msg: &str, inbox_only_self: bool) -> Failure {
    let publish_to = |prefix: &str| {
        msg.contains("Publish to \"") && msg.contains(&format!("Publish to \"{prefix}"))
    };
    if publish_to("vega.quarantine.") {
        return Failure::new(
            "quarantine_denied",
            msg,
            "the broker has no quarantine grant for this host yet; the reply was not sent. Tell the user in your pane; do not ask for send rights",
        );
    }
    if publish_to("vega.inbox.") || (inbox_only_self && publish_to("vega.")) {
        return Failure::new(
            "inbox_only",
            format!("this host is inbox-only by design (receive only); it cannot mail agents ({msg})"),
            "`vega post send` and `done` put replies into quarantine, a notice for the human; this is not a broker misconfiguration, do not ask for send rights",
        );
    }
    Failure::new(
        "permission_denied",
        msg,
        "this machine's nkey lacks that right (mail as itself, commands as itself or as granted; see `vega post auth-config`); check VEGA_NATS_NKEY and the broker config",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subjects_round_trip() {
        let s = subject("proteus", "w3V:p1", "deimos");
        assert_eq!(s, "vega.quarantine.proteus.w3V:p1.deimos");
        assert_eq!(parse_subject(&s), Some(("proteus", "w3V:p1", "deimos")));
        assert_eq!(parse_subject("vega.quarantine.a.b"), None);
        assert_eq!(parse_subject("vega.quarantine.a.b.c.d"), None);
        assert_eq!(parse_subject("vega.inbox.a.b.c"), None);
    }

    #[test]
    fn denied_inbox_publish_says_inbox_only_without_broker_hint() {
        let msg = "nats: Permissions Violation for Publish to \"vega.inbox.proteus.w3V:p1.deimos\"";
        for me in [true, false] {
            let f = denied(msg, me);
            assert_eq!(f.code, "inbox_only");
            assert!(f.message.contains("inbox-only by design"));
            assert!(!f.hint.contains("broker config"));
            assert!(!f.hint.contains("VEGA_NATS_NKEY"));
        }
        let f = denied(
            "Permissions Violation for Publish to \"vega.cmd.proteus.open.deimos\"",
            true,
        );
        assert_eq!(f.code, "inbox_only");
        let f = denied(
            "Permissions Violation for Publish to \"vega.quarantine.proteus.w3V:p1.deimos\"",
            true,
        );
        assert_eq!(f.code, "quarantine_denied");
        assert!(!f.hint.contains("broker config"));
        // Other denials keep the old hint.
        let f = denied(
            "Permissions Violation for Subscription to \"_INBOX.>\"",
            false,
        );
        assert_eq!(f.code, "permission_denied");
        assert!(f.hint.contains("broker config"));
    }

    #[test]
    fn requests_are_checked_before_they_become_notes() {
        let ok = "vega.quarantine.proteus.w3V:p1.deimos";
        assert!(refuse(ok, "proteus", b"hi").is_none());
        assert!(refuse(ok, "ganymede", b"hi").is_some());
        assert!(refuse("vega.quarantine.proteus.nopane.deimos", "proteus", b"hi").is_some());
        assert!(refuse(ok, "proteus", &vec![b'x'; MAX_BODY + 1]).is_some());
        assert!(refuse(ok, "proteus", &[0xff, 0xfe]).is_some());
    }

    #[test]
    fn note_marks_the_quarantine() {
        let (t, b) = note("deimos", "w3V:p1", "claude", "w1:p2", "\nanswer: 42\nmore");
        assert_eq!(t, "deimos (quarantine) to w3V:p1: answer: 42");
        assert!(b.starts_with("\nanswer: 42\nmore"));
        assert!(b.contains("claude in w1:p2 on deimos"));
        assert!(b.contains("for the human only"));
        assert!(read_answer(&answer(true, "stored", "1"), "proteus").is_ok());
        assert_eq!(
            read_answer(&answer(false, "denied", "x"), "proteus")
                .unwrap_err()
                .code,
            "denied"
        );
    }
}
