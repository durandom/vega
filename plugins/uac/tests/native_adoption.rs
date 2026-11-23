//! Run the Windows receiver with an owned hc RPC fake and isolated state.

#[cfg(not(windows))]
fn main() {}

#[cfg(windows)]
fn main() {
    use serde_json::{json, Value};
    use std::{env, fs, io::BufRead, process::Command};

    if env::args().nth(1).as_deref() == Some("connect") {
        let line = std::io::stdin().lock().lines().next().unwrap().unwrap();
        let request: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(request["method"], "terminal.get");
        assert_eq!(request["params"]["terminal"], "t9");
        println!(
            "{}",
            json!({"jsonrpc":"2.0","id":request["id"],"result":{
                "address":"t9","state":"live","name":"worker","argv":[],"labels":{},
                "created_ms":0,"screen":"exact","cols":80,"rows":24,
                "cwd":env::current_dir().unwrap(),"status":{"app":"codex","state":"idle"}
            }})
        );
        return;
    }

    let root = tempfile::tempdir().unwrap();
    fs::copy(env::current_exe().unwrap(), root.path().join("hc.exe")).unwrap();
    let path = env::join_paths(
        std::iter::once(root.path().to_owned())
            .chain(env::split_paths(&env::var_os("PATH").unwrap())),
    )
    .unwrap();
    let request = |name, parent, mode, brief_ms| {
        json!({"schema":1,"target":"hc:t9","name":name,
            "parent":{"pane":parent,"host":"proteus"},"mode":mode,"brief_ms":brief_ms})
    };
    let receive = |request: &Value| {
        Command::new(env!("CARGO_BIN_EXE_vega-uac"))
            .args(["record-adoption", &request.to_string()])
            .current_dir(root.path())
            .env("PATH", &path)
            .env("XDG_STATE_HOME", root.path().join("state"))
            .output()
            .unwrap()
    };
    for payload in [
        request("worker", "t1", "headed", 1),
        request("worker", "t2", "headless", 2),
    ] {
        let out = receive(&payload);
        assert!(out.status.success(), "{out:?}");
        assert_eq!(
            serde_json::from_slice::<Value>(&out.stdout).unwrap(),
            payload
        );
    }
    let out = receive(&request("different-worker", "t3", "headed", 3));
    assert!(!out.status.success(), "{out:?}");
    let state = root.path().join("state").join("vega");
    let saved = |kind| {
        let file = fs::read_dir(state.join(kind))
            .unwrap()
            .next()
            .unwrap()
            .unwrap();
        fs::read_to_string(file.path()).unwrap()
    };
    assert_eq!(
        serde_json::from_str::<Value>(&saved("parents")).unwrap()["parent"],
        "t2"
    );
    assert_eq!(
        serde_json::from_str::<Value>(&saved("modes")).unwrap()["mode"],
        "headless"
    );
    assert_eq!(saved("briefs"), "2");
}
