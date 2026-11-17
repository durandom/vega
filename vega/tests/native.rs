//! Executable fixtures work without a shell on Windows and Unix.
use std::{env, fs, process::Command};
#[test]
fn child_fixture() {
    if env::var_os("VEGA_DISPATCH_FIXTURE").is_some() {
        let args: Vec<_> = env::args().skip(1).collect();
        assert_eq!(args, ["--exact", "child_fixture", "--nocapture"]);
        println!("native dispatcher child");
        std::process::exit(17);
    }
}
#[test]
fn discovers_and_dispatches_native_executable_preserving_exit() {
    let root = tempfile::tempdir().unwrap();
    let bin = root.path().join("bin with spaces");
    fs::create_dir(&bin).unwrap();
    let fixture = bin.join(format!("vega-post{}", env::consts::EXE_SUFFIX));
    fs::copy(env::current_exe().unwrap(), &fixture).unwrap();
    let vega = env!("CARGO_BIN_EXE_vega");
    let out = Command::new(vega).env("PATH", &bin).output().unwrap();
    assert!(out.status.success());
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.lines().any(|line| line == "  post"), "{text}");
    let out = Command::new(vega)
        .args(["post", "--exact", "child_fixture", "--nocapture"])
        .env("PATH", &bin)
        .env("VEGA_DISPATCH_FIXTURE", "1")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(17));
    assert!(String::from_utf8(out.stdout)
        .unwrap()
        .contains("native dispatcher child"));
}
