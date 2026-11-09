use serde_json::{json, Value};
use std::process::Command;

#[test]
fn json_watches_filter_project_and_include_verdict_and_failed_checks() {
    let tmp = tempfile::tempdir().unwrap();
    let watch = tmp.path().join("vega/ci/watch");
    std::fs::create_dir_all(&watch).unwrap();
    let t = vega_ci_watch::Target::run("github.com", "team/repo", 42);
    let w = vega_ci_watch::store::Watch {
        target: t,
        pane: "t1".into(),
        since: 1,
    };
    std::fs::write(watch.join("test.json"), serde_json::to_vec(&w).unwrap()).unwrap();
    let run = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_vega-ci"))
            .args(args)
            .env("HOME", tmp.path())
            .env("XDG_STATE_HOME", tmp.path())
            .current_dir(tmp.path())
            .output()
            .unwrap()
    };
    let o = run(&["--json", "--project", "github.com/team/repo"]);
    assert!(o.status.success());
    let v: Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(v["schema"], 1);
    assert_eq!(v["watches"][0]["verdict"], "unpolled");
    assert_eq!(v["watches"][0]["failed_checks"], json!([]));
    assert_eq!(v["watches"][0]["watchers"], json!(["t1"]));
    let mut state = vega_ci_watch::store::State::new(&w.target, 1);
    state.polled = 50;
    state.head = Some("abc123".into());
    state.runs.push(vega_ci_watch::Check {
        name: "test failing".into(),
        bucket: vega_ci_watch::Bucket::Fail,
        note: None,
    });
    let states = tmp.path().join("vega/ci/state");
    std::fs::create_dir_all(&states).unwrap();
    std::fs::write(
        states.join(format!("{}.json", w.target.key())),
        serde_json::to_vec(&state).unwrap(),
    )
    .unwrap();
    let o = run(&["--json", "--project", "github.com/team/repo"]);
    let v: Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(v["watches"][0]["state"], "active");
    assert_eq!(v["watches"][0]["verdict"], "red");
    assert_eq!(v["watches"][0]["failed_checks"], json!(["test failing"]));
    assert_eq!(v["watches"][0]["polled"], 50);

    let o = run(&["--json", "--project", "github.com/other/repo"]);
    let v: Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(v["watches"], json!([]));
    let o = run(&["--json", "--bad"]);
    assert_eq!(o.status.code(), Some(2));
    let v: Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(v["error"]["code"], "usage");
}
