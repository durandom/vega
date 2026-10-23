use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::os::unix::fs::{symlink, PermissionsExt};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

// The original failure was an infinite loop: regression tests need their own
// deadline even when the shim's guard or helper timeout is broken.
trait BoundedOutput {
    fn bounded_output(&mut self) -> std::io::Result<Output>;
}

impl BoundedOutput for Command {
    fn bounded_output(&mut self) -> std::io::Result<Output> {
        let mut out = tempfile::tempfile()?;
        let mut err = tempfile::tempfile()?;
        let mut child = self
            .stdin(Stdio::null())
            .stdout(out.try_clone()?)
            .stderr(err.try_clone()?)
            .process_group(0)
            .spawn()?;
        let start = Instant::now();
        let status = loop {
            if let Some(status) = child.try_wait()? {
                break status;
            }
            if start.elapsed() > Duration::from_secs(5) {
                // SAFETY: this child leads its own process group.
                unsafe { libc::kill(-(child.id() as i32), libc::SIGKILL) };
                let _ = child.kill();
                let _ = child.wait();
                return Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "bd shim hung",
                ));
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        out.seek(SeekFrom::Start(0))?;
        err.seek(SeekFrom::Start(0))?;
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        out.read_to_end(&mut stdout)?;
        err.read_to_end(&mut stderr)?;
        Ok(Output {
            status,
            stdout,
            stderr,
        })
    }
}

fn script(path: &Path, body: &str) {
    fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

struct Fixture {
    _tmp: tempfile::TempDir,
    repo: PathBuf,
    shim: PathBuf,
    real: PathBuf,
    path: std::ffi::OsString,
}

impl Fixture {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("embedded");
        fs::create_dir_all(repo.join(".beads")).unwrap();
        fs::write(
            repo.join(".beads/metadata.json"),
            r#"{"dolt_mode":"embedded"}"#,
        )
        .unwrap();
        let bin = tmp.path().join("shims");
        let alias = tmp.path().join("alias");
        let originals = tmp.path().join("originals");
        fs::create_dir(&bin).unwrap();
        fs::create_dir(&originals).unwrap();
        symlink(&bin, &alias).unwrap();
        let shim = bin.join("bd");
        symlink(env!("CARGO_BIN_EXE_bd"), &shim).unwrap();
        let real = originals.join("bd");
        script(
            &real,
            "printf '%s' \"$PATH\" > \"$BD_TEST_CHILD_PATH\"; printf 'real:%s:%s\\n' \"$*\" \"$BEADS_ACTOR\"; exit 23",
        );
        // If helpers see the shim directory, this git emulates discovery that
        // calls bd again. With PATH cleaned, the trap must never run.
        script(
            &bin.join("git"),
            "echo invoked > \"$BD_TEST_HELPER_TRAP\"; bd where --json; exit 1",
        );
        let path = std::env::join_paths([
            bin,
            alias,
            originals,
            PathBuf::from("/usr/bin"),
            PathBuf::from("/bin"),
        ])
        .unwrap();
        Self {
            _tmp: tmp,
            repo,
            shim,
            real,
            path,
        }
    }

    fn command(&self) -> Command {
        let mut cmd = Command::new(&self.shim);
        cmd.current_dir(&self.repo)
            .env("PATH", &self.path)
            .env("BEADS_ACTOR", "test/embedded")
            .env("BD_TEST_CHILD_PATH", self._tmp.path().join("child-path"))
            .env("BD_TEST_HELPER_TRAP", self._tmp.path().join("helper-trap"))
            .env_remove("BD_REAL")
            .env_remove("VEGA_BD_SHIM_ACTIVE");
        cmd
    }
}

