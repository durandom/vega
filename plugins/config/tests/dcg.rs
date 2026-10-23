//! Exercise dcg's real decision pipeline when installed, with an isolated HOME.
use std::{fs, path::Path, process::Command};
use vega_config::sources;

#[test]
fn explain_keeps_core_blocks_and_shows_prune_hint() {
    let Some(dcg) = vega_plugin::find_on_path("dcg") else {
        eprintln!("dcg not installed; skipping external explain fixture");
        return;
    };
    let d = tempfile::tempdir().unwrap();
    let config_dir = d.path().join(".config/dcg");
    fs::create_dir_all(config_dir.join("packs")).unwrap();
    fs::write(
        config_dir.join("packs/oc.yaml"),
        sources::default("dcg/packs/oc.yaml").unwrap(),
    )
    .unwrap();
    // Explicit configs live outside the default user-config path so they cannot
    // merge with each other. The baseline preserves the original core decisions.
    let config = d.path().join("managed.toml");
    let text = sources::default("dcg/config.toml")
        .unwrap()
        .replace("self_heal_hook = true", "self_heal_hook = false");
    fs::write(&config, &text).unwrap();
    let baseline = d.path().join("baseline.toml");
    let mut original = text.parse::<toml_edit::DocumentMut>().unwrap();
    original["overrides"]["block"] = toml_edit::value(toml_edit::Array::new());
    fs::write(&baseline, original.to_string()).unwrap();
    let explain = |config: &Path, command: &str| -> serde_json::Value {
        let output = Command::new(&dcg)
            .args(["explain", command, "--format", "json"])
            .current_dir(d.path())
            .env("HOME", d.path())
            .env("XDG_CONFIG_HOME", d.path().join(".config"))
            .env("XDG_DATA_HOME", d.path().join(".local/share"))
            .env("DCG_CONFIG", config)
            .output()
            .unwrap();
        let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert!(result["decision"].is_string(), "{command}: {result}");
        result
    };
    let cases: Vec<serde_json::Value> =
        serde_json::from_str(include_str!("fixtures/dcg-explain.json")).unwrap();
    for case in cases {
        let command = case["command"].as_str().unwrap();
        let result = explain(&config, command);
        let expected = if case["decision"] == "baseline" {
            explain(&baseline, command)["decision"].clone()
        } else {
            case["decision"].clone()
        };
        assert_eq!(result["decision"], expected, "{case}: {result}");
        let reason = result["match"]["reason"].as_str().unwrap_or_default();
        let hint = reason.contains("vega uac prune") && reason.contains("wt remove");
        assert_eq!(hint, case["hint"].as_bool().unwrap(), "{case}: {result}");
        if hint {
            let source = case
                .get("source")
                .and_then(|s| s.as_str())
                .unwrap_or("config_override");
            assert_eq!(result["match"]["source"], source, "{case}: {result}");
        }
    }
}
