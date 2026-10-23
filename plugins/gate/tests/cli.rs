//! `vega-gate` on fixture repos with fake bd, vega and herdr.

mod common;

use std::fs;
use std::path::Path;
use std::process::{Command, Output};

use common::{fakes, git, log, repo};

fn run(work: &Path, tmp: &Path, env: &[(&str, &str)], args: &[&str]) -> Output {
    common::git_env();
    let mut c = Command::new(env!("CARGO_BIN_EXE_vega-gate"));
    c.args(args)
        .current_dir(work)
        .env("HOME", tmp)
        .env("XDG_STATE_HOME", tmp.join("state"))
        .env("XDG_CACHE_HOME", tmp.join("cache"))
        .env("VEGA_TOKEN_BURN", "/nonexistent/token-burn")
        .env("FAKE_LOG", tmp)
        .env_remove("HERDR_PANE_ID")
        .env_remove("CLAUDE_CODE_SESSION_ID")
        .env_remove("CODEX_THREAD_ID");
    for (k, v) in fakes(tmp) {
        c.env(k, v);
    }
    for (k, v) in env {
        c.env(k, v);
    }
    c.output().unwrap()
}

fn out(o: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    )
}

/// A feature branch with one reviewed commit by claude.
fn feature(tmp: &Path) -> std::path::PathBuf {
    let work = repo(tmp);
    git(&work, &["checkout", "-q", "-b", "feat/login"]);
    common::write(&work, "src/a.rs", "fn a() { 1 }\n");
    git(&work, &["add", "-A"]);
    git(
        &work,
        &[
            "commit",
            "-q",
            "-m",
            "a1",
            "--trailer",
            "Assisted-by: claude:claude-opus-5-5",
        ],
    );
    work
}

#[test]
fn check_shows_the_missing_segment_and_writes_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let work = feature(tmp.path());
    let o = run(&work, tmp.path(), &[], &["check"]);
    let s = out(&o);
    assert!(o.status.success(), "{s}");
    assert!(s.contains("target origin/HEAD"), "{s}");
    assert!(s.contains("segments[1]"), "{s}");
    assert!(s.contains(",code,missing"), "{s}");
    assert!(s.contains("result: review needed (1 segments)"), "{s}");
    assert!(log(tmp.path(), "bd")
        .lines()
        .all(|l| l.starts_with("list ")));
    assert!(log(tmp.path(), "vega").is_empty());
}

