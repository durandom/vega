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
            json!({"name":"My project","labels":["work"]})
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
            json!({"labels":["work"],"machines":["puck"]})
        );
        assert!(call(&["view", "Work", "--delete", "--json"], xdg)
            .status
            .success());
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
