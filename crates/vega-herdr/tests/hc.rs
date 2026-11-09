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
                        ["address"],
                    "remote/t7"
                );
            }
            "send" => assert_eq!(
                hc_local(&["send", "t8", "literal --text"]).unwrap()["turn"],
                1
            ),
            "usage" => {
                for args in [
                    &["ls", "--all"][..],
                    &["exec", "word"][..],
                    &["get", "t7", "--bg"][..],
                ] {
                    assert_eq!(hc_local(args).unwrap_err().code, "usage", "{args:?}");
                }
            }
            "bad" => assert_eq!(hc_local(&["ls"]).unwrap_err().code, "hc_bad_output"),
            "exit" => assert_eq!(hc_local(&["ls"]).unwrap_err().code, "hc_bad_output"),
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
    fs::write(&fake, format!(r#"#!/usr/bin/python3
import json,os,sys,time
open(os.environ['VEGA_HC_ARGS'],'w').write('\n'.join(sys.argv[1:])+'\n')
r=json.loads(sys.stdin.readline())
open(os.environ['VEGA_HC_REQUEST'],'w').write(json.dumps(r))
c=os.environ['VEGA_HC_TEST']
t={{"address":"t7","state":"live","pid":{pid},"labels":{{}},"argv":[],"cwd":"/shell/vega","created_ms":0,"screen":"exact","cols":80,"rows":24}}
if c=='hc_foreground': t.update(pid=0,foreground={{"pid":{pid},"name":"codex","argv":["codex"],"cwd":"/project"}})
elif c=='hc_agent_cwd': t.update(foreground={{"pid":{pid},"name":"claude","argv":["claude"],"cwd":"/agent/rally-coach"}},status={{"app":"claude-code","state":"idle"}})
elif c=='hc_agent_launch_cwd': t.update(foreground=None,status={{"app":"claude-code","state":"idle"}})
elif c=='stale': t['pid']=4294967295
elif c=='finished': t['state']='finished'
elif c=='wrong_address': t['address']='t8'
elif c=='bad': print('nope');sys.exit()
elif c=='exit': sys.exit(4)
elif c=='timeout': time.sleep(5)
elif c=='on': t['address']='remote/t7'
elif c=='send': t={{'turn':1}}
elif c not in ['hc']:
 print(json.dumps({{"jsonrpc":"2.0","id":r['id'],"error":{{"code":-32000,"message":"probe","data":{{"kind":c,"hint":"try again"}}}}}}));sys.exit(3)
print(json.dumps({{"jsonrpc":"2.0","id":r['id'],"result":t}}))
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
        "send",
        "usage",
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
        let request = dir.path().join("request");
        let _ = fs::remove_file(&args);
        let mut cmd = Command::new(env::current_exe().unwrap());
        cmd.args(["--exact", "fake_hc_contract", "--nocapture"])
            .env("VEGA_HC_TEST", case)
            .env("VEGA_HC_ARGS", &args)
            .env("VEGA_HC_REQUEST", &request)
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
            assert_eq!(fs::read_to_string(&args).unwrap(), "connect\n");
        }
        if case == "hc" {
            assert_eq!(fs::read_to_string(&args).unwrap(), "connect\n");
        }
        if let Some(method) = match case {
            "hc" => Some("terminal.get"),
            "on" => Some("terminal.run"),
            "send" => Some("terminal.send"),
            _ => None,
        } {
            let request: serde_json::Value =
                serde_json::from_slice(&fs::read(&request).unwrap()).unwrap();
            assert_eq!(request["method"], method);
            assert_eq!(
                request["params"]["caller"], "t7",
                "{method} must preserve caller identity"
            );
        }
        if ["herdr", "stale_herdr", "neither", "usage"].contains(&case) {
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
            _ => assert_eq!(result.unwrap_err().code, "hc_bad_output"),
        }
        return;
    }
    let d = tempfile::tempdir().unwrap();
    for (name,text) in [("hc",r#"#!/usr/bin/python3
import json,os,sys
r=json.loads(sys.stdin.readline()); c=os.environ['VEGA_FLEET_TEST']
if c=='failed':sys.exit(1)
v={'machines':[]} if c=='empty' else {} if c=='bad' else {'machines':[{'name':'corp','target':'user@corp:22','host':'corp','via':False,'state':'connected','id':'id','version':'0.1.18'}]}
print(json.dumps({'jsonrpc':'2.0','id':r['id'],'result':v}))
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
fn rpc_keeps_stdin_open_and_bounds_process_cleanup() {
    if let Ok(case) = env::var("VEGA_HC_PIPE_TEST") {
        let start = std::time::Instant::now();
        let result = if case == "unbounded" {
            hc_local(&["exec", "--", "true"])
        } else {
            vega_herdr::hc::rpc(
                "machine.exec",
                serde_json::json!({"argv":["true"]}),
                // Generous under load (python startup); the fake sleeps 30 s, so
                // timeout cases still prove the deadline and the kill.
                if case == "bounded" {
                    Duration::from_secs(8)
                } else {
                    Duration::from_secs(3)
                },
            )
        };
        if case == "timeout" || case == "descendant" {
            assert_eq!(result.unwrap_err().code, "hc_timeout");
        } else {
            let result = result.unwrap();
            assert_eq!(result["code"], 0);
            assert_eq!(result["stdout"], "");
        }
        assert!(start.elapsed() < Duration::from_secs(10));
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let fake = dir.path().join("hc");
    fs::write(&fake, r#"#!/usr/bin/python3
import json,os,select,sys,time
assert sys.argv[1:] == ['connect']
r=json.loads(sys.stdin.readline())
assert r['method']=='machine.exec' and r['params']=={'argv':['true']}
case=os.environ['VEGA_HC_PIPE_TEST']
if case=='descendant':
 if os.fork(): os._exit(0)
if case in ('timeout','descendant'):
 open(os.environ['VEGA_HC_PID'],'w').write(str(os.getpid()))
 time.sleep(30)
 sys.exit(1)
# Real hc drops a pending reply when its request stream reaches EOF.
if select.select([sys.stdin],[],[],1.0)[0] and sys.stdin.readline()=='':
 sys.exit(0)
open(os.environ['VEGA_HC_PID'],'w').write(str(os.getpid()))
print(json.dumps({'jsonrpc':'2.0','id':r['id'],'result':{'code':0,'signal':None,'stdout':'','stderr':'','ms':150,'truncated':False,'timed_out':False}}),flush=True)
# The caller must finish on the reply, even if the process stays alive.
time.sleep(30)
"#).unwrap();
    fs::set_permissions(&fake, fs::Permissions::from_mode(0o755)).unwrap();
    for case in ["bounded", "unbounded", "timeout", "descendant"] {
        let pid_file = dir.path().join(case);
        let out = Command::new(env::current_exe().unwrap())
            .args([
                "--exact",
                "rpc_keeps_stdin_open_and_bounds_process_cleanup",
                "--nocapture",
            ])
            .env_clear()
            .env("VEGA_HC_PIPE_TEST", case)
            .env("VEGA_HC_PID", &pid_file)
            .env("PATH", format!("{}:/usr/bin:/bin", dir.path().display()))
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{case}: {}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        let pid = fs::read_to_string(pid_file).unwrap();
        // A killed orphan can briefly remain as a zombie until init reaps it.
        for attempt in 0..50 {
            let state = Command::new("ps")
                .args(["-o", "stat=", "-p", pid.trim()])
                .output()
                .unwrap();
            let state = String::from_utf8_lossy(&state.stdout);
            if state.trim().is_empty() || state.trim().starts_with('Z') {
                break;
            }
            assert!(
                attempt < 49,
                "{case}: hc process {pid} survived cleanup ({state})"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
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
            if case == "timeout" {
                Duration::from_millis(200)
            } else {
                Duration::from_secs(3)
            },
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
IFS= read -r request
printf '%s\n' "$request" > "$VEGA_HC_ARGS"
case "$VEGA_HC_RPC_TEST" in
ok) echo '{"jsonrpc":"2.0","id":1,"result":{"address":"remote/t7","labels":{},"argv":[],"cwd":"/fixture","state":"live","created_ms":0,"screen":"exact","cols":80,"rows":24}}' ;;
error) echo '{"jsonrpc":"2.0","id":1,"error":{"message":"failed","data":{"kind":"spawn_failed","hint":"check cwd"}}}' ;;
timeout) sleep 5 ;;
wrong_id) echo '{"jsonrpc":"2.0","id":2,"result":{}}' ;;
missing_field) echo '{"jsonrpc":"2.0","id":1,"result":{"address":"t7"}}' ;;
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
        "missing_field",
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

// Both inventory and individual reads preserve full RPC Terminal metadata.
#[test]
fn terminal_list_contract_preserves_full_metadata() {
    use serde_json::{json, Value};
    if env::var_os("VEGA_HC_LIST_TEST").is_some() {
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
    let terminal = json!({"created_ms":0,"screen":"exact","cols":80,"rows":24,"address":"t34", "name":"coach", "parent":"t2",
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
    ] {
        fs::write(dir.path().join(name), value.to_string()).unwrap();
    }
    let fake = dir.path().join("hc");
    fs::write(
        &fake,
        r#"#!/bin/sh
case "$1 $2" in
 'connect ')
   read -r request
   printf '%s' "$request" > "$VEGA_HC_ARGS"
   case "$request" in
    *'"method":"terminal.get"'*) printf '{"jsonrpc":"2.0","id":1,"result":'; cat "$VEGA_HC_LIST_TEST/terminal"; printf '}' ;;
    *) cat "$VEGA_HC_LIST_TEST/list" ;;
   esac ;;
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
        .env_remove("HC_TERMINAL_ID")
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
        json!({"jsonrpc":"2.0","id":1,"method":"terminal.get","params":{"terminal":"t34"}})
    );
}

/// hc 0.1.18 and 0.1.19 omit an empty `machines` from terminal.list (io, deimos: no
/// machines added); vega reads it as `[]` instead of rejecting the listing.
#[test]
fn terminal_list_without_machines_reads_as_empty() {
    if env::var("VEGA_HC_NO_MACHINES").is_ok() {
        let list = vega_herdr::hc::terminal_list().unwrap();
        assert_eq!(list["machines"], serde_json::json!([]));
        assert_eq!(list["terminals"][0]["address"], "t3");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let fake = dir.path().join("hc");
    fs::write(&fake, r#"#!/bin/sh
[ "$1" = connect ] || exit 1
IFS= read -r request
echo '{"jsonrpc":"2.0","id":1,"result":{"hidden_finished":0,"terminals":[{"address":"t3","labels":{},"argv":[],"cwd":"/fixture","state":"live","created_ms":0,"screen":"exact","cols":80,"rows":24}]}}'
"#).unwrap();
    fs::set_permissions(&fake, fs::Permissions::from_mode(0o755)).unwrap();
    let out = Command::new(env::current_exe().unwrap())
        .args([
            "--exact",
            "terminal_list_without_machines_reads_as_empty",
            "--nocapture",
        ])
        .env("VEGA_HC_NO_MACHINES", "1")
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
