//! Public CLI validation must happen before reading briefs or creating panes.
use std::process::Command;

#[test]
fn missing_tier_exits_two_with_models_before_reading_a_brief() {
    let dir = tempfile::tempdir().unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_vega-mux"))
        .current_dir(dir.path())
        .args([
            "spawn",
            "fix",
            "--kind",
            "codex",
            "--prompt-file",
            "/nonexistent/brief",
        ])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    let stdout = String::from_utf8(out.stdout).unwrap();
    for text in [
        "usage",
        "--tier flagship",
        "design/root-cause",
        "gpt-6-astra",
        "--tier balanced",
        "normal implementation",
        "gpt-6.1-sol",
        "--tier budget",
        "mechanical",
        "gpt-6-luna",
    ] {
        assert!(stdout.contains(text), "{stdout} lacks {text}");
    }
}

#[test]
fn tier_and_model_and_no_parent_reach_brief_reading() {
    let dir = tempfile::tempdir().unwrap();
    for extra in [
        vec!["--tier", "budget"],
        vec!["--no-parent"],
        vec!["--", "-m", "custom"],
        vec!["--", "--model=custom"],
    ] {
        let out = Command::new(env!("CARGO_BIN_EXE_vega-mux"))
            .current_dir(dir.path())
            .args([
                "spawn",
                "fix",
                "--kind",
                "codex",
                "--prompt-file",
                "/nonexistent/brief",
            ])
            .args(extra)
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(2));
        let stdout = String::from_utf8(out.stdout).unwrap();
        assert!(!stdout.contains("spawn requires --tier"), "{stdout}");
        assert!(stdout.contains("/nonexistent/brief"), "{stdout}");
    }
}
