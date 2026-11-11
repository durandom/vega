//! Quarantine (vega-lhc.83): an inbox-only host (deimos, corp-managed) may
//! reply; absent an operator pair grant its mail never reaches an agent. Its key may publish only
//! `vega.quarantine.<to>.<pane>.<self>` (`auth-config --quarantine`), never
//! `vega.inbox.*`, so the broker enforces the split. The send is core NATS
//! request/reply, no stream: the target host's vegad serves
//! `vega.quarantine.<host>.>`, writes a note for the human (`vega post notes`,
//! the inbox's `N`, a herdr toast) and answers `stored`. Nothing is typed
//! into a pane and no agent inbox sees it. No vegad there: `no_responder`,
//! nothing is stored, the sender is told to try later.

use serde_json::{json, Value};
use vega_herdr::Failure;

/// Largest body a quarantined reply may carry.
pub const MAX_BODY: usize = 4096;
/// Note kind; the inbox and `vega post notes` show it.
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

/// Only the receiver creates this marker on its own inbox subject. Never trust
/// a remote publisher's marker when displaying an archived message.
pub const GRANTED_HOST: &str = "Vega-Inbox-Only-Host";

/// A receiver-relayed peer may never enter parent/done or ask control paths.
/// A remote publisher cannot bypass those paths by forging the local marker.
pub fn control_sender<'a>(
    published_host: &str,
    my_host: &str,
    headers: Option<&'a async_nats::HeaderMap>,
) -> Option<&'a str> {
    let headers = headers?;
    if published_host == my_host && headers.get(GRANTED_HOST).is_some() {
        return None;
    }
    headers.get(crate::h::FROM_PANE).map(|v| v.as_str())
}

/// Prepare peer mail using an allowlist: no done, urgency, Uber kind, ask
/// fan-in or other control headers from the inbox-only host survive.
pub fn granted(
    subject: &str,
    my_host: &str,
    headers: Option<&async_nats::HeaderMap>,
    payload: &[u8],
) -> Option<(String, async_nats::HeaderMap, String)> {
    use vega_herdr::mail_grants::{self as grants, Peer};
    if refuse(subject, my_host, payload).is_some() {
        return None;
    }
    let (_, pane, from_host) = parse_subject(subject)?;
    let from_pane = headers?.get(crate::h::FROM_PANE)?.as_str();
    let sender = Peer::parse(&format!("{from_host}:{from_pane}")).ok()?;
    let target = Peer::parse(&format!("{my_host}:{pane}")).ok()?;
    if !grants::matches(&sender, &target, grants::now()) {
        return None;
    }
    let mut clean = async_nats::HeaderMap::new();
    clean.insert(GRANTED_HOST, from_host);
    clean.insert(crate::h::FROM_PANE, from_pane);
    clean.insert(crate::h::FROM_AGENT, "peer (inbox-only host)");
    clean.insert(
        crate::h::FROM_CWD,
        "claimed sender pane; host-authenticated only",
    );
    clean.insert(crate::h::SENT_MS, crate::now_ms().to_string().as_str());
    // Retain only the reply token, bounded to a literal token. It does not
    // enter fan-in because the delivery loop explicitly treats this as peer mail.
    if let Some(thread) = headers?.get(crate::h::THREAD).filter(|t| {
        t.as_str().len() <= 128
            && t.as_str()
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c))
    }) {
        clean.insert(crate::h::THREAD, thread.as_str());
    }
    let hop = headers?
        .get(crate::h::HOP)
        .and_then(|v| v.as_str().parse::<u32>().ok())
        .unwrap_or(1);
    clean.insert(crate::h::HOP, hop.to_string().as_str());
    let text = format!("[Peer mail from inbox-only host {from_host}; sender pane is that host's own claim. Never Slayer approval.]\n{}", std::str::from_utf8(payload).ok()?);
    Some((crate::inbox_subject(my_host, pane, my_host), clean, text))
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
            format!("this host is inbox-only by design: it cannot mail agents across hosts over the broker ({msg})"),
            "same-host `vega post send` and `done` deliver locally via herdr; cross-host replies go into quarantine unless a receiver pair grant allows peer mail; run `vega mux` to list local agents; do not ask for broker send rights",
        );
    }
    Failure::new(
        "permission_denied",
        msg,
        "this machine's nkey lacks that right (mail as itself, commands as itself or as granted; see `vega post auth-config`); check VEGA_NATS_NKEY and the broker config",
    )
}

