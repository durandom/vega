//! The binary on the recorded token-burn fixture and a temp HOME.

use std::path::Path;
use std::process::{Command, Output};

const FIXTURE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../crates/vega-herdr/tests/fixtures/token-burn"
);

/// A provider and window vega's code does not know (acme), and a thin
/// model-scoped claude window.
const UNKNOWN: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../crates/vega-herdr/tests/fixtures/unknown-provider"
);

fn run(home: &Path, args: &[&str]) -> Output {
    run_on(FIXTURE, home, args)
}

fn run_on(fixture: &str, home: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_vega-capacity"))
        .args(args)
        .env("HOME", home)
        .env_remove("XDG_CONFIG_HOME")
        .env("XDG_STATE_HOME", home.join("state"))
        .env("VEGA_CAPACITY_FIXTURE", fixture)
        .env("VEGA_TOKEN_BURN", "/nonexistent/token-burn")
        .output()
        .unwrap()
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

#[test]
fn home_view_reads_the_fixture_and_writes_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let o = run(tmp.path(), &[]);
    assert!(o.status.success());
    let out = stdout(&o);
    assert!(
        out.starts_with("capacity[14]{provider,window,scope,left,resets_in,runs_out,binding}:\n")
    );
    assert!(out.contains("\n  claude,seven_day_fable,model:fable,"));
    assert!(out.contains("\nresets: none recorded\n"));
    // The recording is days old: stale, and the fixture has no service file.
    assert!(out.contains("\ntoken_burn: stale (fixture, service unknown, sample "));
    assert!(!tmp.path().join("state").exists());
}

#[test]
fn ledger_commands_write_the_state_file() {
    let tmp = tempfile::tempdir().unwrap();
    let o = run(tmp.path(), &["resets", "claude", "2"]);
    assert!(o.status.success());
    assert_eq!(
        stdout(&o),
        "resets[1]{provider,owned,plan,override_until,last_used}:\n  claude,2,,,\n"
    );
    assert!(tmp.path().join("state/vega/capacity.json").is_file());
    let o = run(tmp.path(), &["used", "claude"]);
    assert!(stdout(&o).contains("\n  claude,1,,,"));
}

#[test]
fn bad_input_is_a_structured_error() {
    let tmp = tempfile::tempdir().unwrap();
    let o = run(tmp.path(), &["reset", "claude"]);
    assert_eq!(o.status.code(), Some(2));
    assert!(stdout(&o).starts_with("error{code,message,hint}:\n  usage,"));
    let o = run(tmp.path(), &["resets", "nosuch", "1"]);
    assert_eq!(o.status.code(), Some(1));
    assert!(stdout(&o).contains("unknown_provider"));
}

#[test]
fn a_provider_only_the_fixture_knows_shows_binds_and_takes_ledger_entries() {
    let tmp = tempfile::tempdir().unwrap();
    let o = run_on(UNKNOWN, tmp.path(), &[]);
    assert!(o.status.success());
    let out = stdout(&o);
    assert!(out.contains("\n  acme,daily,account,55%,"), "{out}");
    assert!(
        out.contains("\n  acme,monthly,account,8%,") && binds(&out, "acme,monthly,"),
        "{out}"
    );
    // The thinnest claude window is model-scoped: it never binds.
    assert!(
        out.lines()
            .any(|l| l.starts_with("  claude,seven_day_fable,model:fable,2%,") && l.ends_with(",")),
        "{out}"
    );
    assert!(binds(&out, "claude,five_hour,"), "{out}");
    let o = run_on(UNKNOWN, tmp.path(), &["resets", "acme", "1"]);
    assert!(o.status.success(), "{}", stdout(&o));
    assert!(stdout(&o).contains("\n  acme,1,,,"));
}

#[test]
fn a_local_windows_toml_replaces_the_default_scopes() {
    let tmp = tempfile::tempdir().unwrap();
    let cfg = tmp.path().join(".config/vega/config/capacity");
    std::fs::create_dir_all(&cfg).unwrap();
    std::fs::write(
        cfg.join("windows.toml"),
        "[acme]\nmonthly = \"model:turbo\"\n",
    )
    .unwrap();
    let out = stdout(&run_on(UNKNOWN, tmp.path(), &[]));
    assert!(binds(&out, "acme,daily,account,55%,"), "{out}");
    assert!(out.contains("\n  acme,monthly,model:turbo,8%,"), "{out}");
    // Not listed locally: fable is account-wide now and binds.
    assert!(binds(&out, "claude,seven_day_fable,account,"), "{out}");
}

/// The row starting with `prefix` is marked binding.
fn binds(out: &str, prefix: &str) -> bool {
    out.lines()
        .any(|l| l.starts_with(&format!("  {prefix}")) && l.ends_with(",yes"))
}
