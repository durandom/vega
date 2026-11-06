#[path = "../../../crates/vega-herdr/tests/fixtures/own-terminal.rs"]
mod common;
#[test]
fn hc_answer_checks_claim_and_reads_exact_harness_session_log() {
    for (key, path) in [
        (
            "CLAUDE_CODE_SESSION_ID",
            ".claude/projects/-project/session-7.jsonl",
        ),
        (
            "CODEX_THREAD_ID",
            ".codex/sessions/2026/11/06/rollout-test-session-7.jsonl",
        ),
    ] {
        let (dir, mut c) = common::fixture(env!("CARGO_BIN_EXE_vega-uber"));
        c.env_remove("CODEX_THREAD_ID")
            .env(key, "session-7")
            .args(["answer", "missing"]);
        let o = c.output().unwrap();
        assert!(
            String::from_utf8_lossy(&o.stdout).contains("not_uber"),
            "{o:?}"
        );
        let claim = dir.path().join("state/vega/uber/claim.json");
        std::fs::create_dir_all(claim.parent().unwrap()).unwrap();
        std::fs::write(
            claim,
            r#"{"schema":1,"pane":"t7","host":"worker","at":1,"epoch":1}"#,
        )
        .unwrap();
        let o = c.output().unwrap();
        assert!(
            String::from_utf8_lossy(&o.stdout).contains("no_session_log"),
            "{o:?}"
        );
        let log = dir.path().join(path);
        std::fs::create_dir_all(log.parent().unwrap()).unwrap();
        std::fs::write(log, "{}\n").unwrap();
        let o = c.output().unwrap();
        assert!(
            String::from_utf8_lossy(&o.stdout).contains("no_ask"),
            "{o:?}"
        );
        let o = c.env("TEST_PARENT_PID", "4294967295").output().unwrap();
        assert!(
            String::from_utf8_lossy(&o.stdout).contains("not_in_terminal"),
            "{o:?}"
        );
    }
}
