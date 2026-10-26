//! vegad's uber pass against a real broker with the production grants
//! (vega-lhc.209.5): the host key creates its own consumer, an answer from
//! the claim's host is applied once, a replay is refused, every message is
//! acked, and `uber.applied` reaches the Uber's inbox.

use std::os::unix::fs::PermissionsExt;

use async_nats::jetstream;
use futures_util::StreamExt;
use vega_registry::uber_asks;
use vega_uber_msg::Level;

use crate::auth::{self, Entry};
use crate::registry_read_test::{connect, start, user};
use crate::uber::tests::{answer, ask, ID};

/// bd that knows one open bead and logs every comment.
const FAKE_BD: &str = r#"#!/bin/sh
case "$1" in
  show) echo '[{"id":"vega-x.1","status":"open"}]' ;;
  comments) shift 2; echo "$@" >> "$(dirname "$0")/comments.log" ;;
  *) exit 1 ;;
esac
"#;

static NOTES: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

fn count_note(_: &str, _: &str, _: &str) {
    NOTES.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
}

#[test]
fn answers_are_verified_once_and_acknowledged_to_the_uber() {
    let dir = tempfile::tempdir().unwrap();
    let (admin_seed, admin) = user();
    let (proteus_seed, proteus) = user();
    let (ganymede_seed, ganymede) = user();
    let entries = vec![
        Entry {
            host: "proteus".into(),
            key: proteus,
        },
        Entry {
            host: "ganymede".into(),
            key: ganymede,
        },
    ];
    let authz = auth::render(
        &entries,
        Some(&admin),
        &["proteus".into(), "ganymede".into()],
        &[],
        None,
        &[],
        &[],
        &[],
        None,
    );
    let (_server, url) = start(dir.path(), &authz);

    let bd = dir.path().join("bd");
    std::fs::write(&bd, FAKE_BD).unwrap();
    std::fs::set_permissions(&bd, std::fs::Permissions::from_mode(0o755)).unwrap();
    let asks = dir.path().join("asks");
    let mut open = ask(Level::L4);
    open.bd = bd.display().to_string();
    open.cwd = dir.path().to_path_buf();
    uber_asks::save(&asks, &open).unwrap();
    let body = serde_json::to_string(&answer(Level::L4)).unwrap();

    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            // As `vega post provision` does.
            let a = jetstream::new(connect(&url, &admin_seed, crate::ADMIN_INBOX).await);
            for host in ["proteus", "ganymede"] {
                a.create_stream(jetstream::stream::Config {
                    max_bytes: 1 << 20,
                    ..crate::stream_config(host)
                })
                .await
                .unwrap();
            }

            // The Uber in w9:p1 on ganymede: plain mail, then the answer twice.
            let g = jetstream::new(connect(&url, &ganymede_seed, "_INBOX_ganymede").await);
            let subject = crate::inbox_subject("proteus", "w1:p1", "ganymede");
            let publish = |kind: Option<&'static str>, text: String| {
                let g = g.clone();
                let subject = subject.clone();
                async move {
                    let mut hs = async_nats::HeaderMap::new();
                    hs.insert(crate::h::FROM_PANE, "w9:p1");
                    if let Some(k) = kind {
                        hs.insert(crate::h::KIND, k);
                    }
                    g.publish_with_headers(subject, hs, text.into())
                        .await
                        .unwrap()
                        .await
                        .unwrap();
                }
            };
            publish(None, "plain".into()).await;
            publish(Some("uber.answer"), body.clone()).await;

            // vegad creates its consumer only now (first start, or this host
            // slept): the answer stored before is still verified.
            let p = jetstream::new(connect(&url, &proteus_seed, "_INBOX_proteus").await);
            let stream = crate::inbox_stream(&p, "proteus").await.unwrap();
            let consumer = crate::uber_consumer(&stream, "proteus").await.unwrap();

            // Without the ask store the answer is not verified, not dropped.
            let err = crate::uber_pass(&consumer, None).await;
            assert_eq!(err.map(|f| f.code), Some("no_state_dir".to_string()));

            let outbox = crate::uber::outbox(&asks);
            let err = crate::uber_pass(&consumer, Some(asks.clone())).await;
            assert!(err.is_none(), "{err:?}");
            let applied = format!(r#"{{"id":"{ID}","result":"applied"}}"#);
            let owed = crate::uber::unsent(&outbox);
            assert_eq!(owed.len(), 1, "{owed:?}");
            assert_eq!(owed[0].seq, 2);
            let (pane, text) = owed[0].mail.clone().unwrap();
            assert_eq!(pane, "w1:p1");
            assert!(text.contains("L4 approval of option"), "{text}");
            assert_eq!(
                owed[0].reply,
                Some(("ganymede".into(), "w9:p1".into(), applied.clone()))
            );
            let comments = || std::fs::read_to_string(dir.path().join("comments.log")).unwrap();
            assert!(
                comments().contains("--author Slayer -- yes"),
                "{}",
                comments()
            );
            assert!(comments().contains(&format!("via uber {ID}")));

            // A replay of the same answer: refused, nothing for the UAC.
            publish(Some("uber.answer"), body.clone()).await;
            let err = crate::uber_pass(&consumer, Some(asks.clone())).await;
            assert!(err.is_none(), "{err:?}");
            let owed = crate::uber::unsent(&outbox);
            assert_eq!(owed.len(), 2, "{owed:?}");
            assert_eq!(owed[1].mail, None);
            let refused = format!(r#"{{"id":"{ID}","result":"refused:answered"}}"#);
            assert_eq!(owed[1].reply.as_ref().unwrap().2, refused);

            let mut c = consumer.clone();
            let info = c.info().await.unwrap();
            assert_eq!((info.num_pending, info.num_ack_pending), (0, 0));

            // Redelivered after a restart before the ack (here: the consumer
            // made again): handled answers are not handled twice.
            stream
                .delete_consumer(&crate::uber::consumer("proteus"))
                .await
                .unwrap();
            let consumer = crate::uber_consumer(&stream, "proteus").await.unwrap();
            assert!(crate::uber_pass(&consumer, Some(asks.clone()))
                .await
                .is_none());
            assert_eq!(comments().matches("--author").count(), 1, "{}", comments());
            assert_eq!(crate::uber::unsent(&outbox).len(), 2);

            // The outbox is sent: mail to the UAC, uber.applied to the Uber.
            // An outbox that cannot be rewritten is not clear: vegad retries.
            let mode =
                |m: u32| std::fs::set_permissions(&outbox, std::fs::Permissions::from_mode(m));
            mode(0o555).unwrap();
            let clear = crate::uber_flush(&p, "proteus", &outbox, count_note).await;
            mode(0o755).unwrap();
            assert!(!clear);
            assert!(crate::uber_flush(&p, "proteus", &outbox, count_note).await);
            assert!(crate::uber::unsent(&outbox).is_empty());
            let notes = NOTES.load(std::sync::atomic::Ordering::SeqCst);
            // At least once: after the failed rewrite the note is written again.
            assert!(notes >= 1, "the replay's refusal note");
            let mine = stream.get_info().await.unwrap();
            assert_eq!(mine.state.messages, 4, "plain, two answers, the UAC mail");
            let theirs = crate::inbox_stream(&g, "ganymede").await.unwrap();
            let inbox = theirs
                .get_or_create_consumer(
                    &crate::consumer_name("ganymede", "w9:p1"),
                    crate::inbox_consumer("ganymede", "w9:p1"),
                )
                .await
                .unwrap();
            let mut got = inbox
                .fetch()
                .max_messages(1)
                .expires(std::time::Duration::from_secs(2))
                .messages()
                .await
                .unwrap();
            let m = got
                .next()
                .await
                .expect("uber.applied in the Uber's inbox")
                .unwrap();
            assert_eq!(m.subject.as_str(), "vega.inbox.ganymede.w9:p1.proteus");
            let kind = m.headers.as_ref().and_then(|hs| hs.get(crate::h::KIND));
            assert_eq!(kind.unwrap().as_str(), "uber.applied");
            assert_eq!(&m.payload[..], applied.as_bytes());
        });
}
