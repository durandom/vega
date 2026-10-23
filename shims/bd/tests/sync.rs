//! Exercise the real shim boundary: clone selection, stopped guards, stderr
//! hints and unchanged real-bd stdout/exit status.
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, Output};
use vega_bd_sync::{store, Project, State};

fn git(dir: &Path, args: &[&str]) {
    assert!(Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .status()
        .unwrap()
        .success());
}

#[test]
fn guard_and_hints_keep_duplicate_clones_independent() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    std::env::set_var("XDG_STATE_HOME", root.join("state"));
    let real = root.join("real-bd");
    std::fs::write(&real, "#!/bin/sh\necho real-bd-output\nexit 17\n").unwrap();
    std::fs::set_permissions(&real, std::fs::Permissions::from_mode(0o755)).unwrap();
    for name in ["a", "b"] {
        let dir = root.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        git(&dir, &["init", "-q"]);
        git(
            &dir,
            &[
                "remote",
                "add",
                "origin",
                "https://github.com/team/repo.git",
            ],
        );
        let p = Project {
            key: "github.com/team/repo".into(),
            common: dir.join(".git"),
            dir: dir.clone(),
            database: "DB".into(),
            remote: "http://remote/DB".into(),
            bd: real.clone(),
            data: dir.join(".beads/dolt"),
            since: 1,
            resumed: 0,
        };
        assert!(store::save_project(&p));
        if name == "a" {
            assert!(store::save_state(
                &p.key,
                &p.common,
                &State {
                    stopped: Some(20),
                    stop_reason: Some("clone-a-conflict".into()),
                    ..State::default()
                }
            ));
            assert!(vega_bd_sync::git::save_state(
                &p.key,
                &p.common,
                &vega_bd_sync::git::GitState {
                    fetched: 10,
                    branch: Some("main".into()),
                    main_dir: Some(dir),
                    behind: Some(3),
                    skip: Some("dirty".into()),
                    ..Default::default()
                }
            ));
        }
    }
    let run = |cwd: &str, args: &[&str], manual: bool| -> Output {
        Command::new(env!("CARGO_BIN_EXE_bd"))
            .current_dir(root.join(cwd))
            .args(args)
            .env("BD_REAL", &real)
            .env("VEGA_POST_HOST", "fixture")
            .env("VEGA_BD_MANUAL", if manual { "1" } else { "0" })
            .output()
            .unwrap()
    };
    let outside = run(".", &["dolt", "pull"], false);
    assert_eq!(outside.status.code(), Some(17));
    assert_eq!(outside.stdout, b"real-bd-output\n");
    let gone = root.join("removed-cwd");
    std::fs::create_dir(&gone).unwrap();
    let unreadable = Command::new("sh")
        .args([
            "-c",
            "cd \"$1\" && rmdir \"$1\" && exec \"$2\" dolt pull",
            "fixture",
        ])
        .arg(&gone)
        .arg(env!("CARGO_BIN_EXE_bd"))
        .env("BD_REAL", &real)
        .env("VEGA_BD_MANUAL", "0")
        .output()
        .unwrap();
    assert_eq!(unreadable.status.code(), Some(17));
    assert_eq!(unreadable.stdout, b"real-bd-output\n");
    let stopped = run("b", &["-C", "../a", "dolt", "pull"], false);
    assert_eq!(stopped.status.code(), Some(1));
    assert!(stopped.stdout.is_empty());
    assert!(String::from_utf8_lossy(&stopped.stderr).contains("clone-a-conflict"));
    let healthy = run("b", &["dolt", "push"], false);
    assert!(healthy.status.success());
    assert!(healthy.stdout.is_empty());
    let hinted = run("a", &["list"], false);
    assert_eq!(hinted.status.code(), Some(17));
    assert_eq!(hinted.stdout, b"real-bd-output\n");
    let stderr = String::from_utf8_lossy(&hinted.stderr);
    assert!(stderr.contains("clone-a-conflict") && stderr.contains("3 behind origin/main"));
    let manual = run("b", &["dolt", "push"], true);
    assert_eq!(manual.status.code(), Some(17));
    // A URL change cannot silently remove a provisioned clone's guard.
    git(
        &root.join("a"),
        &[
            "remote",
            "set-url",
            "origin",
            "https://github.com/team/fork.git",
        ],
    );
    let changed = run("a", &["dolt", "push"], false);
    assert_eq!(changed.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&changed.stderr).contains("project_identity_changed"));
}
