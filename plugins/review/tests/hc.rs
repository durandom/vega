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

#[test]
fn review_uses_the_pane_hosts_local_mode_record() {
    let (dir, mut c) = common::fixture(env!("CARGO_BIN_EXE_vega-review"));
    // Stop just beyond the ownership check, before any inbox or browser starts.
    let state = dir.path().join("state/vega");
    std::fs::create_dir_all(&state).unwrap();
    std::fs::write(state.join("review"), "not a directory").unwrap();
    c.args(["start", "http://127.0.0.1:5287/"]);
    let modes = state.join("modes");
    std::fs::create_dir(&modes).unwrap();
    // A caller-side remote-scoped record cannot authorize the pane host.
    std::fs::write(
        modes.join("remote_hc-7437"),
        r#"{"name":"child","mode":"headed"}"#,
    )
    .unwrap();
    let out = c.output().unwrap();
    assert!(String::from_utf8_lossy(&out.stdout).contains("headless_pane"));
    for (name, mode, expected) in [
        ("child", "headed", "write_failed"),
        ("child", "headless", "headless_pane"),
        ("previous", "headed", "headless_pane"),
    ] {
        std::fs::write(
            modes.join("local_hc-7437"),
            format!(r#"{{"name":"{name}","mode":"{mode}"}}"#),
        )
        .unwrap();
        let out = c.output().unwrap();
        let text = String::from_utf8_lossy(&out.stdout);
        assert!(!out.status.success());
        assert!(text.contains(expected), "{name}/{mode}: {text}");
    }
}
