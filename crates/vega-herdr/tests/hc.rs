use std::{env, fs, os::unix::fs::PermissionsExt, process::Command, time::Duration};
use vega_herdr::{hc_local, hc_on, own_terminal, Terminal};

#[test]
fn fake_hc_contract() {
    if let Ok(case) = env::var("VEGA_HC_TEST") {
        match case.as_str() {
            "hc" | "hc_foreground" => {
                assert_eq!(own_terminal().unwrap(), Terminal::Hc("t7".into()))
            }
            "hc_agent_cwd" | "hc_agent_launch_cwd" => {
                let terminal = own_terminal().unwrap();
                let metadata = terminal.agent().unwrap();
                assert_eq!(
                    metadata["cwd"],
                    if case == "hc_agent_cwd" {
                        "/agent/rally-coach"
                    } else {
                        "/shell/vega"
                    }
                );
            }
            "herdr" => assert_eq!(own_terminal().unwrap(), Terminal::Herdr("w1:p2".into())),
            "stale" | "finished" | "wrong_address" | "neither" | "stale_herdr" => {
                assert!(own_terminal().is_err())
            }
            "on" => {
                assert_eq!(
                    hc_on(Some("remote"), &["run", "--bg", "--", "echo", "hi"], None).unwrap()
                        ["ok"],
                    true
                );
            }
            "bad" => assert_eq!(hc_local(&["ls"]).unwrap_err().code, "hc_bad_output"),
            "exit" => assert_eq!(hc_local(&["ls"]).unwrap_err().code, "hc_error"),
            "timeout" => assert_eq!(
                hc_on(None, &["ls"], Some(Duration::from_millis(50)))
                    .unwrap_err()
                    .code,
                "hc_timeout"
            ),
            "missing" => assert_eq!(hc_local(&["ls"]).unwrap_err().code, "hc_not_found"),
            kind => {
                let f = hc_local(&["ls"]).unwrap_err();
                let expected = match kind {
                    "not_found" => "pane_not_found".into(),
                    "machine_unknown" => "unknown_machine".into(),
                    "blocked" => "agent_blocked".into(),
                    "needs_target" | "invalid_params" => "usage".into(),
                    k => format!("hc_{k}"),
                };
                assert_eq!(f.code, expected);
                assert_eq!(f.hint, "try again");
                assert_eq!(f.message, "probe");
            }
        }
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let pid = std::process::id();
    let fake = dir.path().join("hc");
    fs::write(&fake, format!(r#"#!/bin/sh
printf '%s\n' "$@" > "$VEGA_HC_ARGS"
case "$VEGA_HC_TEST" in
hc) printf '%s' '{{"state":"live","address":"t7","pid":{pid}}}' ;;
hc_foreground) printf '%s' '{{"state":"live","address":"t7","pid":0,"foreground":{{"pid":{pid}}}}}' ;;
hc_agent_cwd) printf '%s' '{{"state":"live","address":"t7","pid":{pid},"cwd":"/shell/vega","foreground":{{"pid":{pid},"cwd":"/agent/rally-coach"}},"status":{{"app":"claude-code","state":"idle"}}}}' ;;
hc_agent_launch_cwd) printf '%s' '{{"state":"live","address":"t7","pid":{pid},"cwd":"/shell/vega","foreground":null,"status":{{"app":"claude-code","state":"idle"}}}}' ;;
stale) printf '%s' '{{"state":"live","address":"t7","pid":4294967295}}' ;;
finished) printf '%s' '{{"state":"finished","address":"t7","pid":{pid}}}' ;;
wrong_address) printf '%s' '{{"state":"live","address":"t8","pid":{pid}}}' ;;
on) printf '%s' '{{"ok":true}}' ;;
bad) printf nope ;;
exit) exit 4 ;;
timeout) /bin/sleep 5 ;;
*) printf '{{"error":{{"kind":"%s","message":"probe","hint":"try again"}}}}' "$VEGA_HC_TEST" >&2; exit 3 ;;
esac
"#)).unwrap();
    fs::set_permissions(&fake, fs::Permissions::from_mode(0o755)).unwrap();
    let herdr = dir.path().join("herdr");
    fs::write(&herdr, format!(r#"#!/bin/sh
case "$2" in
list) printf '%s' '{{"result":{{"panes":[{{"pane_id":"w1:p2","agent":"codex"}}]}}}}' ;;
process-info) if [ "$VEGA_HC_TEST" = herdr ]; then printf '%s' '{{"result":{{"process_info":{{"shell_pid":{pid}}}}}}}'; else printf '{{}}'; fi ;;
esac
"#)).unwrap();
    fs::set_permissions(&herdr, fs::Permissions::from_mode(0o755)).unwrap();
    for case in [
        "hc",
        "hc_foreground",
        "hc_agent_cwd",
        "hc_agent_launch_cwd",
        "herdr",
        "stale",
        "finished",
        "wrong_address",
        "neither",
        "stale_herdr",
        "on",
        "bad",
        "exit",
        "timeout",
        "not_found",
        "machine_unknown",
        "blocked",
        "needs_target",
        "invalid_params",
        "finished_error",
        "status_mismatch",
        "unavailable",
        "missing",
    ] {
        let args = dir.path().join("args");
        let _ = fs::remove_file(&args);
        let mut cmd = Command::new(env::current_exe().unwrap());
        cmd.args(["--exact", "fake_hc_contract", "--nocapture"])
            .env("VEGA_HC_TEST", case)
            .env("VEGA_HC_ARGS", &args)
            .env_remove("HERDR_PANE_ID")
            .env_remove("HC_TERMINAL_ID")
            .env(
                "PATH",
                if case == "missing" {
                    "/nonexistent".into()
                } else {
                    format!(
                        "{}:{}",
                        dir.path().display(),
                        env::var("PATH").unwrap_or_default()
                    )
                },
            );
        if case != "neither" {
            cmd.env("HC_TERMINAL_ID", "t7");
        }
        if case == "herdr" || case == "stale_herdr" {
            cmd.env("HERDR_PANE_ID", "w1:p2");
        }
        let out = cmd.output().unwrap();
        assert!(
            out.status.success(),
            "{case}: {} {}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        if case == "on" {
            assert_eq!(
                fs::read_to_string(&args).unwrap(),
                "--json\nrun\n--bg\n--on\nremote\n--\necho\nhi\n"
            );
        }
        if case == "hc" {
            assert_eq!(fs::read_to_string(&args).unwrap(), "--json\nget\nt7\n");
        }
        if ["herdr", "stale_herdr", "neither"].contains(&case) {
            assert!(!args.exists(), "{case} must not call hc");
        }
    }
}

