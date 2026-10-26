use std::net::TcpListener;
use std::path::Path;
use std::process::{Child, Command, Stdio};

use super::*;

#[test]
fn skill_has_valid_frontmatter() {
    vega_plugin::assert_guide(SKILL, "uber");
}

fn here(dir: &Path, host: &str, pane: &str) -> Here {
    Here {
        host: host.into(),
        pane: pane.into(),
        panes: vec![pane.into()],
        file: dir.join(host).join("claim.json"),
    }
}

fn claim_entry(view: &mut View, host: &str, pane: &str, epoch: u64, live: bool) {
    view.entries.insert(
        vega_registry::beat_key(host),
        json!({"at": now_ms() / 1000}),
    );
    view.entries.insert(
        vega_registry::uber_key(host),
        uber_state::value(pane, host, 0, epoch),
    );
    if live {
        view.entries.insert(
            vega_registry::agent_key(host, pane),
            json!({"pane": pane, "status": "idle"}),
        );
    }
}

#[test]
fn local_panes_and_claim_file_replace_the_registry_copy() {
    let dir = tempfile::tempdir().unwrap();
    let me = here(dir.path(), "proteus", "w2:p1");
    let mut view = View::default();
    // The registry still lists a claim of a pane gone here, and misses mine.
    claim_entry(&mut view, "proteus", "w9:p1", 50, true);
    with_local(&mut view, &me);
    assert!(view.uber_claims().is_empty());
    uber_state::save(&me.file, &uber_state::value("w9:p1", "proteus", 0, 50)).unwrap();
    with_local(&mut view, &me);
    let (winner, superseded, gone) = split(&view);
    assert_eq!((winner, superseded.len(), gone.len()), (None, 0, 1));
    uber_state::save(&me.file, &uber_state::value("w2:p1", "proteus", 0, 60)).unwrap();
    with_local(&mut view, &me);
    assert!(view.live_uber().is_some_and(|w| me.mine(&w)));
}

#[test]
fn a_returning_older_claim_is_superseded_not_the_uber() {
    let dir = tempfile::tempdir().unwrap();
    let me = here(dir.path(), "proteus", "w2:p1");
    let mut view = View::default();
    // proteus slept, ganymede took over with a newer epoch, proteus woke.
    uber_state::save(&me.file, &uber_state::value("w2:p1", "proteus", 0, 100)).unwrap();
    claim_entry(&mut view, "ganymede", "w1:p1", 200, true);
    claim_entry(&mut view, "titan", "w3:p1", 10, false);
    with_local(&mut view, &me);
    let (winner, superseded, gone) = split(&view);
    let winner = winner.unwrap();
    assert_eq!(winner.host, "ganymede");
    assert!(superseded.iter().any(|c| me.mine(c)));
    assert_eq!(gone[0].host, "titan");
    assert_eq!(conflict(&superseded[0], &winner).code, "uber_conflict");
}

