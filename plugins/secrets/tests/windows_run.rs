#![cfg(windows)]

use std::process::Command;

use vega_secrets::{Entry, Manifest, Secret, Store};

#[test]
fn run_injects_the_scope_and_forwards_the_windows_exit_code() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store {
        config: dir.path().join("vega"),
        state: dir.path().join("state"),
        host: "test-win".into(),
    };
    let (recipient, _) = store.host_init().unwrap();
    let mut manifest = Manifest::default();
    manifest.set_host(&store.host, &recipient);
    let mut entry = Entry {
        name: "fixture".into(),
        env: "VEGA_TEST_SECRET".into(),
        tools: vec!["fixture-scope".into()],
        hosts: vec![store.host.clone()],
        ..Entry::default()
    };
    entry.encrypted_to = store
        .encrypt(&manifest, &entry, &Secret::new("fixture-value".into()))
        .unwrap();
    manifest.set_entry(&entry);
    store.save_manifest(&manifest).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_vega-secrets"))
        .env("VEGA_POST_HOST", &store.host)
        .env("XDG_CONFIG_HOME", dir.path())
        .env("XDG_STATE_HOME", &store.state)
        .env_remove("VEGA_TEST_SECRET")
        .args([
            "run",
            "--for",
            "fixture-scope",
            "--",
            "powershell.exe",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "if ($env:VEGA_TEST_SECRET.Length -eq 13) { exit 301 }; exit 1",
        ])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(301));
    assert!(output.stdout.is_empty());
    assert!(output.stderr.is_empty());
}
