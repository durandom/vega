use std::process::Command;

#[test]
fn home_names_an_unconfigured_broker_before_reporting_missing_herdr() {
    let root = tempfile::tempdir().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_vega-post"))
        .env_clear()
        .env("HOME", root.path())
        .env("PATH", root.path().join("no-binaries"))
        .env("XDG_CONFIG_HOME", root.path().join("config"))
        .env("XDG_STATE_HOME", root.path().join("state"))
        .env("VEGA_POST_HOST", "testhost")
        .output()
        .unwrap();
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.lines().any(|l| l == "broker: unconfigured"), "{text}");
}