struct Server(Child);

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// A local nats-server with JetStream and the registry bucket.
async fn broker(dir: &Path) -> (Server, async_nats::Client, async_nats::jetstream::kv::Store) {
    let port = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let conf = dir.join("nats.conf");
    std::fs::write(
        &conf,
        format!(
            "listen: 127.0.0.1:{port}\njetstream {{ store_dir: \"{}\" }}\n",
            dir.join("js").display()
        ),
    )
    .unwrap();
    let server = Server(
        Command::new("nats-server")
            .arg("-c")
            .arg(&conf)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("nats-server not on PATH: run the tests via `mise run check`"),
    );
    let url = format!("nats://127.0.0.1:{port}");
    let mut client = None;
    for _ in 0..50 {
        if let Ok(c) = async_nats::connect(&url).await {
            client = Some(c);
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let client = client.expect("nats-server did not start");
    let kv = async_nats::jetstream::new(client.clone())
        .create_key_value(async_nats::jetstream::kv::Config {
            bucket: vega_registry::BUCKET.into(),
            ..Default::default()
        })
        .await
        .unwrap();
    (server, client, kv)
}

/// What vegad publishes for a host: a fresh beat and its agent.
async fn host_up(kv: &async_nats::jetstream::kv::Store, h: &Here) {
    vega_registry::heartbeat(kv, &h.host).await.unwrap();
    kv.put(
        vega_registry::agent_key(&h.host, &h.pane),
        json!({"pane": h.pane, "status": "idle"}).to_string().into(),
    )
    .await
    .unwrap();
}

#[test]
fn two_hosts_racing_a_claim_converge_on_one_live_uber() {
    let dir = tempfile::tempdir().unwrap();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        let (_server, client, kv) = broker(dir.path()).await;
        let a = here(dir.path(), "ganymede", "w1:p1");
        let b = here(dir.path(), "proteus", "w2:p1");
        host_up(&kv, &a).await;
        host_up(&kv, &b).await;
        // Both read an empty registry before either writes: a true race.
        let (ra, rb) = tokio::join!(
            claim_on(&client, &kv, &a, 1_000),
            claim_on(&client, &kv, &b, 1_000)
        );
        let (ra, rb) = (ra.unwrap(), rb.unwrap());
        assert!(!ra.already && !rb.already);
        // Same claim time: the tie goes to the lower host name.
        let va = read_view(&client, &kv, &a).await.unwrap();
        let vb = read_view(&client, &kv, &b).await.unwrap();
        let (wa, sa, _) = split(&va);
        let (wb, sb, _) = split(&vb);
        assert_eq!(wa, wb, "both hosts see the same Uber");
        let winner = wa.unwrap();
        assert_eq!(winner.host, "ganymede");
        assert!(
            sb.iter().any(|c| b.mine(c)),
            "the loser sees it is superseded"
        );
        assert_eq!(sa, sb);
        // The loser's next claim refuses; the winner's is a no-op.
        assert_eq!(
            claim_on(&client, &kv, &b, 2_000).await.unwrap_err().code,
            "uber_exists"
        );
        assert!(claim_on(&client, &kv, &a, 2_000).await.unwrap().already);

        // Takeover: ganymede's pane is gone; proteus releases and claims anew.
        uber_state::remove(&b.file).unwrap();
        uber_state::unpublish(&kv, &b.host).await.unwrap();
        kv.delete(vega_registry::agent_key(&a.host, &a.pane))
            .await
            .unwrap();
        let c = claim_on(&client, &kv, &b, 500).await.unwrap();
        assert_eq!(c.took_over, vec![winner.clone()]);
        assert!(
            c.me.epoch > winner.epoch,
            "a takeover outranks the claim it replaced"
        );
        assert!(c.lost_to.is_none());
        // ganymede's pane comes back (same id): the older claim stays superseded.
        host_up(&kv, &a).await;
        let va = read_view(&client, &kv, &a).await.unwrap();
        assert_eq!(va.live_uber(), Some(c.me.clone()));
        assert!(split(&va).1.iter().any(|s| a.mine(s)));
    });
}

mod answer_path {
    use super::super::answer::answer_in;
    use serde_json::json;
    use std::cell::RefCell;
    use std::path::Path;
    use vega_herdr::Failure;
    use vega_uber_msg::received::{self, State};
    use vega_uber_msg::{audit, capture, shown, Ask, Level, Msg, Opt};

    const NOW: &str = "2026-10-26T13:00:00Z";

    fn receive(dir: &Path) -> received::Received {
        let id = vega_uber_msg::new_id("vega-lhc.209").unwrap();
        let ask = Ask {
            id: id.clone(),
            project: "github.com/durandom/vega".into(),
            bead: "vega-lhc.209".into(),
            level: Level::L4,
            urgent: None,
            question: "Push main?".into(),
            context: String::new(),
            options: vec![
                Opt {
                    label: "yes".into(),
                    detail: "push".into(),
                },
                Opt {
                    label: "no".into(),
                    detail: "wait".into(),
                },
            ],
            recommend: 0,
            why: String::new(),
            deadline: "no-rush".into(),
        };
        received::ingest(dir, &Msg::Ask(ask), "proteus", "w3V:p1", NOW).unwrap();
        received::load(dir, &id).unwrap()
    }

    fn pick(dir: &Path, r: &received::Received, label: &str) -> usize {
        let s = shown::render(&r.ask, &r.tag);
        let p = json!({
            "hook_event_name": "PostToolUse", "tool_name": "AskUserQuestion",
            "tool_input": {"questions": [{"header": s.header, "question": s.question, "options": s.options}]},
            "tool_response": {"answers": {s.question.clone(): label}},
        });
        let logged = [
            json!({"type": "assistant", "message": {"content": [
                {"type": "tool_use", "id": "toolu_x", "name": "AskUserQuestion", "input": p["tool_input"]}]}}),
            json!({"type": "user", "message": {"content": [{"type": "tool_result", "tool_use_id": "toolu_x"}]},
                "toolUseResult": {"questions": p["tool_input"]["questions"], "answers": p["tool_response"]["answers"]}}),
        ];
        LOG.with(|t| {
            for l in logged {
                t.borrow_mut().push_str(&format!("{l}\n"));
            }
        });
        capture::append(dir, &capture::from_hook(&p, NOW).unwrap()).unwrap()
    }

