//! The built shim against a fake `tea` on PATH: stdout and exit codes pass
//! through unchanged, every call is counted, the third poll gets one hint.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, Output};

fn fake_tea(dir: &Path) {
    let bin = dir.join("tea");
    fs::write(
        &bin,
        "#!/bin/sh\nprintf 'out:%s\\n' \"$*\"\nexit \"${FAKE_TEA_EXIT:-0}\"\n",
    )
    .unwrap();
    fs::set_permissions(&bin, fs::Permissions::from_mode(0o755)).unwrap();
}

fn run(tmp: &Path, args: &[&str], exit: &str) -> Output {
    Command::new(env!("CARGO_BIN_EXE_tea"))
        .args(args)
        .env_clear()
        .env("PATH", tmp.join("bin"))
        .env("HOME", tmp)
        .env("XDG_STATE_HOME", tmp.join("state"))
        .env("XDG_CACHE_HOME", tmp.join("cache"))
        .env("HERDR_PANE_ID", "w1:p2")
        .env("FAKE_TEA_EXIT", exit)
        .output()
        .unwrap()
}

#[test]
fn passes_through_counts_and_hints_once() {
    let tmp = tempfile::tempdir().unwrap();
    let t = tmp.path();
    fs::create_dir(t.join("bin")).unwrap();
    fake_tea(&t.join("bin"));

    let out = run(t, &["issues", "list"], "3");
    assert_eq!(out.status.code(), Some(3));
    assert_eq!(String::from_utf8_lossy(&out.stdout), "out:issues list\n");
    assert!(out.stderr.is_empty());

    let stderr: Vec<String> = (0..4)
        .map(|_| {
            let out = run(t, &["pulls", "list"], "0");
            assert_eq!(out.status.code(), Some(0));
            assert_eq!(String::from_utf8_lossy(&out.stdout), "out:pulls list\n");
            String::from_utf8_lossy(&out.stderr).into_owned()
        })
        .collect();
    assert_eq!(stderr[0], "");
    assert_eq!(stderr[1], "");
    assert!(stderr[2].contains("vega ci watch"), "{}", stderr[2]);
    assert_eq!(stderr[3], "", "the hint shows once per session");

    let log = fs::read_to_string(t.join("state/vega/calls/tea/w1_p2")).unwrap();
    let cmds: Vec<&str> = log
        .lines()
        .map(|l| l.rsplit('\t').next().unwrap())
        .collect();
    assert_eq!(cmds, ["issues", "pulls", "pulls", "pulls", "pulls"]);
}

#[test]
fn a_missing_tea_says_how_to_get_it() {
    let tmp = tempfile::tempdir().unwrap();
    let t = tmp.path();
    fs::create_dir(t.join("bin")).unwrap();
    let out = run(t, &["pulls", "list"], "0");
    assert_eq!(out.status.code(), Some(127));
    assert!(out.stdout.is_empty());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("brew install tea"), "{err}");
}
