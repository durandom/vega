//! `vega post notes` on an inbox-only host skips the registry read.
use std::process::Command;

#[test]
fn notes_list_says_the_registry_is_off_by_design() {
    let home = tempfile::tempdir().unwrap();
    let start = std::time::Instant::now();
    let out = Command::new(env!("CARGO_BIN_EXE_vega-post"))
        .arg("notes")
        .current_dir(home.path())
        .env_clear()
        .env("HOME", home.path())
        .env("XDG_STATE_HOME", home.path().join("state"))
        .env("XDG_CONFIG_HOME", home.path().join("config"))
        .env("PATH", "/usr/bin:/bin")
        .env("VEGA_POST_HOST", "puck")
        .env("VEGA_POST_INBOX_ONLY", "puck")
        .env("VEGA_NATS_URL", "nats://10.255.255.1:4222")
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.contains("registry: off (inbox-only host: local agents only)"),
        "{text}"
    );
    assert!(start.elapsed() < std::time::Duration::from_secs(2));
}
