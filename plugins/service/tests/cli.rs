use std::process::Command;

#[test]
fn embedded_backend_and_fast_paths_use_no_live_state() {
    let home = tempfile::tempdir().unwrap();
    let run = |args: &[&str], path: Option<&str>| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_vega-service"));
        command
            .args(args)
            .env("HOME", home.path())
            .env("XDG_STATE_HOME", home.path().join("state"));
        if let Some(path) = path {
            command.env("PATH", path);
        }
        command.output().unwrap()
    };
    for flag in ["--version", "-v", "-V", "--skill"] {
        let output = run(&[flag], Some(""));
        assert!(output.status.success(), "{flag}");
        assert!(!output.stdout.is_empty());
    }
    for args in [
        vec!["--help"],
        vec!["register", "--help"],
        vec!["ls", "--help"],
        vec!["rm", "--help"],
    ] {
        assert!(run(&args, None).status.success());
    }
    let empty = run(&["ls", "--json"], None);
    assert!(empty.status.success());
    let data: serde_json::Value = serde_json::from_slice(&empty.stdout).unwrap();
    assert_eq!(data["services"], serde_json::json!([]));
    let invalid = run(&["register", "--typo", "--json"], None);
    assert_eq!(invalid.status.code(), Some(2));
    let data: serde_json::Value = serde_json::from_slice(&invalid.stdout).unwrap();
    assert_eq!(data["error"]["code"], "usage");
    assert!(!home.path().join("state").exists());
}