#[test]
fn fleet_prefers_hc_and_only_falls_back_when_empty() {
    if let Ok(case) = env::var("VEGA_FLEET_TEST") {
        let result = vega_herdr::hc::machines();
        match case.as_str() {
            "hc" => {
                let list = result.unwrap();
                assert_eq!(list[0]["label"], "corp");
                assert_eq!(list[0]["target"], "user@corp:22");
                assert_eq!(list[0]["backend"], "hc");
            }
            "empty" => assert_eq!(result.unwrap()[0]["label"], "legacy"),
            "bad" => assert_eq!(result.unwrap_err().code, "hc_bad_output"),
            _ => assert_eq!(result.unwrap_err().code, "hc_error"),
        }
        return;
    }
    let d = tempfile::tempdir().unwrap();
    for (name,text) in [("hc",r#"#!/bin/sh
case "$VEGA_FLEET_TEST" in
hc) echo '{"machines":[{"name":"corp","target":"user@corp:22","host":"corp","via":false}]}' ;;
empty) echo '{"machines":[]}' ;;
bad) echo '{}' ;;
*) exit 1 ;;
esac
"#),("herdr","#!/bin/sh\necho '[{\"label\":\"legacy\",\"target\":\"old\",\"enabled\":true}]'\ntouch \"$HOME/fallback\"\n")] {
        fs::write(d.path().join(name),text).unwrap();fs::set_permissions(d.path().join(name),fs::Permissions::from_mode(0o755)).unwrap();
    }
    for case in ["hc", "empty", "bad", "failed"] {
        let _ = fs::remove_file(d.path().join("fallback"));
        let o = Command::new(env::current_exe().unwrap())
            .args(["--exact", "fleet_prefers_hc_and_only_falls_back_when_empty"])
            .env_clear()
            .env("HOME", d.path())
            .env("PATH", format!("{}:/usr/bin:/bin", d.path().display()))
            .env("VEGA_FLEET_TEST", case)
            .output()
            .unwrap();
        assert!(
            o.status.success(),
            "{}{}",
            String::from_utf8_lossy(&o.stdout),
            String::from_utf8_lossy(&o.stderr)
        );
        assert_eq!(d.path().join("fallback").exists(), case == "empty");
    }
}

