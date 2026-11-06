#[path = "../../../crates/vega-herdr/tests/fixtures/own-terminal.rs"]
mod common;
use std::io::Write;
use std::process::Stdio;
#[test]
fn hc_hook_routes_native_parent_and_preserves_headed_mode() {
    let (dir, mut c) = common::fixture(env!("CARGO_BIN_EXE_vega-mux"));
    c.args(["hook", "ask-parent"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped());
    let run = |c: &mut std::process::Command| {
        let mut child = c.spawn().unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(br#"{"tool_name":"AskUserQuestion","tool_input":{"questions":[]}}"#)
            .unwrap();
        String::from_utf8(child.wait_with_output().unwrap().stdout).unwrap()
    };
    let text = run(&mut c);
    assert!(
        text.contains("deny") && text.contains("vega post send t2"),
        "{text}"
    );
    let parents = dir.path().join("state/vega/parents");
    std::fs::create_dir_all(&parents).unwrap();
    std::fs::write(
        parents.join("local_hc-7437"),
        r#"{"name":"child","parent":"w1:p2","host":"remote"}"#,
    )
    .unwrap();
    let text = run(&mut c);
    assert!(
        text.contains("vega post send --host remote w1:p2"),
        "{text}"
    );
    let modes = dir.path().join("state/vega/modes");
    std::fs::create_dir_all(&modes).unwrap();
    std::fs::write(
        modes.join("local_hc-7437"),
        r#"{"name":"child","mode":"headed"}"#,
    )
    .unwrap();
    assert!(run(&mut c).is_empty());
    c.env("TEST_PARENT_PID", "4294967295");
    assert!(run(&mut c).is_empty());
}