    thread_local! {
        static LOG: RefCell<String> = const { RefCell::new(String::new()) };
    }

    /// `answer_in` against the session log of this test's dialogs.
    fn answer_in_t(
        d: &Path,
        key: &str,
        n: Option<usize>,
        epoch: u64,
        at: &str,
        send: impl FnOnce(&received::Received, &str) -> Result<(), Failure>,
    ) -> Result<super::super::answer::Sent, Failure> {
        let t = LOG.with(|t| t.borrow().clone());
        answer_in(d, key, n, epoch, &t, at, send)
    }

    fn events(dir: &Path) -> Vec<String> {
        audit::last(dir, 50).into_iter().map(|e| e.event).collect()
    }

    #[test]
    fn without_a_capture_nothing_is_sent_and_the_hint_names_the_bound_ones() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        let r = receive(d);
        let never = |_: &received::Received, _: &str| -> Result<(), Failure> { panic!("sent") };
        let f = answer_in_t(d, &r.ask.id, None, 7, NOW, never)
            .err()
            .unwrap();
        assert_eq!(f.code, "no_capture");
        let n = pick(d, &r, "yes (Recommended)");
        let tag = r.tag.trim_start_matches('#').to_string();
        let f = answer_in_t(d, &tag, None, 7, NOW, never).err().unwrap();
        assert!(f.hint.contains(&format!(": {n};")), "{}", f.hint);
        assert_eq!(events(d), ["received", "rejected", "rejected"]);
    }

    #[test]
    fn an_answer_is_sent_once_and_audited() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        let r = receive(d);
        let n = pick(d, &r, "yes (Recommended)");
        let body = RefCell::new(String::new());
        let send = |to: &received::Received, b: &str| {
            assert_eq!(
                (to.from_host.as_str(), to.from_pane.as_str()),
                ("proteus", "w3V:p1")
            );
            *body.borrow_mut() = b.to_string();
            Ok(())
        };
        let s = answer_in_t(d, &r.tag, Some(n), 7, NOW, send).ok().unwrap();
        assert!(s.answer.approval);
        let m = vega_uber_msg::validate(vega_uber_msg::Kind::Answer, body.borrow().as_bytes());
        assert!(matches!(m, Ok(Msg::Answer(a)) if a.pick.as_deref() == Some("yes")));
        let rec = received::load(d, &r.ask.id).unwrap();
        assert_eq!((rec.state, rec.used.clone()), (State::Sent, vec![n]));
        let again = answer_in_t(d, &r.ask.id, Some(n), 7, NOW, |_, _| Ok(()))
            .err()
            .unwrap();
        assert_eq!(again.code, "not_open");
        assert_eq!(events(d), ["received", "answered", "sent", "rejected"]);
        let lines: Vec<String> = audit::last(d, 50).iter().map(|e| e.line()).collect();
        assert_eq!(
            lines[1],
            format!(
                "10-26 13:00Z answered {} vega-lhc.209: capture {n}: yes",
                r.tag
            )
        );
    }

    #[test]
    fn a_failed_send_leaves_the_ask_open() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        let r = receive(d);
        let n = pick(d, &r, "no");
        let fail =
            |_: &received::Received, _: &str| Err(Failure::new("send_failed", "down", "retry"));
        assert_eq!(
            answer_in_t(d, &r.ask.id, Some(n), 7, NOW, fail)
                .err()
                .unwrap()
                .code,
            "send_failed"
        );
        let rec = received::load(d, &r.ask.id).unwrap();
        assert_eq!((rec.state, rec.used.len()), (State::Open, 0));
        assert!(answer_in_t(d, &r.ask.id, Some(n), 7, NOW, |_, _| Ok(())).is_ok());
    }
}

#[test]
fn capture_is_not_a_vega_uber_command() {
    use clap::CommandFactory;
    // The Uber's model may run `vega uber ...` unprompted; it must not
    // reach the capture hook (docs/design/uber-uac.md §4).
    assert!(Cli::try_parse_from(["vega uber", "capture"]).is_err());
    let names: Vec<String> = Cli::command()
        .get_subcommands()
        .map(|c| c.get_name().to_string())
        .collect();
    assert!(!names.iter().any(|n| n.contains("capture")), "{names:?}");
    let manifest = include_str!("../Cargo.toml");
    assert!(manifest.contains("name = \"vega_uber_capture\""));
    assert!(!manifest.contains("name = \"vega-uber-capture\""));
}
