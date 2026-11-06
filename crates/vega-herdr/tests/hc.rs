use std::{env, fs, os::unix::fs::PermissionsExt, process::Command, time::Duration};
use vega_herdr::{hc_local, hc_on, own_terminal, Terminal};

#[test]
fn fake_hc_contract() {
    if let Ok(case) = env::var("VEGA_HC_TEST") {
        match case.as_str() {
            "hc" | "hc_foreground" => {
                assert_eq!(own_terminal().unwrap(), Terminal::Hc("t7".into()))
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
