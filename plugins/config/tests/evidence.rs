//! Real CLI, isolated HOME/XDG directories; no installed configuration touched.
use std::fs;
use std::process::Command;

use serde_json::Value;

#[test]
fn successive_applies_retain_redacted_write_evidence() {
    let d = tempfile::tempdir().unwrap();
    let home = d.path().join("home");
    let config = d.path().join("config");
    let state = d.path().join("state");
    fs::create_dir_all(home.join(".claude")).unwrap();
    let src = config.join("vega/config/claude");
    fs::create_dir_all(&src).unwrap();
    fs::write(
        src.join("settings.json"),
        r#"{"outputStyle":"SOURCE_SECRET"}"#,
    )
    .unwrap();
    let target = home.join(".claude/settings.json");
    for phase in ["install", "deploy"] {
        fs::write(&target, r#"{"env":{"TOKEN":"USER_SECRET"}}"#).unwrap();
        let output = Command::new(env!("CARGO_BIN_EXE_vega-config"))
            .args(["apply", "claude.settings"])
            .env("HOME", &home)
            .env("XDG_CONFIG_HOME", &config)
            .env("XDG_STATE_HOME", &state)
            .env("XDG_DATA_HOME", d.path().join("data"))
            .env("VEGA_CONFIG_PHASE", phase)
            .env("PATH", "/usr/bin:/bin")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stdout)
        );
        let text = String::from_utf8(output.stdout).unwrap();
        assert!(text.contains("summary: written 1"));
        assert!(text.contains("/outputStyle"));
        assert!(text.contains("missing_key"));
        for forbidden in ["SOURCE_SECRET", "USER_SECRET", "TOKEN", "/env"] {
            assert!(!text.contains(forbidden));
        }
    }
    let text = fs::read_to_string(state.join("vega/config-evidence.json")).unwrap();
    let events: Vec<Value> = serde_json::from_str(&text).unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0]["phase"], "install");
    assert_eq!(events[1]["phase"], "deploy");
    assert_eq!(events[1]["writes"][0]["keys"][0]["source_changed"], false);
    assert!(events.iter().all(|e| e["at_unix_ms"].as_u64().unwrap() > 0));
    for forbidden in ["SOURCE_SECRET", "USER_SECRET", "TOKEN", "/env"] {
        assert!(!text.contains(forbidden));
    }
}
