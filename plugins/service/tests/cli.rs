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

#[test]
fn registration_persists_canonical_project_and_cwd() {
    use std::os::unix::fs::PermissionsExt;
    let home = tempfile::tempdir().unwrap();
    let repo = home.path().join("checkout");
    std::fs::create_dir(&repo).unwrap();
    for args in [
        vec!["init", "-q"],
        vec![
            "remote",
            "add",
            "origin",
            "git@github.com:example/owner.git",
        ],
    ] {
        assert!(Command::new("git")
            .args(args)
            .current_dir(&repo)
            .status()
            .unwrap()
            .success());
    }
    let bin = home.path().join("bin");
    std::fs::create_dir(&bin).unwrap();
    for (name, body) in [
        ("ps", "case \"$*\" in *lstart*) echo 'S Tue Nov 10 12:00:00 2026';; *) echo '100 10 node node server.js';; esac"),
        ("ss", "echo 'LISTEN 0 128 127.0.0.1:5187 0.0.0.0:* users:((\"node\",pid=100,fd=4))'"),
        ("lsof", "exit 0"),
    ] {
        let file = bin.join(name);
        std::fs::write(&file, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(file, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let run = |cwd: &std::path::Path, args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_vega-service"))
            .args(args)
            .current_dir(cwd)
            .env("HOME", home.path())
            .env("XDG_CONFIG_HOME", home.path().join("config"))
            .env("XDG_STATE_HOME", home.path().join("state"))
            .env(
                "PATH",
                format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
            )
            .output()
            .unwrap()
    };
    let registered = run(
        &repo,
        &["--json", "register", "--port", "5187", "--title", "Fixture"],
    );
    assert!(
        registered.status.success(),
        "{}",
        String::from_utf8_lossy(&registered.stdout)
    );
    let data: serde_json::Value = serde_json::from_slice(&registered.stdout).unwrap();
    assert_eq!(data["service"]["project"], "github.com/example/owner");
    assert_eq!(
        data["service"]["cwd"],
        repo.canonicalize().unwrap().to_str().unwrap()
    );
    let listed = run(home.path(), &["ls", "--json"]);
    let listed: serde_json::Value = serde_json::from_slice(&listed.stdout).unwrap();
    assert_eq!(listed["services"][0], data["service"]);
    let global = run(
        home.path(),
        &["register", "--port", "5187", "--title", "Global", "--json"],
    );
    assert!(global.status.success());
    let global: serde_json::Value = serde_json::from_slice(&global.stdout).unwrap();
    assert_eq!(global["service"]["project"], "");
}
