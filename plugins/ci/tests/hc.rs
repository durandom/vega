#[path = "../../../crates/vega-herdr/tests/fixtures/own-terminal.rs"]
mod common;
#[test]
fn hc_watch_registers_own_address_and_rejects_stale_identity() {
    let (_dir, mut c) = common::fixture(env!("CARGO_BIN_EXE_vega-ci"));
    c.args(["watch", "acme/repo#42"]);
    let o = c.output().unwrap();
    let text = String::from_utf8_lossy(&o.stdout);
    assert!(o.status.success(), "{text}");
    assert!(text.contains("t7"), "{text}");
    let o = c.env("TEST_PARENT_PID", "4294967295").output().unwrap();
    assert!(!o.status.success());
    assert!(String::from_utf8_lossy(&o.stdout).contains("not_in_terminal"));
}
