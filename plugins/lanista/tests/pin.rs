use std::process::{Command, Output};

fn run(dir: &std::path::Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_vega-lanista"))
        .current_dir(dir)
        .env("VEGA_NATS_URL", "nats://127.0.0.1:1")
        .env("VEGA_NATS_NKEY", dir.join("missing-key"))
        .args(args)
        .output()
        .unwrap()
}

#[test]
fn project_reads_fall_back_visibly_and_mutations_fail_without_registry() {
    let dir = tempfile::tempdir().unwrap();
    assert!(Command::new("git")
        .args(["init", "-q"])
        .arg(dir.path())
        .status()
        .unwrap()
        .success());
    let out = run(dir.path(), &["pin", "codex", "--tier", "budget"]);
    assert!(out.status.success());
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.contains("model: gpt-6-luna\n"), "{text}");
    assert!(text.contains("source: global seats\n"), "{text}");
    assert!(text.contains("using global seats"), "{text}");
    assert!(text.contains("project:"), "{text}");
    for flags in [
        vec!["--set", "gpt-6.1-sol", "--until", "never"],
        vec!["--clear"],
    ] {
        let mut args = vec!["pin", "codex"];
        args.extend(flags);
        let out = run(dir.path(), &args);
        assert_eq!(out.status.code(), Some(1));
        let text = String::from_utf8(out.stdout).unwrap();
        assert!(text.contains("registry_write"), "{text}");
        assert!(!text.contains("action: set"), "{text}");
    }
}

#[test]
fn project_and_flag_validation_happen_before_registry_access() {
    let dir = tempfile::tempdir().unwrap();
    let out = run(dir.path(), &["pin", "codex"]);
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.contains("source: global seats"));
    assert!(!text.contains("registry:"));
    let out = run(dir.path(), &["pin", "codex", "--set", "test"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8(out.stdout)
        .unwrap()
        .contains("project_missing"));
    for flags in [
        vec!["--set", "test", "--clear"],
        vec!["--until", "never"],
        vec!["--typo"],
    ] {
        let mut args = vec!["pin", "codex"];
        args.extend(flags);
        assert_eq!(run(dir.path(), &args).status.code(), Some(2));
    }
}
