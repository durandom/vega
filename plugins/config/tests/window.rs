//! Full CLI exercise, with every hc/app call handled by fakes in a private HOME.
use std::{fs, os::unix::fs::PermissionsExt, process::Command};

#[test]
fn apply_and_open_twice_only_starts_three_terminals_in_the_profile_home() {
    let d = tempfile::tempdir().unwrap();
    let config = d.path().join("config");
    let bin = d.path().join("bin");
    fs::create_dir_all(&bin).unwrap();
    // inline-md: allow (executable test fixture, not guide/help)
    let fake = r#"#!/usr/bin/python3
import json, os, pathlib, sys
root = pathlib.Path(os.environ['HOME'])
a = sys.argv[1:]
with (root/'calls').open('a') as f: f.write(json.dumps([pathlib.Path(sys.argv[0]).name, a, os.environ.get('HERDR_APP_HOME')])+'\n')
state = root/'terminals.json'
terms = json.loads(state.read_text()) if state.exists() else []
if pathlib.Path(sys.argv[0]).name == 'hc':
    if a == ['connect']:
        req = json.loads(sys.stdin.readline())
        assert req['method'] == 'terminal.list'
        print(json.dumps({'jsonrpc':'2.0','id':req['id'],'result':{'terminals':terms,'hidden_finished':0}}))
    elif a[0] == 'exec':
        assert a == ['exec','--on','io','--timeout','10','--','sh','-lc','command -v -- "$1" >/dev/null','vega-window','btop']
        sys.exit(1)
    else:
        assert a[0] == 'run' and '--no-parent' in a and '--bg' in a
        labels = dict(a[i+1].split('=',1) for i,x in enumerate(a) if x == '--label')
        t = dict(address=a[a.index('--on')+1]+'/t'+str(len(terms)+1), labels=labels, argv=a[a.index('--')+1:], cwd='/', state='live', created_ms=0, screen='exact', cols=80, rows=24)
        terms.append(t); state.write_text(json.dumps(terms)); print(json.dumps(t))
else:
    if a and a[0] == 'check':
        assert pathlib.Path(a[1], 'main.js').is_file()
        sys.exit(0)
    assert os.environ['HERDR_APP_HOME'] == str(root/'config/herdr-app-status')
    if a: assert a == ['do','layout.switch','--name','vega-window'] or a == ['status']
    print(json.dumps({'layout':'vega-window','errors':[]}))
"#;
    for name in ["hc", "herdr-app"] {
        let p = bin.join(name);
        fs::write(&p, fake).unwrap();
        fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let command = |exe: &str, args: &[&str]| {
        let out = Command::new(exe)
            .args(args)
            .env("HOME", d.path())
            .env("XDG_CONFIG_HOME", &config)
            .env("XDG_STATE_HOME", d.path().join("state"))
            .env("HERDR_APP_HOME", d.path().join("must-not-touch"))
            .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
            .env_remove("VEGA_CONFIG_PHASE")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap()
    };
    command(
        env!("CARGO_BIN_EXE_vega-config"),
        &["apply", "herdr-app.profiles"],
    );
    let calls = fs::read_to_string(d.path().join("calls")).unwrap();
    for line in calls.lines() {
        let call: serde_json::Value = serde_json::from_str(line).unwrap();
        assert_eq!(call[0], "herdr-app");
        assert_eq!(call[1][0], "check", "apply must only validate staged mods");
    }
    let first = command(env!("CARGO_BIN_EXE_vega-window"), &["status"]);
    assert!(first.contains("started: 3"));
    assert!(first.contains("skipped: 1"));
    assert!(first.contains("skipping io-btop"));
    let layout_path = config.join("herdr-app-status/layouts/vega-window.json");
    let first_layout = fs::read(&layout_path).unwrap();
    let second = command(env!("CARGO_BIN_EXE_vega-window"), &["status"]);
    assert_eq!(fs::read(&layout_path).unwrap(), first_layout);
    assert!(second.contains("started: 0"));
    assert!(second.contains("reused: 3"));
    assert!(!d.path().join("must-not-touch").exists());
    let terms: serde_json::Value =
        serde_json::from_slice(&fs::read(d.path().join("terminals.json")).unwrap()).unwrap();
    assert_eq!(terms.as_array().unwrap().len(), 3);
    assert_eq!(terms[2]["argv"], serde_json::json!(["token-burn", "tui"]));
}
