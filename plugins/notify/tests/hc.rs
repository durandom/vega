#[path = "../../../crates/vega-herdr/tests/fixtures/own-terminal.rs"]
mod common;
#[test]
fn hc_notify_records_own_pane_without_herdr() {
    let (dir, mut c) = common::fixture(env!("CARGO_BIN_EXE_vega-notify"));
    let o = c
        .args(["info", "demo: test -> t7", "--project", "demo"])
        .output()
        .unwrap();
    assert!(o.status.success(), "{o:?}");
    let notes = vega_herdr::notes::Store {
        dir: dir.path().join("state/vega/notes"),
    }
    .list();
    assert_eq!(notes.len(), 1);
    assert_eq!(notes[0].pane.as_deref(), Some("t7"));
}