#[test]
fn embedded_fixture_skips_shim_dirs_and_preserves_output_and_status() {
    let f = Fixture::new();
    for args in [
        vec!["where", "--json"],
        vec!["dolt", "remote", "list"],
        vec!["backup", "--help"],
        vec!["dolt", "push"],
    ] {
        let out = f.command().args(&args).bounded_output().unwrap();
        assert_eq!(out.status.code(), Some(23));
        assert_eq!(
            String::from_utf8_lossy(&out.stdout),
            format!("real:{}:test/embedded\n", args.join(" "))
        );
        assert!(
            out.stderr.is_empty(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(
            !f._tmp.path().join("helper-trap").exists(),
            "metadata probes invoked git from the shim directory"
        );
        let child_path = fs::read_to_string(f._tmp.path().join("child-path")).unwrap();
        let dirs: Vec<_> = std::env::split_paths(&child_path).collect();
        assert!(!dirs.contains(&f.shim.parent().unwrap().to_path_buf()));
        assert!(!dirs.contains(&f._tmp.path().join("alias")));
    }
}

#[test]
fn copied_shim_override_cannot_recurse() {
    let f = Fixture::new();
    fs::copy(env!("CARGO_BIN_EXE_bd"), &f.real).unwrap();
    let out = f
        .command()
        .env("BD_REAL", &f.real)
        .arg("where")
        .bounded_output()
        .unwrap();
    assert_eq!(out.status.code(), Some(126));
    assert!(out.stdout.is_empty());
    assert!(String::from_utf8_lossy(&out.stderr).contains("recursive invocation blocked"));
}

#[test]
fn self_override_and_hardlink_on_path_are_skipped() {
    let f = Fixture::new();
    // Same filesystem as the test binary: a hardlink into /tmp fails with EXDEV where /tmp is a
    // separate mount (GitHub runners). Only this dir lives there; the fixture repo stays outside
    // the checkout so the shim cannot find vega's own .beads.
    let hard_tmp = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    let hard = hard_tmp.path().join("hard");
    fs::create_dir(&hard).unwrap();
    fs::hard_link(env!("CARGO_BIN_EXE_bd"), hard.join("bd")).unwrap();
    let path =
        std::env::join_paths(std::iter::once(hard.clone()).chain(std::env::split_paths(&f.path)))
            .unwrap();
    let out = f
        .command()
        .env("PATH", path)
        .env("BD_REAL", &f.shim)
        .arg("where")
        .bounded_output()
        .unwrap();
    assert_eq!(out.status.code(), Some(23));
    let child_path = fs::read_to_string(f._tmp.path().join("child-path")).unwrap();
    assert!(!std::env::split_paths(&child_path).any(|dir| dir == hard));
}

#[test]
fn stuck_metadata_helper_cannot_delay_passthrough_indefinitely() {
    let f = Fixture::new();
    let helpers = f._tmp.path().join("helpers");
    fs::create_dir(&helpers).unwrap();
    script(&helpers.join("git"), "sleep 30");
    let path = std::env::join_paths(std::iter::once(helpers).chain(std::env::split_paths(&f.path)))
        .unwrap();
    let start = Instant::now();
    let out = f
        .command()
        .env("PATH", path)
        .arg("where")
        .bounded_output()
        .unwrap();
    assert_eq!(out.status.code(), Some(23));
    assert!(start.elapsed() < Duration::from_secs(5));
}

#[test]
fn no_real_bd_fails_promptly_instead_of_reentering() {
    let f = Fixture::new();
    let path = std::env::join_paths([
        f.shim.parent().unwrap(),
        Path::new("/usr/bin"),
        Path::new("/bin"),
    ])
    .unwrap();
    let out = f
        .command()
        .env("PATH", path)
        .env("BD_REAL", &f.shim)
        .arg("where")
        .bounded_output()
        .unwrap();
    assert_eq!(out.status.code(), Some(127));
    assert!(out.stdout.is_empty());
    assert!(String::from_utf8_lossy(&out.stderr).contains("shim_target_not_found"));
}

/// Run with BD_EMBEDDED_TEST_BIN=/absolute/path/to/original/bd. Kept opt-in
/// because bd (and support for embedded mode) is not a build prerequisite.
#[test]
fn real_embedded_bd_returns_promptly() {
    let Some(real) = std::env::var_os("BD_EMBEDDED_TEST_BIN") else {
        eprintln!("skipped: set BD_EMBEDDED_TEST_BIN to an embedded-capable bd");
        return;
    };
    let f = Fixture::new();
    fs::remove_dir_all(f.repo.join(".beads")).unwrap();
    assert!(Command::new("/usr/bin/git")
        .args(["init", "-q"])
        .arg(&f.repo)
        .status()
        .unwrap()
        .success());
    let mut init = Command::new(&real);
    init.current_dir(&f.repo).args([
        "init",
        "--non-interactive",
        "--skip-hooks",
        "--prefix",
        "fixture",
    ]);
    let result = vega_bd_sync::bd::run(init, Duration::from_secs(60));
    assert!(result.ok, "{}", result.out);
    let metadata = fs::read_to_string(f.repo.join(".beads/metadata.json")).unwrap();
    assert!(metadata.contains("embedded"), "{metadata}");
    for args in [
        vec!["where", "--json"],
        vec!["dolt", "remote", "list"],
        vec!["backup", "--help"],
    ] {
        let mut cmd = f.command();
        cmd.env("BD_REAL", &real).args(&args);
        let result = vega_bd_sync::bd::run(cmd, Duration::from_secs(15));
        assert!(result.ok && !result.timed_out, "{args:?}: {}", result.out);
    }
}
