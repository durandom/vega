use std::fs;
use std::io::{Read, Seek, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

const SHIM: &str = env!("CARGO_BIN_EXE_git");

fn bounded_output(cmd: &mut Command) -> Output {
    let stdout = tempfile::tempfile().unwrap();
    let stderr = tempfile::tempfile().unwrap();
    let mut child = cmd
        .stdout(stdout.try_clone().unwrap())
        .stderr(stderr.try_clone().unwrap())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(15);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            child.wait().unwrap();
            panic!("git shim did not exit");
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let read = |mut file: fs::File| {
        file.rewind().unwrap();
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).unwrap();
        bytes
    };
    Output {
        status,
        stdout: read(stdout),
        stderr: read(stderr),
    }
}

fn git(path: impl AsRef<Path>) -> Command {
    let mut cmd = Command::new(path.as_ref());
    cmd.env_remove("VEGA_GIT_SHIM_ACTIVE")
        .env_remove("VEGA_SHIM_HOPS")
        .stdin(Stdio::null());
    cmd
}

#[test]
fn two_separate_shims_on_path_pass_through_to_real_git() {
    let tmp = tempfile::tempdir().unwrap();
    let a = tmp.path().join("a");
    let b = tmp.path().join("b");
    for dir in [&a, &b] {
        fs::create_dir(dir).unwrap();
        fs::copy(SHIM, dir.join("git")).unwrap();
    }
    // Make the copies byte-distinct as an installed and a debug build would be.
    fs::OpenOptions::new()
        .append(true)
        .open(b.join("git"))
        .unwrap()
        .write_all(b"another build")
        .unwrap();
    let real = Command::new("/usr/bin/git")
        .arg("--version")
        .output()
        .unwrap();
    assert!(real.status.success(), "{real:?}");
    let path = std::env::join_paths([a.as_path(), b.as_path(), Path::new("/usr/bin")]).unwrap();
    let out = bounded_output(git(a.join("git")).arg("--version").env("PATH", &path));
    assert_eq!(out.status, real.status, "{out:?}");
    assert_eq!(out.stdout, real.stdout);
    assert_eq!(out.stderr, real.stderr);

    // A legitimate git subprocess, such as a shell alias or hook, still works.
    let out = bounded_output(
        git(a.join("git"))
            .current_dir(tmp.path())
            .env("PATH", path)
            .args(["-c", "alias.nested=!git --version", "nested"]),
    );
    assert_eq!(out.status, real.status, "{out:?}");
    assert_eq!(out.stdout, real.stdout);
    assert_eq!(out.stderr, real.stderr);
}

#[test]
fn wrapper_loops_stop_at_the_marker_or_hop_limit() {
    let tmp = tempfile::tempdir().unwrap();
    let wrapper = tmp.path().join("git");
    for (body, reason) in [
        ("exec \"$TEST_SHIM\" \"$@\"", "re-entered the same shim"),
        (
            "unset VEGA_GIT_SHIM_ACTIVE\nexec \"$TEST_SHIM\" \"$@\"",
            "hop limit reached",
        ),
    ] {
        fs::write(&wrapper, format!("#!/bin/sh\n{body}\n")).unwrap();
        fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o755)).unwrap();
        let out = bounded_output(
            git(SHIM)
                .arg("--version")
                .env("PATH", tmp.path())
                .env("TEST_SHIM", SHIM),
        );
        assert_eq!(out.status.code(), Some(126), "{out:?}");
        assert!(out.stdout.is_empty());
        let stderr = String::from_utf8(out.stderr).unwrap();
        assert!(stderr.contains("recursive invocation blocked"), "{stderr}");
        assert!(stderr.contains(reason), "{stderr}");
    }
}
