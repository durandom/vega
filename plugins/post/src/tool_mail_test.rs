//! Broker-mode tool mail must bypass inbox publishing on inbox-only hosts.
use std::os::unix::fs::PermissionsExt;
use std::process::Command;

use crate::{delivery::Delivery, Drafts};

#[test]
fn inbox_only_broker_tool_mail_is_local_and_holds_busy_panes_and_drafts() {
    const FIXTURE: &str = "VEGA_TOOL_MAIL_FIXTURE";
    if std::env::var_os(FIXTURE).is_none() {
        let dir = tempfile::tempdir().unwrap();
        let herdr = dir.path().join("herdr");
        std::fs::write(
            &herdr,
            r#"#!/bin/sh
case "$1 $2" in
  'pane read') printf '────────\n❯ \n────────\n' ;;
  'agent prompt') printf '%s\n' "$3 $4" >> "$VEGA_TOOL_MAIL_FIXTURE/prompts"; echo '{}' ;;
  *) exit 1 ;;
esac
"#,
        )
        .unwrap();
        std::fs::set_permissions(&herdr, std::fs::Permissions::from_mode(0o755)).unwrap();
        let output = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "tool_mail_test::inbox_only_broker_tool_mail_is_local_and_holds_busy_panes_and_drafts",
                "--nocapture",
            ])
            .env(FIXTURE, dir.path())
            .env("XDG_STATE_HOME", dir.path())
            .env("XDG_CONFIG_HOME", dir.path())
            .env("VEGA_POST_INBOX_ONLY", "deimos,puck")
            .env("PATH", format!("{}:{}", dir.path().display(), std::env::var("PATH").unwrap()))
            .env_remove("VEGA_MACHINE")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    // Deliberately allow publishing: a regression would ack and be visible
    // in the stream count, instead of hanging on a production ACL denial.
    let (_server, url) = crate::registry_read_test::start(dir.path(), "");
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            let client = async_nats::connect(url).await.unwrap();
            let js = async_nats::jetstream::new(client);
            let mut d = Delivery::default();
            d.status("w1:p1", "idle");
            d.status("w1:p2", "done");
            d.status("w1:p3", "working");
            d.status("invalid", "idle");
            let prompt_file =
                std::path::PathBuf::from(std::env::var_os(FIXTURE).unwrap()).join("prompts");
            for host in ["deimos", "puck"] {
                let mut stream = js.create_stream(crate::stream_config(host)).await.unwrap();
                let mut drafts = Drafts::default();
                let now = std::time::Instant::now();
                drafts
                    .seen
                    .insert("w1:p2".into(), ("draft".into(), now, now));
                let mut mail: Vec<_> = [
                    crate::hygiene::AGENT,
                    crate::ci::AGENT,
                    crate::bdsync::AGENT,
                    crate::upstream::AGENT,
                    vega_uber_msg::brief::DIGEST_AGENT,
                ]
                .into_iter()
                .map(|agent| (agent.to_string(), "w1:p1".into(), "first\nsecond".into()))
                .collect();
                mail.extend(["w1:p2", "w1:p3", "invalid"].map(|pane| {
                    (
                        crate::ci::AGENT.to_string(),
                        pane.to_string(),
                        "waiting".into(),
                    )
                }));
                std::fs::write(&prompt_file, "").unwrap();
                let left = crate::deliver_tool_mail(&js, host, mail, &d, &mut drafts).await;
                assert_eq!(
                    left.iter().map(|(_, p, _)| p.as_str()).collect::<Vec<_>>(),
                    ["w1:p2", "w1:p3"]
                );
                let typed = std::fs::read_to_string(&prompt_file).unwrap();
                assert_eq!(typed.lines().count(), 5);
                assert!(typed
                    .lines()
                    .all(|l| l.starts_with("w1:p1 [") && l.ends_with("first | second")));
                assert_eq!(
                    stream.info().await.unwrap().state.messages,
                    0,
                    "{host} must never publish tool mail"
                );

                // A later pass releases the held mail through the same local route.
                drafts.seen.clear();
                d.status("w1:p3", "done");
                let left = crate::deliver_tool_mail(&js, host, left, &d, &mut drafts).await;
                assert!(left.is_empty());
                assert_eq!(
                    std::fs::read_to_string(&prompt_file)
                        .unwrap()
                        .lines()
                        .count(),
                    7
                );
                assert_eq!(stream.info().await.unwrap().state.messages, 0);
                d.status("w1:p3", "working");
            }
            // Normal hosts retain broker mail even while the target is busy.
            let mut stream = js
                .create_stream(crate::stream_config("proteus"))
                .await
                .unwrap();
            std::fs::write(&prompt_file, "").unwrap();
            let left = crate::deliver_tool_mail(
                &js,
                "proteus",
                vec![(
                    crate::ci::AGENT.into(),
                    "w1:p3".into(),
                    "broker mail".into(),
                )],
                &d,
                &mut Drafts::default(),
            )
            .await;
            assert!(left.is_empty());
            assert_eq!(stream.info().await.unwrap().state.messages, 1);
            assert_eq!(std::fs::read_to_string(&prompt_file).unwrap(), "");
        });
}