#[test]
fn start_spawns_a_non_author_reviewer_in_a_clone() {
    let tmp = tempfile::tempdir().unwrap();
    let work = feature(tmp.path());
    let head = git(&work, &["rev-parse", "HEAD"]);
    let target = git(&work, &["rev-parse", "origin/main"]);
    // The author's local main differs from the push target. A shared clone's
    // origin/main points to that local branch, so it must not scope the review.
    git(&work, &["branch", "-f", "main", &head]);
    let o = run(&work, tmp.path(), &[("HERDR_PANE_ID", "w1:p1")], &["start"]);
    let s = out(&o);
    assert!(o.status.success(), "{s}");
    // claude wrote it (pane and trailer): the first candidate, codex, reviews.
    assert!(
        s.contains("result: review running in tab gate-feat-login (codex m-codex)"),
        "{s}"
    );
    let vega = log(tmp.path(), "vega");
    let spawn = vega.lines().find(|l| l.starts_with("mux spawn")).unwrap();
    assert!(
        spawn.contains("gate-feat-login --kind codex --at tab"),
        "{spawn}"
    );
    assert!(
        spawn.contains("--env VEGA_GATE_ROLE=reviewer --no-parent -- -m m-codex"),
        "{spawn}"
    );

    let bd = log(tmp.path(), "bd");
    let create = bd.lines().find(|l| l.starts_with("create")).unwrap();
    assert!(
        create.contains("--labels gate --external-ref gate:"),
        "{create}"
    );
    assert!(create.contains("\"verdict\":\"running\""), "{create}");
    assert!(create.contains("--status in_progress"), "{create}");
    assert!(
        bd.contains("update gate-1 --set-metadata reviewer_pane=w9:p5"),
        "{bd}"
    );

    let st: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(tmp.path().join("state/vega/gate/w9_p5.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(st["author_pane"], "w1:p1");
    assert_eq!(st["beads"][0], "gate-1");
    let clone = Path::new(st["clone"].as_str().unwrap());
    assert!(clone.starts_with(tmp.path().join("cache/vega/gate/clones")));
    assert_eq!(git(clone, &["rev-parse", "HEAD"]), head);
    // The rubric comes from the target, the prompt names the diff and the author.
    let work_dir = Path::new(st["work"].as_str().unwrap());
    assert_eq!(
        fs::read_to_string(work_dir.join("rubric/code/SKILL.md")).unwrap(),
        "# rubric v1\n"
    );
    let prompt = fs::read_to_string(work_dir.join("prompt.md")).unwrap();
    assert_eq!(git(clone, &["rev-parse", "origin/main"]), head);
    assert!(
        prompt.contains(&format!("target {target}, base {target}")),
        "{prompt}"
    );
    assert!(
        prompt.contains(&format!("git diff {target} {head} --")),
        "{prompt}"
    );
    assert!(
        prompt.contains(&format!("Rubric (from {target},")),
        "{prompt}"
    );
    assert!(!prompt.contains("origin/main"), "{prompt}");
    assert!(prompt.contains("':(glob)src/**'"), "{prompt}");
    assert!(prompt.contains("vega post send w1:p1"), "{prompt}");
    assert!(prompt.contains("vega gate verdict pass|fail"), "{prompt}");
}

#[test]
fn new_head_replaces_the_reviewer_and_refuses_its_unread_stale_verdict() {
    let tmp = tempfile::tempdir().unwrap();
    let work = feature(tmp.path());
    // Two initial segments must survive a later replacement of their reviewer.
    git(&work, &["checkout", "-q", "-b", "side"]);
    common::commit(&work, "src/b.rs", "fn b() {}\n", "side");
    git(&work, &["checkout", "-q", "feat/login"]);
    git(
        &work,
        &["merge", "-q", "--no-ff", "side", "-m", "merge side"],
    );
    let prior = git(&work, &["rev-parse", "HEAD"]);
    let env = [("HERDR_PANE_ID", "w1:p1")];
    let first = run(&work, tmp.path(), &env, &["start"]);
    assert!(first.status.success(), "{}", out(&first));
    fs::write(
        tmp.path().join("herdr-get.json"),
        r#"{"result":{"pane":{"agent":"codex","agent_status":"working"}}}"#,
    )
    .unwrap();
    let head = common::commit(&work, "src/a.rs", "fn a() { 2 }\n", "fix");
    // Another author pane taking over the same checkout still replaces the old review.
    let env = [("HERDR_PANE_ID", "w1:p2")];
    let second = run(&work, tmp.path(), &env, &["start"]);
    assert!(second.status.success(), "{}", out(&second));
    let vega = log(tmp.path(), "vega");
    assert_eq!(
        vega.lines().filter(|l| l.starts_with("mux spawn")).count(),
        2
    );
    assert!(!vega.contains("post send"), "{vega}");
    assert!(log(tmp.path(), "herdr").contains("pane close w9:p5"));
    let state_dir = tmp.path().join("state/vega/gate");
    assert!(!state_dir.join("w9_p5.json").exists());
    let replacement: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(state_dir.join("w9_p6.json")).unwrap()).unwrap();
    assert_eq!(replacement["head"], head);
    assert_eq!(replacement["author_pane"], "w1:p2");
    let generation = replacement["review"].as_str().unwrap();
    assert_eq!(
        Path::new(replacement["clone"].as_str().unwrap()),
        tmp.path().join("cache/vega/gate/clones").join(generation)
    );
    assert_eq!(
        Path::new(replacement["work"].as_str().unwrap()),
        tmp.path().join("cache/vega/gate").join(generation)
    );
    assert_ne!(generation, &replacement["keys"][0].as_str().unwrap()[..12]);
    let prompt =
        fs::read_to_string(Path::new(replacement["work"].as_str().unwrap()).join("prompt.md"))
            .unwrap();
    assert!(prompt.contains(&head), "{prompt}");
    assert_ne!(prior, head);
    // No mail acknowledgement: the original pane attempts both old verdicts.
    // Neither can touch the fresh generation's beads or count as a new-head pass.
    let before = log(tmp.path(), "bd");
    for verdict in ["pass", "fail"] {
        let stale = run(
            &work,
            tmp.path(),
            &[("HERDR_PANE_ID", "w9:p5"), ("CODEX_THREAD_ID", "s-rev")],
            &["verdict", verdict],
        );
        assert_eq!(stale.status.code(), Some(1), "{}", out(&stale));
        assert!(out(&stale).contains("runs no gate review"));
        assert_eq!(log(tmp.path(), "bd"), before);
    }
    // Return real per-key running records for the unchanged segments, rather
    // than the fake tool's default empty lookup. The new direct key has none.
    let previous: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(tmp.path().join("state/vega/gate/w9_p6.json")).unwrap(),
    )
    .unwrap();
    for (key, bead) in previous["keys"]
        .as_array()
        .unwrap()
        .iter()
        .zip(previous["beads"].as_array().unwrap())
    {
        let record = serde_json::json!([{
            "id": bead, "status": "in_progress",
            "metadata": {"verdict": "running", "started_at": previous["started_at"].as_i64().unwrap().to_string()}
        }]);
        fs::write(
            tmp.path()
                .join(format!("bd-key-gate:{}.json", key.as_str().unwrap())),
            record.to_string(),
        )
        .unwrap();
    }
    // A new bare codex trailer makes the running codex reviewer ineligible.
    common::commit(
        &work,
        "src/a.rs",
        "fn a() { 3 }\n",
        "fix\n\nAssisted-by: codex",
    );
    let third = run(&work, tmp.path(), &env, &["start"]);
    assert!(third.status.success(), "{}", out(&third));
    let vega = log(tmp.path(), "vega");
    let spawns: Vec<_> = vega
        .lines()
        .filter(|l| l.starts_with("mux spawn"))
        .collect();
    assert_eq!(spawns.len(), 3, "{vega}");
    assert!(spawns[2].contains("--kind pi"), "{vega}");
    assert!(log(tmp.path(), "herdr").contains("pane close w9:p5"));
    assert!(log(tmp.path(), "bd").contains("superseded by a fresh review"));
    let st: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(tmp.path().join("state/vega/gate/w9_p7.json")).unwrap(),
    )
    .unwrap();
    // The replacement gets the new direct segment plus both unchanged segments.
    assert_eq!(st["keys"].as_array().unwrap().len(), 3);
    assert_eq!(st["beads"].as_array().unwrap().len(), 3);
}