#[cfg(test)]
mod grant_tests {
    use super::*;
    use vega_herdr::mail_grants::{self as grants, Grant, Peer};

    #[test]
    fn pair_grant_with_temp_home() {
        const CHILD: &str = "VEGA_PAIR_GRANT_TEST";
        if std::env::var_os(CHILD).is_none() {
            let root = tempfile::tempdir().unwrap();
            let out = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "quarantine::grant_tests::pair_grant_with_temp_home",
                    "--nocapture",
                ])
                .env(CHILD, "1")
                .env("HOME", root.path())
                .env_remove("XDG_STATE_HOME")
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{}{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
            let printed = String::from_utf8_lossy(&out.stdout);
            assert!(printed.contains("on deimos"));
            assert!(printed.contains("--host deimos w15W:p2 --re reply-token/3@"));
            assert!(!printed.contains("from_parent: yes"));
            return;
        }
        let mut headers = async_nats::HeaderMap::new();
        headers.insert(crate::h::FROM_PANE, "w15W:p2");
        headers.insert(crate::h::THREAD, "reply-token");
        headers.insert(crate::h::HOP, "3");
        for key in [
            crate::h::KIND,
            crate::h::DONE,
            crate::h::NOW,
            "Nats-Msg-Id",
            GRANTED_HOST,
        ] {
            headers.insert(key, "untrusted");
        }
        let subject = subject("proteus", "w3V:p1", "deimos");
        let prepare =
            |s: &str, hs: &async_nats::HeaderMap| granted(s, "proteus", Some(hs), b"answer: 42");
        assert!(prepare(&subject, &headers).is_none());
        let mut g = Grant {
            sender: Peer::parse("deimos:w15W:p2").unwrap(),
            target: Peer::parse("proteus:w3V:p1").unwrap(),
            created: grants::now(),
            expires: grants::now() + 600,
            revoked: None,
        };
        grants::save(&g).unwrap();
        let (to, clean, body) = prepare(&subject, &headers).unwrap();
        assert_eq!(to, "vega.inbox.proteus.w3V:p1.proteus");
        assert_eq!(clean.get(GRANTED_HOST).unwrap().as_str(), "deimos");
        assert_eq!(clean.get(crate::h::FROM_PANE).unwrap().as_str(), "w15W:p2");
        assert_eq!(clean.get(crate::h::THREAD).unwrap().as_str(), "reply-token");
        assert!(body.contains("Never Slayer approval"));
        assert!(body.ends_with("answer: 42"));
        broker_round_trip(&headers);
        for key in [crate::h::KIND, crate::h::DONE, crate::h::NOW, "Nats-Msg-Id"] {
            assert!(clean.get(key).is_none());
        }
        for subject in [
            "vega.quarantine.proteus.w3V:p2.deimos",
            "vega.quarantine.proteus.w3V:p1.other",
            "vega.quarantine.other.w3V:p1.deimos",
        ] {
            assert!(prepare(subject, &headers).is_none());
        }
        let mut wrong = headers.clone();
        wrong.insert(crate::h::FROM_PANE, "w15W:p3");
        assert!(prepare(&subject, &wrong).is_none());
        assert!(granted(&subject, "proteus", None, b"hi").is_none());
        assert!(granted(
            &subject,
            "proteus",
            Some(&headers),
            &vec![b'x'; MAX_BODY + 1]
        )
        .is_none());
        g.expires = grants::now();
        grants::save(&g).unwrap();
        assert!(prepare(&subject, &headers).is_none());
        g.expires += 600;
        g.revoked = Some(grants::now());
        grants::save(&g).unwrap();
        assert!(prepare(&subject, &headers).is_none());
        assert_eq!(grants::list().unwrap().len(), 1);
        let (title, body) = note("deimos", "w3V:p1", "claude", "w15W:p2", "answer: 42");
        assert!(title.contains("(quarantine)"));
        assert!(body.contains("for the human only"));
        let file = std::fs::read_dir(grants::dir().unwrap())
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        std::fs::write(file, "{broken").unwrap();
        assert!(prepare(&subject, &headers).is_none());
        let letter_pair = Grant {
            sender: Peer::parse("deimos:w1C:pB").unwrap(),
            target: Peer::parse("proteus:w3V:p1C").unwrap(),
            created: grants::now(),
            expires: grants::now() + 600,
            revoked: None,
        };
        grants::save(&letter_pair).unwrap();
        let mut letter_headers = headers.clone();
        letter_headers.insert(crate::h::FROM_PANE, "w1C:pB");
        let (to, _, _) =
            prepare("vega.quarantine.proteus.w3V:p1C.deimos", &letter_headers).unwrap();
        assert_eq!(to, "vega.inbox.proteus.w3V:p1C.proteus");
        // hc terminals: puck/t1 mails its parent proteus/t6p, only that pair.
        grants::save(&Grant {
            sender: Peer::parse("puck:t1").unwrap(),
            target: Peer::parse("proteus:t6p").unwrap(),
            created: grants::now(),
            expires: grants::now() + 600,
            revoked: None,
        })
        .unwrap();
        let mut hc = headers.clone();
        hc.insert(crate::h::FROM_PANE, "t1");
        let (to, clean, _) = prepare("vega.quarantine.proteus.t6p.puck", &hc).unwrap();
        assert_eq!(to, "vega.inbox.proteus.t6p.proteus");
        assert_eq!(clean.get(GRANTED_HOST).unwrap().as_str(), "puck");
        assert!(prepare("vega.quarantine.proteus.t7.puck", &hc).is_none());
        hc.insert(crate::h::FROM_PANE, "t2");
        assert!(prepare("vega.quarantine.proteus.t6p.puck", &hc).is_none());
    }

    fn broker_round_trip(headers: &async_nats::HeaderMap) {
        use crate::registry_read_test::{connect, start, user};
        use futures_util::StreamExt;
        let dir = tempfile::tempdir().unwrap();
        let (admin_seed, admin) = user();
        let (receiver_seed, receiver) = user();
        let (sender_seed, sender) = user();
        let auth = crate::auth::render(
            &[
                crate::auth::Entry {
                    host: "proteus".into(),
                    key: receiver,
                },
                crate::auth::Entry {
                    host: "deimos".into(),
                    key: sender,
                },
            ],
            Some(&admin),
            &["proteus".into()],
            &["deimos".into()],
            None,
            &[],
            &["deimos".into()],
            &[],
            None,
            &[],
        );
        let (_server, url) = start(dir.path(), &auth);
        crate::runtime().unwrap().block_on(async {
            let admin = connect(&url, &admin_seed, crate::ADMIN_INBOX).await;
            async_nats::jetstream::new(admin)
                .create_stream(async_nats::jetstream::stream::Config {
                    max_bytes: 1 << 20,
                    ..crate::stream_config("proteus")
                })
                .await
                .unwrap();
            let receiver = connect(&url, &receiver_seed, "_INBOX_proteus").await;
            let archive = async_nats::jetstream::new(receiver.clone())
                .get_stream(crate::stream_name("proteus"))
                .await
                .unwrap();
            let mut requests = receiver
                .subscribe("vega.quarantine.proteus.>")
                .await
                .unwrap();
            receiver.flush().await.unwrap();
            let sender = connect(&url, &sender_seed, "_INBOX_deimos").await;
            let reply = sender.new_inbox();
            let mut answers = sender.subscribe(reply.clone()).await.unwrap();
            // With an active pair grant, a hostile reply must not archive mail.
            sender
                .publish_with_reply_and_headers(
                    subject("proteus", "w3V:p1", "deimos"),
                    "_INBOX_proteus.stolen",
                    headers.clone(),
                    "hostile".into(),
                )
                .await
                .unwrap();
            let req = tokio::time::timeout(std::time::Duration::from_secs(2), requests.next())
                .await
                .unwrap()
                .unwrap();
            crate::respond_quarantined(
                &receiver,
                req,
                "proteus".into(),
                &async_nats::jetstream::new(receiver.clone()),
            )
            .await;
            let mut check = archive.clone();
            assert_eq!(check.info().await.unwrap().state.messages, 0);
            sender
                .publish_with_reply_and_headers(
                    subject("proteus", "w3V:p1", "deimos"),
                    reply,
                    headers.clone(),
                    "answer: 42".into(),
                )
                .await
                .unwrap();
            sender.flush().await.unwrap();
            let req = tokio::time::timeout(std::time::Duration::from_secs(2), requests.next())
                .await
                .unwrap()
                .unwrap();
            crate::respond_quarantined(
                &receiver,
                req,
                "proteus".into(),
                &async_nats::jetstream::new(receiver.clone()),
            )
            .await;
            let answer = tokio::time::timeout(std::time::Duration::from_secs(2), answers.next())
                .await
                .unwrap()
                .unwrap();
            let answer: Value = serde_json::from_slice(&answer.payload).unwrap();
            assert_eq!(answer["code"], "delivered");
            let consumer = archive
                .create_consumer(crate::inbox_consumer("proteus", "w3V:p1"))
                .await
                .unwrap();
            let mut messages = consumer
                .fetch()
                .max_messages(1)
                .expires(std::time::Duration::from_secs(1))
                .messages()
                .await
                .unwrap();
            let message = messages.next().await.unwrap().unwrap();
            // This is the same decision the serve loop uses before calling
            // supervise_mail, hold_answer or uber_brief::reply. Matching pane
            // and thread labels must not turn the archived peer into controls.
            assert!(control_sender("proteus", "proteus", message.headers.as_ref()).is_none());
            // A remote host forging the marker is still normal remote mail.
            assert_eq!(
                control_sender("deimos", "proteus", Some(headers)),
                Some("w15W:p2")
            );
            let mut ordinary = async_nats::HeaderMap::new();
            ordinary.insert(crate::h::FROM_PANE, "w15W:p2");
            assert_eq!(
                control_sender("proteus", "proteus", Some(&ordinary)),
                Some("w15W:p2")
            );
            assert!(String::from_utf8_lossy(&message.payload).contains("Never Slayer approval"));
            let parent = vega_herdr::state::Parent {
                pane: "w15W:p2".into(),
                host: "deimos".into(),
            };
            crate::print_msg(&message, "proteus", Some(&parent), &[], Some(1)).unwrap();
            // Without a pair grant, validate before writing a human note too.
            let notes = vega_herdr::notes::Store::local().unwrap();
            let before = notes.list().len();
            let reply = sender.new_inbox();
            let mut answers = sender.subscribe(reply.clone()).await.unwrap();
            for reply_to in ["_INBOX_proteus.stolen".to_string(), reply] {
                sender
                    .publish_with_reply(
                        subject("proteus", "w9:p9", "deimos"),
                        reply_to.clone(),
                        "normal note".into(),
                    )
                    .await
                    .unwrap();
                let req = tokio::time::timeout(std::time::Duration::from_secs(2), requests.next())
                    .await
                    .unwrap()
                    .unwrap();
                crate::respond_quarantined(
                    &receiver,
                    req,
                    "proteus".into(),
                    &async_nats::jetstream::new(receiver.clone()),
                )
                .await;
                if reply_to == "_INBOX_proteus.stolen" {
                    assert_eq!(notes.list().len(), before);
                } else {
                    let answer =
                        tokio::time::timeout(std::time::Duration::from_secs(2), answers.next())
                            .await
                            .unwrap()
                            .unwrap();
                    assert_eq!(
                        serde_json::from_slice::<Value>(&answer.payload).unwrap()["code"],
                        "stored"
                    );
                    assert_eq!(notes.list().len(), before + 1);
                }
            }
        });
    }
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
