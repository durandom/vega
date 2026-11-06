#[path = "../../../crates/vega-herdr/tests/fixtures/own-terminal.rs"]
mod common;
#[test]
fn hc_wait_and_clear_use_verified_caller() {
    let (dir, mut c) = common::fixture(env!("CARGO_BIN_EXE_vega-orchestration"));
    let o = c
        .args(["waiting", "--on", "Slayer", "--for", "decision pending"])
        .output()
        .unwrap();
    assert!(o.status.success(), "{o:?}");
    let file = dir.path().join("state/vega/orchestration/waits.sqlite3");
    let wait = vega_herdr::waits::Store::read(&file)
        .unwrap()
        .pop()
        .unwrap();
    let (_other, mut c) = common::fixture(env!("CARGO_BIN_EXE_vega-orchestration"));
    let o = c
        .env("XDG_STATE_HOME", dir.path().join("state"))
        .args(["clear", &wait.id, "--evidence", "decision received"])
        .output()
        .unwrap();
    assert!(o.status.success(), "{o:?}");
}