#[test]
fn verdict_authorized_just_before_supersession_is_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let work = feature(tmp.path());
    reviewer_state(tmp.path(), &work, "codex");
    fs::write(
        tmp.path().join("herdr-get.json"),
        r#"{"result":{"pane":{"agent_session":{"value":"s-rev"}}}}"#,
    )
    .unwrap();
    // authorize loads the old generation, then the pane lookup races with
    // start revoking it. give must recheck before any bead mutation or mail.
    fs::write(
        tmp.path().join("revoke-state"),
        tmp.path()
            .join("state/vega/gate/w9_p5.json")
            .to_str()
            .unwrap(),
    )
    .unwrap();
    let stale = run(
        &work,
        tmp.path(),
        &[("HERDR_PANE_ID", "w9:p5"), ("CODEX_THREAD_ID", "s-rev")],
        &["verdict", "pass"],
    );
    assert_eq!(stale.status.code(), Some(1), "{}", out(&stale));
    assert!(out(&stale).contains("review superseded or finished"));
    assert!(log(tmp.path(), "bd").is_empty());
    assert!(log(tmp.path(), "vega").is_empty());
}

#[test]
fn current_generation_pass_is_accepted() {
    let tmp = tempfile::tempdir().unwrap();
    let work = feature(tmp.path());
    let started = run(&work, tmp.path(), &[("HERDR_PANE_ID", "w1:p1")], &["start"]);
    assert!(started.status.success(), "{}", out(&started));
    fs::write(
        tmp.path().join("herdr-get.json"),
        r#"{"result":{"pane":{"agent_session":{"value":"s-rev"}}}}"#,
    )
    .unwrap();
    let verdict = run(
        &work,
        tmp.path(),
        &[("HERDR_PANE_ID", "w9:p5"), ("CODEX_THREAD_ID", "s-rev")],
        &["verdict", "pass"],
    );
    assert!(verdict.status.success(), "{}", out(&verdict));
    assert!(log(tmp.path(), "bd").contains("update gate-1 --set-metadata verdict=pass"));
}

#[test]
fn start_without_herdr_records_unreviewed_and_lets_it_through() {
    let tmp = tempfile::tempdir().unwrap();
    let work = feature(tmp.path());
    let o = run(&work, tmp.path(), &[], &["start"]);
    let s = out(&o);
    assert!(o.status.success(), "{s}");
    assert!(
        s.contains("not reviewed: no herdr, push not reviewed, recorded as gate-1"),
        "{s}"
    );
    let bd = log(tmp.path(), "bd");
    assert!(bd.contains("\"verdict\":\"unreviewed\""), "{bd}");
    assert!(bd.contains("close gate-1 --reason unreviewed: "), "{bd}");
    assert!(!log(tmp.path(), "vega").contains("mux spawn"));
}

#[test]
fn a_recorded_pass_needs_no_review() {
    let tmp = tempfile::tempdir().unwrap();
    let work = feature(tmp.path());
    fs::write(
        tmp.path().join("bd-list.json"),
        r#"[{"id":"gate-7","status":"closed","metadata":{"verdict":"pass"}}]"#,
    )
    .unwrap();
    let o = run(&work, tmp.path(), &[("HERDR_PANE_ID", "w1:p1")], &["start"]);
    let s = out(&o);
    assert!(s.contains("result: pass (every segment has one)"), "{s}");
    assert!(log(tmp.path(), "vega").is_empty());
    let o = run(&work, tmp.path(), &[], &["check"]);
    assert!(out(&o).contains("pass gate-7"));
}

