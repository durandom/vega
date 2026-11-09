use serde_json::{json, Value};
use std::process::Command;
#[test]
fn json_status_has_an_explicit_empty_state_and_json_errors() {
    let tmp = tempfile::tempdir().unwrap();
    let run = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_vega-bd"))
            .args(args)
            .env("HOME", tmp.path())
            .env("XDG_STATE_HOME", tmp.path())
            .current_dir(tmp.path())
            .output()
            .unwrap()
    };
    let o = run(&["status", "--json"]);
    assert!(o.status.success());
    let v: Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(v, json!({"schema":1,"projects":[]}));
    let o = run(&["status", "--json", "--bad"]);
    assert_eq!(o.status.code(), Some(2));
    let v: Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(v["error"]["code"], "usage");
    assert!(!tmp.path().join("vega").exists());
}

#[test]
fn json_status_reports_stopped_project_timestamps_and_filters() {
    let tmp = tempfile::tempdir().unwrap();
    let common = tmp.path().join("repo/.git");
    let key = "github.com/team/repo";
    let encode = vega_herdr::project::encode_component;
    let root = tmp.path().join("vega/bd");
    let path = |kind: &str| {
        root.join(kind)
            .join("v2")
            .join(encode(key))
            .join(format!("{}.json", encode(common.to_str().unwrap())))
    };
    let p = json!({"key":key,"common":common,"dir":tmp.path(),"database":"REPO","remote":"http://127.0.0.1:1/REPO","bd":"/nonexistent/bd","data":tmp.path().join(".beads/dolt"),"since":1,"resumed":0});
    let sync = vega_bd_sync::store::State {
        pulled: 10,
        pushed: 20,
        stopped: Some(30),
        stop_reason: Some("merge conflict".into()),
        ..Default::default()
    };
    for (kind, value) in [
        ("projects", p),
        ("state", serde_json::to_value(sync).unwrap()),
    ] {
        let file = path(kind);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(
            file,
            json!({"schema":2,"project":key,"common_dir":common,"value":value}).to_string(),
        )
        .unwrap();
    }
    let run = |project: &str| {
        Command::new(env!("CARGO_BIN_EXE_vega-bd"))
            .args(["status", "--json", "--project", project])
            .env("HOME", tmp.path())
            .env("XDG_STATE_HOME", tmp.path())
            .current_dir(tmp.path())
            .output()
            .unwrap()
    };
    let o = run(key);
    assert!(o.status.success());
    let v: Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(v["projects"][0]["state"], "stopped");
    assert_eq!(v["projects"][0]["last_pull"], 10);
    assert_eq!(v["projects"][0]["last_push"], 20);
    assert_eq!(v["projects"][0]["conflict"], "merge conflict");
    let o = run("github.com/other/repo");
    let v: Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(v["projects"], json!([]));
}