#[test]
fn rpc_contract() {
    if let Ok(case) = env::var("VEGA_HC_RPC_TEST") {
        let result = vega_herdr::hc::rpc(
            "terminal.run",
            serde_json::json!({
                "argv":["codex","literal $text; no shell"], "env":{"SPECIAL":"a b"},
                "on":"remote", "outlive_parent":true,
            }),
            Duration::from_millis(200),
        );
        match case.as_str() {
            "ok" => assert_eq!(result.unwrap()["address"], "remote/t7"),
            "error" => assert_eq!(result.unwrap_err().code, "hc_spawn_failed"),
            "timeout" => assert_eq!(result.unwrap_err().code, "hc_timeout"),
            _ => assert_eq!(result.unwrap_err().code, "hc_bad_output"),
        }
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let fake = dir.path().join("hc");
    fs::write(&fake, r#"#!/bin/sh
[ "$1" = connect ] || exit 1
cat > "$VEGA_HC_ARGS"
case "$VEGA_HC_RPC_TEST" in
ok) echo '{"jsonrpc":"2.0","id":1,"result":{"address":"remote/t7"}}' ;;
error) echo '{"jsonrpc":"2.0","id":1,"error":{"message":"failed","data":{"kind":"spawn_failed","hint":"check cwd"}}}' ;;
timeout) sleep 5 ;;
wrong_id) echo '{"jsonrpc":"2.0","id":2,"result":{}}' ;;
missing_result) echo '{"jsonrpc":"2.0","id":1}' ;;
*) echo not-json ;;
esac
"#).unwrap();
    fs::set_permissions(&fake, fs::Permissions::from_mode(0o755)).unwrap();
    for case in [
        "ok",
        "error",
        "timeout",
        "wrong_id",
        "missing_result",
        "malformed",
    ] {
        let args = dir.path().join("args");
        let out = Command::new(env::current_exe().unwrap())
            .args(["--exact", "rpc_contract", "--nocapture"])
            .env("VEGA_HC_RPC_TEST", case)
            .env("VEGA_HC_ARGS", &args)
            .env("PATH", format!("{}:/usr/bin:/bin", dir.path().display()))
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{case}: {}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        let request: serde_json::Value = serde_json::from_slice(&fs::read(args).unwrap()).unwrap();
        assert_eq!(request["method"], "terminal.run");
        assert_eq!(request["params"]["argv"][1], "literal $text; no shell");
        assert_eq!(request["params"]["env"]["SPECIAL"], "a b");
    }
}

