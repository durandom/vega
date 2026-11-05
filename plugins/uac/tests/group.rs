//! Exercise the actual CLI with a local-only herdr and an unreachable broker.
use serde_json::json;
use std::{fs, os::unix::fs::PermissionsExt, path::Path, process::Command};

fn repo(path: &Path, name: &str) {
    fs::create_dir_all(path).unwrap();
    for args in [
        vec!["init", "-q"],
        vec![
            "remote",
            "add",
            "origin",
            &format!("https://github.com/team/{name}.git"),
        ],
    ] {
        assert!(Command::new("git")
            .arg("-C")
            .arg(path)
            .args(args)
            .status()
            .unwrap()
            .success());
    }
}

#[test]
fn group_cli_claims_members_and_lists_only_group_agents_with_project() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path();
    for name in ["coord", "service", "web", "unrelated"] {
        repo(&home.join(name), name);
    }
    let bin = home.join("bin");
    fs::create_dir_all(&bin).unwrap();
    fs::write(
        bin.join("herdr"),
        r#"#!/bin/sh
case "$1 $2" in
  'pane list') cat "$HOME/panes.json" ;;
  'pane process-info') printf '{"result":{"process_info":{"shell_pid":%s}}}' "$FAKE_PID" ;;
  'machine list') echo '[]' ;;
  'agent list') cat "$HOME/agents.json" ;;
  'workspace list') echo '{"result":{"workspaces":[]}}' ;;
  'tab list') echo '{"result":{"tabs":[]}}' ;;
  *) echo '{"result":{}}' ;;
esac
"#,
    )
    .unwrap();
    fs::set_permissions(bin.join("herdr"), fs::Permissions::from_mode(0o755)).unwrap();
    fs::write(
        home.join("panes.json"),
        json!({"result":{"panes":[{"pane_id":"uac","agent":"codex","cwd":home.join("coord")}]}})
            .to_string(),
    )
    .unwrap();
    let agents: Vec<_> = ["coord", "service", "web", "unrelated"].into_iter().map(|name| json!({"pane_id":if name == "coord" {"uac"} else {name},"cwd":home.join(name),"name":name,"agent_status":"idle"})).collect();
    fs::write(
        home.join("agents.json"),
        json!({"result":{"agents":agents}}).to_string(),
    )
    .unwrap();
    let call = |args: &[&str]| {
        let out = Command::new(env!("CARGO_BIN_EXE_vega-uac"))
            .args(args)
            .current_dir(home.join("coord"))
            .env_clear()
            .env("HOME", home)
            .env("XDG_STATE_HOME", home.join("state"))
            .env("XDG_CONFIG_HOME", home.join("config"))
            .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
            .env("HERDR_PANE_ID", "uac")
            .env("FAKE_PID", std::process::id().to_string())
            .env("VEGA_POST_HOST", "fixture")
            .env("VEGA_POST_INBOX_ONLY", "fixture")
            .env("VEGA_NATS_URL", "nats://127.0.0.1:9")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .unwrap();
        let text = String::from_utf8_lossy(&out.stdout).into_owned();
        assert!(
            out.status.success(),
            "{args:?}: {text} {}",
            String::from_utf8_lossy(&out.stderr)
        );
        text
    };
    assert!(call(&["group"]).contains("members[0]"));
    let bad = Command::new(env!("CARGO_BIN_EXE_vega-uac"))
        .args(["group", "--oops"])
        .output()
        .unwrap();
    assert_eq!(bad.status.code(), Some(2));
    call(&["group", "add", "../service"]);
    call(&["group", "add", "github.com/team/web"]);
    call(&["claim"]);
    for name in ["coord", "service", "web"] {
        let id = format!("github.com/team/{name}");
        let file = home.join("state/vega/uac/v2").join(format!(
            "{}.json",
            vega_herdr::project::encode_component(&id)
        ));
        let record: serde_json::Value = serde_json::from_slice(&fs::read(file).unwrap()).unwrap();
        assert_eq!(record["project"], id);
        assert_eq!(record["pane"], "uac");
    }
    let listing = call(&[]);
    assert!(listing.contains("agents[2]{project,machine"), "{listing}");
    assert!(
        listing.contains("github.com/team/service,fixture,service"),
        "{listing}"
    );
    assert!(
        listing.contains("github.com/team/web,fixture,web"),
        "{listing}"
    );
    assert!(!listing.contains("unrelated"), "{listing}");
    // Release must remain possible even if the manifest has become malformed.
    fs::write(home.join("coord/.vega/group.toml"), "broken = [").unwrap();
    call(&["release"]);
    assert_eq!(
        fs::read_dir(home.join("state/vega/uac/v2"))
            .unwrap()
            .count(),
        0
    );
}
