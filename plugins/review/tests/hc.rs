#[path = "../../../crates/vega-herdr/tests/fixtures/own-terminal.rs"]
mod common;
#[test]
fn hc_review_reads_own_session_and_rejects_headless_child_start() {
    let (_dir, mut c) = common::fixture(env!("CARGO_BIN_EXE_vega-review"));
    let o = c.output().unwrap();
    assert!(o.status.success(), "{o:?}");
    let o = c
        .args(["start", "http://127.0.0.1:5287/"])
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&o.stdout);
    assert!(!o.status.success());
    assert!(
        text.contains("headless") || text.contains("headed"),
        "{text}"
    );
    assert!(!text.contains("not_in_terminal"), "{text}");
}