/// A state file as `start` writes it, for pane w9:p5.
fn reviewer_state(tmp: &Path, work: &Path, kind: &str) {
    let clone = tmp.join("cache/vega/gate/clones/abc");
    fs::create_dir_all(&clone).unwrap();
    let st = serde_json::json!({
        "pane": "w9:p5", "repo": work, "branch": "feat/login", "target": "origin/HEAD",
        "head": "0123456789", "base": "abc", "keys": ["k"], "beads": ["gate-1"],
        "review": "r", "author_pane": "w1:p1", "clone": clone,
        "work": tmp.join("cache/vega/gate/abc"), "kind": kind, "model": "m-codex",
        "tab": "gate-feat-login", "started_at": 1, "timeout_secs": 1200
    });
    fs::create_dir_all(tmp.join("state/vega/gate")).unwrap();
    fs::write(tmp.join("state/vega/gate/w9_p5.json"), st.to_string()).unwrap();
}

#[test]
fn verdict_from_a_foreign_pane_is_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let work = feature(tmp.path());
    reviewer_state(tmp.path(), &work, "codex");
    fs::write(
        tmp.path().join("herdr-get.json"),
        r#"{"result":{"pane":{"agent_session":{"value":"s-rev"}}}}"#,
    )
    .unwrap();
    // The author's own pane has no review.
    let o = run(
        &work,
        tmp.path(),
        &[("HERDR_PANE_ID", "w1:p1"), ("CODEX_THREAD_ID", "s-author")],
        &["verdict", "pass"],
    );
    let s = out(&o);
    assert_eq!(o.status.code(), Some(1), "{s}");
    assert!(
        s.contains("only the reviewer pane w9:p5 gives the verdict"),
        "{s}"
    );
    // The reviewer's pane id with another session: forged.
    let o = run(
        &work,
        tmp.path(),
        &[("HERDR_PANE_ID", "w9:p5"), ("CODEX_THREAD_ID", "s-author")],
        &["verdict", "pass"],
    );
    let s = out(&o);
    assert_eq!(o.status.code(), Some(1), "{s}");
    assert!(
        s.contains("session s-author is not the pane's s-rev"),
        "{s}"
    );
    assert!(!log(tmp.path(), "bd").contains("close"));
}

#[test]
fn verdict_from_the_reviewer_closes_beads_and_mails_the_author() {
    let tmp = tempfile::tempdir().unwrap();
    let work = feature(tmp.path());
    reviewer_state(tmp.path(), &work, "codex");
    fs::write(
        tmp.path().join("herdr-get.json"),
        r#"{"result":{"pane":{"agent_session":{"value":"s-rev"}}}}"#,
    )
    .unwrap();
    let findings = tmp.path().join("f.jsonl");
    fs::write(
        &findings,
        r#"{"file":"src/a.rs","line":1,"rubric":"code","principle":"2","score":0,"fix":"add a test"}"#,
    )
    .unwrap();
    let env = [("HERDR_PANE_ID", "w9:p5"), ("CODEX_THREAD_ID", "s-rev")];
    let f = findings.to_str().unwrap();
    // pass with a 0 is refused before anything is recorded.
    let o = run(
        &work,
        tmp.path(),
        &env,
        &["verdict", "pass", "--findings", f],
    );
    assert_eq!(o.status.code(), Some(1), "{}", out(&o));
    assert!(out(&o).contains("pass with a 0 finding"));
    assert!(log(tmp.path(), "bd").is_empty());

    let o = run(
        &work,
        tmp.path(),
        &env,
        &["verdict", "fail", "--findings", f],
    );
    let s = out(&o);
    assert!(o.status.success(), "{s}");
    assert!(s.contains("verdict: fail"), "{s}");
    // `vega gate` drops the review at once, not when the pane has closed.
    assert!(!tmp.path().join("state/vega/gate/w9_p5.json").exists());
    let bd = log(tmp.path(), "bd");
    assert!(bd.contains("update gate-1 --set-metadata verdict=fail --append-notes src/a.rs:1 code/2 0: add a test"), "{bd}");
    assert!(
        bd.contains("close gate-1 --reason fail by codex/m-codex (1 findings), head 0123456"),
        "{bd}"
    );
    let vega = log(tmp.path(), "vega");
    assert!(
        vega.contains("post send --from-tool vega-gate w1:p1 gate fail for feat/login@0123456 (codex/m-codex), 1 findings\n- src/a.rs:1 code/2 0: add a test\nNext: fix the findings, commit, then push again."),
        "{vega}"
    );
}
