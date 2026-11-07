use super::*;

#[test]
fn quiet_attention_uses_durable_evidence_and_item_identity() {
    const CHILD: &str = "VEGA_QUIET_ATTENTION_TEST";
    if env::var_os(CHILD).is_none() {
        let dir = tempfile::tempdir().unwrap();
        let out = std::process::Command::new(env::current_exe().unwrap())
            .args([
                "--exact",
                "quiet_attention_test::quiet_attention_uses_durable_evidence_and_item_identity",
                "--nocapture",
            ])
            .env_clear()
            .env("HOME", dir.path())
            .env("XDG_STATE_HOME", dir.path())
            .env("PATH", "/usr/bin:/bin")
            .env("VEGA_POST_HOST", "fixture")
            .env(CHILD, "1")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        return;
    }
    let mut sup = Supervisor::default();
    let mut d = Delivery::default();
    for pane in ["t2", "w1:p2"] {
        st::save_parent_on(None, pane, "child", "t1");
        st::save_mode(None, pane, "child", st::Mode::Headless);
        st::save_silence(None, pane, &st::Silence::default());
        d.status(pane, "idle");
        sup.observe(pane, "idle", false);
    }
    d.status("t1", "idle");
    sup.snapshot([&"w1:p2".to_string()]);
    sup.snapshot_backend([&"t2".to_string(), &"t1".to_string()], true);
    let initial = due_wakes(&mut sup, &d, None).remove("t1").unwrap().1;
    assert_eq!(initial.len(), 2);
    sup.woke("t1", &initial);
    assert!(due_wakes(&mut sup, &d, None).is_empty());
    for pane in ["t2", "w1:p2"] {
        for outcome in [st::Outcome::Succeeded, st::Outcome::Failed] {
            st::save_done(
                None,
                pane,
                &st::Done {
                    outcome,
                    summary: "reported".into(),
                },
            );
            let set = attentions(&sup, &d, None).remove("t1").unwrap().0;
            assert!(!set.iter().any(|(p, _)| p == pane), "reported {pane}");
        }
        st::settle(None, pane);
        st::save_silence(None, pane, &st::Silence::default());
        let target = vega_ci_watch::Target::pr("github.com", "team/repo", 1);
        assert!(vega_ci_watch::store::add_watch(
            &vega_ci_watch::store::Watch {
                target: target.clone(),
                pane: pane.into(),
                since: 1
            }
        ));
        assert!(!attentions(&sup, &d, None)["t1"]
            .0
            .iter()
            .any(|(p, _)| p == pane));
        vega_ci_watch::store::remove_watch(&target, pane);
        let review = vega_gate::state::Review::from_json(&serde_json::json!({
            "pane":"t99", "author_pane":pane, "repo":"/r", "clone":"/c", "started_at":vega_ci_watch::unix_now(), "timeout_secs":60
        })).unwrap();
        review.save().unwrap();
        assert!(!attentions(&sup, &d, None)["t1"]
            .0
            .iter()
            .any(|(p, _)| p == pane));
        let mut expired = review.clone();
        expired.started_at -= 120;
        expired.save().unwrap();
        assert!(attentions(&sup, &d, None)["t1"]
            .0
            .iter()
            .any(|(p, _)| p == pane));
        vega_gate::state::remove("t99");
        st::save_park(
            None,
            pane,
            &st::Park {
                bead: "decision".into(),
                title: "hold".into(),
                dir: "/r".into(),
                bd: "bd".into(),
                closed: false,
            },
        );
        assert!(!attentions(&sup, &d, None)["t1"]
            .0
            .iter()
            .any(|(p, _)| p == pane));
        st::unpark(None, pane);
    }
    // kill/rm is proven absence in hc; native notes own those exits.
    sup.gone("t2");
    let set = attentions(&sup, &d, None).remove("t1").unwrap().0;
    assert_eq!(set.len(), 1);
    assert_eq!(set.iter().next().unwrap().0, "w1:p2");
    // An equally-sized set with a different item is a new actionable wake.
    sup.woke("t1", &set);
    st::save_done(
        None,
        "w1:p2",
        &st::Done {
            outcome: st::Outcome::Succeeded,
            summary: "done".into(),
        },
    );
    sup.observe("t2", "idle", false);
    let changed = due_wakes(&mut sup, &d, None).remove("t1").unwrap().1;
    assert_eq!(changed.len(), 1);
    assert_eq!(changed.iter().next().unwrap().0, "t2");
    sup.woke("t1", &changed);
    assert!(due_wakes(&mut sup, &d, None).is_empty());
    assert!(wake_line("1 sentinel needs you", 0).contains("≤10 words"));
    assert!(wake_line("1 sentinel needs you", 2).contains("≤10 words"));
}