#[test]
fn empty_tui_environment_is_absent_for_history_and_errors() {
    if env::var_os("VEGA_HC_EMPTY_TEST").is_some() {
        let event = vega_herdr::history::event("host", "test", serde_json::json!({}));
        assert!(event["pane"].is_null(), "{event}");
        assert!(event["session"].is_null(), "{event}");
        let failure = vega_herdr::herdr_local(&["agent", "list"]).unwrap_err();
        assert_eq!(failure.code, "socket_denied");
        assert!(
            failure.message.ends_with("herdr socket"),
            "{}",
            failure.message
        );
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let herdr = dir.path().join("herdr");
    fs::write(&herdr, "#!/bin/sh\necho PermissionDenied >&2\nexit 1\n").unwrap();
    fs::set_permissions(&herdr, fs::Permissions::from_mode(0o755)).unwrap();
    let out = Command::new(env::current_exe().unwrap())
        .args([
            "--exact",
            "empty_tui_environment_is_absent_for_history_and_errors",
            "--nocapture",
        ])
        .env_clear()
        .env("VEGA_HC_EMPTY_TEST", "1")
        .env("HERDR_PANE_ID", "")
        .env("HERDR_SOCKET_PATH", "")
        .env("CODEX_THREAD_ID", "must-not-bind")
        .env("PATH", format!("{}:/usr/bin:/bin", dir.path().display()))
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

// Keep the two hc wire contracts distinct: `ls` is a display summary, while
// terminal.list and CLI get expose full Terminal metadata used by discovery.
#[test]
fn terminal_list_contract_preserves_full_metadata() {
    use serde_json::{json, Value};
    if env::var_os("VEGA_HC_LIST_TEST").is_some() {
        let summary = hc_local(&["ls"]).unwrap();
        assert_eq!(summary["terminals"][0]["status"], "working");
        assert!(summary["terminals"][0].get("labels").is_none());
        assert!(summary["terminals"][0].get("foreground").is_none());
        let list = vega_herdr::hc::terminal_list().unwrap();
        let terminal = &list["terminals"][0];
        assert_eq!(terminal["status"]["app"], "claude-code");
        assert_eq!(terminal["status"]["state"], "working");
        assert_eq!(terminal["foreground"]["cwd"], "/projects/rally-coach");
        assert_eq!(terminal["labels"]["herdr.space"], "sp1");
        assert_eq!(terminal["parent"], "t2");
        assert_eq!(list["terminals"][1]["address"], "ganymede/t1g");
        assert_eq!(&hc_local(&["get", "t34"]).unwrap(), terminal);
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let terminal = json!({"address":"t34", "name":"coach", "parent":"t2",
        "state":"live", "cwd":"/home/user", "argv":["/bin/zsh","-l"],
        "pid":123, "turn":4, "foreground":{"argv":["claude"],
            "cwd":"/projects/rally-coach", "name":"claude", "pid":456},
        "status":{"app":"claude-code","state":"working"},
        "labels":{"herdr.space":"sp1","vega.project":"vega"}});
    let mut remote = terminal.clone();
    remote["address"] = json!("ganymede/t1g");
    for (name, value) in [
        ("terminal", terminal.clone()),
        (
            "list",
            json!({"jsonrpc":"2.0","id":1,"result":{
            "hidden_finished":0,"machines":[],"terminals":[
                terminal,remote]}}),
        ),
        (
            "summary",
            json!({"hidden_finished":0,"machines":[],"terminals":[{
            "address":"t34","agent":"claude-code","cwd":"/home/user",
            "state":"live","status":"working","turn":4,"where":"coach / 1"}]}),
        ),
    ] {
        fs::write(dir.path().join(name), value.to_string()).unwrap();
    }
    let fake = dir.path().join("hc");
    fs::write(
        &fake,
        r#"#!/bin/sh
case "$1 $2" in
 'connect ')
   cat > "$VEGA_HC_ARGS"
   cat "$VEGA_HC_LIST_TEST/list" ;;
 '--json ls') cat "$VEGA_HC_LIST_TEST/summary" ;;
 '--json get') cat "$VEGA_HC_LIST_TEST/terminal" ;;
 *) exit 1 ;;
esac
"#,
    )
    .unwrap();
    fs::set_permissions(&fake, fs::Permissions::from_mode(0o755)).unwrap();
    let request = dir.path().join("request");
    let out = Command::new(env::current_exe().unwrap())
        .args([
            "--exact",
            "terminal_list_contract_preserves_full_metadata",
            "--nocapture",
        ])
        .env("VEGA_HC_LIST_TEST", dir.path())
        .env("VEGA_HC_ARGS", &request)
        .env("PATH", format!("{}:/usr/bin:/bin", dir.path().display()))
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let request: Value = serde_json::from_slice(&fs::read(request).unwrap()).unwrap();
    assert_eq!(
        request,
        json!({"jsonrpc":"2.0","id":1,"method":"terminal.list","params":{}})
    );
}
