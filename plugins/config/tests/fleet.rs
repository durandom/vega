//! Explicit fleet diagnostics must never make ordinary infra depend on the network.
use std::{fs, os::unix::fs::PermissionsExt, process::Command};

#[test]
fn fleet_is_explicit_and_empty_inventory_needs_no_registry() {
    let home = tempfile::tempdir().unwrap();
    let bin = home.path().join("bin");
    fs::create_dir(&bin).unwrap();
    let hc = bin.join("hc");
    fs::write(
        &hc,
        r#"#!/bin/sh
echo called >> "$HOME/hc-calls"
read -r request
printf '{"jsonrpc":"2.0","id":1,"result":%s}' "$TEST_FLEET"
"#,
    )
    .unwrap();
    fs::set_permissions(&hc, fs::Permissions::from_mode(0o755)).unwrap();
    let run = |fleet: bool, inventory: &str| {
        let mut c = Command::new(env!("CARGO_BIN_EXE_vega-config"));
        c.env_clear()
            .env("HOME", home.path())
            .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
            .env("TEST_FLEET", inventory)
            .arg("infra");
        if fleet {
            c.arg("--fleet");
        }
        c.output().unwrap()
    };
    assert!(run(false, "not-json").status.success());
    assert!(!home.path().join("hc-calls").exists());
    let empty = run(true, r#"{"machines":[]}"#);
    assert!(
        empty.status.success(),
        "{}",
        String::from_utf8_lossy(&empty.stdout)
    );
    assert!(String::from_utf8_lossy(&empty.stdout).contains("0 hc machines configured"));
    let malformed = run(true, "{}");
    assert!(!malformed.status.success());
    assert!(String::from_utf8_lossy(&malformed.stdout).contains("hc_bad_output"));
    let unavailable = run(
        true,
        r#"{"machines":[{"name":"remote","target":"remote","id":"id","host":"remote","version":"0.1.18","state":"unreachable"}]}"#,
    );
    assert!(!unavailable.status.success());
    assert!(String::from_utf8_lossy(&unavailable.stdout).contains("registry_unavailable"));
}
