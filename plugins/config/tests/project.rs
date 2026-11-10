use serde_json::{json, Value};
use std::{fs, process::Command};

#[test]
fn project_cli_uses_private_home_and_xdg_config_and_reports_failures() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("home");
    let config = root.path().join("config");
    let call = |args: &[&str], xdg: bool| {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_vega-project"));
        cmd.env("HOME", &home).env_remove("XDG_CONFIG_HOME");
        if xdg {
            cmd.env("XDG_CONFIG_HOME", &config);
        }
        cmd.args(args).output().unwrap()
    };
    let id = "github.com/team/repo";
    for xdg in [true, false] {
        let absent = call(&["config", id, "--json"], xdg);
        assert_eq!(
            serde_json::from_slice::<Value>(&absent.stdout).unwrap()["projects"][id],
            json!({"name":"","labels":[],"pinned":false})
        );
        let path = if xdg {
            config.clone()
        } else {
            home.join(".config")
        }
        .join("vega/projects.toml");
        assert!(
            !path.exists(),
            "reading an absent entry must not create config"
        );
        let result = call(
            &[
                "config",
                id,
                "--name",
                "My project",
                "--label",
                "work",
                "--label",
                "work",
                "--json",
            ],
            xdg,
        );
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stdout)
        );
        let out: Value = serde_json::from_slice(&result.stdout).unwrap();
        assert_eq!(
            out["projects"][id],
            json!({"name":"My project","labels":["work"],"pinned":false})
        );
        let path = if xdg {
            config.clone()
        } else {
            home.join(".config")
        }
        .join("vega/projects.toml");
        assert!(path.exists());
        assert!(call(
            &[
                "view",
                "Work",
                "--label",
                "work",
                "--machine",
                "puck",
                "--json"
            ],
            xdg
        )
        .status
        .success());
        let out = call(&["config", "--json"], xdg);
        let data: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(
            data["views"]["Work"],
            json!({"labels":["work"],"machines":["puck"],"match":"all"})
        );
        assert!(call(&["view", "Work", "--delete", "--json"], xdg)
            .status
            .success());
        let after_delete = call(&["config", "--json"], xdg);
        let after_delete: Value = serde_json::from_slice(&after_delete.stdout).unwrap();
        assert!(after_delete["views"].get("Work").is_none());
        assert_eq!(after_delete["projects"][id]["name"], "My project");
        assert!(call(&["view", "Work", "--delete", "--json"], xdg)
            .status
            .success());
        fs::write(&path, "broken = [").unwrap();
        assert_eq!(
            call(&["config", id, "--name", "new", "--json"], xdg)
                .status
                .code(),
            Some(1)
        );
        assert_eq!(fs::read_to_string(path).unwrap(), "broken = [");
    }
    let bad = call(&["config", "--typo"], true);
    assert_eq!(bad.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&bad.stdout).contains("--typo"));
    assert!(bad.stderr.is_empty());
}

#[test]
fn view_filters_replace_both_dimensions_and_label_removal_wins() {
    let root = tempfile::tempdir().unwrap();
    let call = |args: &[&str]| {
        let out = Command::new(env!("CARGO_BIN_EXE_vega-project"))
            .env("HOME", root.path().join("home"))
            .env("XDG_CONFIG_HOME", root.path().join("config"))
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stdout)
        );
        serde_json::from_slice::<Value>(&out.stdout).unwrap()
    };
    let id = "github.com/team/a";
    let project = call(&[
        "config",
        id,
        "--label",
        "work",
        "--label",
        "private",
        "--unlabel",
        "work",
        "--json",
    ]);
    assert_eq!(project["projects"][id]["labels"], json!(["private"]));
    call(&[
        "view",
        "Focus",
        "--label",
        "work",
        "--machine",
        "puck",
        "--json",
    ]);
    let replaced = call(&["view", "Focus", "--machine", "local", "--json"]);
    assert_eq!(
        replaced["views"]["Focus"],
        json!({"labels":[],"machines":["local"],"match":"all"})
    );
}

#[test]
fn reserved_and_blank_inputs_are_rejected_without_changing_preferences() {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("home");
    let config = root.path().join("config");
    let call = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_vega-project"))
            .env("HOME", &home)
            .env("XDG_CONFIG_HOME", &config)
            .args(args)
            .output()
            .unwrap()
    };
    assert!(
        call(&["config", "github.com/team/a", "--name", "Keep", "--json"])
            .status
            .success()
    );
    let path = config.join("vega/projects.toml");
    let original = fs::read_to_string(&path).unwrap();
    for args in [
        vec!["view", "all", "--label", "work", "--json"],
        vec!["view", "", "--label", "work", "--json"],
        vec!["config", "   ", "--name", "bad", "--json"],
        vec!["config", "github.com/team/a", "--label", "  ", "--json"],
        vec!["config", "github.com/team/a", "--unlabel", "  ", "--json"],
        vec!["view", "Work", "--machine", "  ", "--json"],
    ] {
        let out = call(&args);
        assert_eq!(out.status.code(), Some(1), "{args:?}");
        assert!(String::from_utf8_lossy(&out.stdout).contains("error"));
        assert_eq!(fs::read_to_string(&path).unwrap(), original, "{args:?}");
    }
}
