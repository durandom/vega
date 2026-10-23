//! Runs the built `vega-skills` against fake plugins on a private PATH.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, Output};

const GOOD: &str = "---\nname: vega-good\ndescription: Does good things, quickly. Use when good.\n---\n\n# vega good\n\nBODY-MARKER\n";

fn plugin(dir: &Path, name: &str, script: &str) {
    let path = dir.join(format!("vega-{name}"));
    fs::write(&path, format!("#!/bin/sh\n{script}\n")).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
}

fn run(dir: &Path, args: &[&str]) -> (Output, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_vega-skills"))
        .args(args)
        .env("PATH", dir)
        // Never read the real ~/.config/vega or skill dirs.
        .env("HOME", dir)
        .env("XDG_CONFIG_HOME", dir)
        .env("XDG_DATA_HOME", dir)
        // Under a loaded test run even `exit 2` can take seconds to spawn.
        .env("VEGA_SKILL_TIMEOUT_MS", "60000")
        .output()
        .unwrap();
    let stdout = String::from_utf8(out.stdout.clone()).unwrap();
    (out, stdout)
}

fn fixture() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    plugin(
        dir.path(),
        "good",
        &format!("[ \"$1\" = --skill ] && printf '%s' '{GOOD}'"),
    );
    plugin(dir.path(), "fails", "exit 2");
    plugin(dir.path(), "nofm", "echo '# no frontmatter'");
    plugin(dir.path(), "skills", "exit 0");
    dir
}

#[test]
fn index_lists_descriptions_without_bodies() {
    let dir = fixture();
    let (out, stdout) = run(dir.path(), &[]);
    assert!(out.status.success());
    assert!(stdout.contains("plugins[4]{name,description}:"), "{stdout}");
    assert!(stdout.contains("  good,\"Does good things, quickly. Use when good.\""));
    assert!(
        stdout
            .contains("  fails,no guide: `vega-fails --skill` exited 2; run `vega fails --help`\n"),
        "{stdout}"
    );
    assert!(stdout.contains("  nofm,invalid guide ("));
    assert!(!stdout.contains("BODY-MARKER"));
    // vega-skills's own guide is built in, whatever the binary on PATH prints.
    assert!(stdout.contains("\n  skills,\"Lists and loads"), "{stdout}");
    assert!(stdout.contains("`vega skills lib`"), "{stdout}");
}

#[test]
fn name_prints_full_guide() {
    let dir = fixture();
    for name in ["good", "vega-good"] {
        let (out, stdout) = run(dir.path(), &[name]);
        assert!(
            out.status.success(),
            "{stdout}{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(stdout, GOOD);
    }
}

#[test]
fn unknown_name_lists_valid_names() {
    let dir = fixture();
    let (out, stdout) = run(dir.path(), &["nope"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(stdout.starts_with("error{code,message,hint}:\n  unknown_guide,"));
    assert!(
        stdout.contains("valid names: fails good nofm skills,"),
        "{stdout}"
    );
}

#[test]
fn failing_plugin_is_a_runtime_error() {
    let dir = fixture();
    let (out, stdout) = run(dir.path(), &["fails"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(stdout.contains("no_guide,"));
}

#[test]
fn empty_path_is_a_definitive_empty_state() {
    let dir = tempfile::tempdir().unwrap();
    let (out, stdout) = run(dir.path(), &[]);
    assert!(out.status.success());
    assert!(stdout.contains("plugins: 0 vega plugins found on PATH"));
}

#[test]
fn rejects_unknown_flags_and_extra_args() {
    let dir = fixture();
    for args in [&["--nope"][..], &["good", "mux"][..]] {
        let (out, stdout) = run(dir.path(), args);
        assert_eq!(out.status.code(), Some(2));
        assert!(stdout.contains("usage,"));
    }
}

#[test]
fn own_guide_is_built_in() {
    let dir = fixture();
    for name in ["skills", "vega-skills"] {
        let (out, stdout) = run(dir.path(), &[name]);
        assert!(out.status.success(), "{stdout}");
        assert!(stdout.starts_with("---\nname: vega-skills\n"), "{stdout}");
    }
}

#[test]
fn old_top_level_library_commands_point_to_lib() {
    let dir = fixture();
    for cmd in ["install", "remove", "list", "sync", "doctor"] {
        let (out, stdout) = run(dir.path(), &[cmd, "x"]);
        assert_eq!(out.status.code(), Some(2), "{stdout}");
        assert!(
            stdout.contains(&format!(
                "  usage,`vega skills {cmd}` moved to `vega skills lib {cmd}`,run `vega skills lib {cmd} x`"
            )),
            "{stdout}"
        );
    }
}

#[test]
fn lib_rejects_unknown_commands() {
    let dir = fixture();
    let (out, stdout) = run(dir.path(), &["lib", "nope"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(stdout.contains("unknown lib command nope"), "{stdout}");
    assert!(stdout.contains("Usage: vega skills lib\n"), "{stdout}");
}
