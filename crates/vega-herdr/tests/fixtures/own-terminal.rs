// Shared subprocess fixture: no real hc core, herdr server, broker, or user state.
use std::{fs, os::unix::fs::PermissionsExt, process::Command};
pub fn fixture(binary: &str) -> (tempfile::TempDir, Command) {
    let dir = tempfile::tempdir().unwrap();
    let bin = dir.path().join("bin");
    fs::create_dir_all(&bin).unwrap();
    fs::write(bin.join("hc"), r#"#!/bin/sh
[ "$1 $2 $3" = '--json get t7' ] || exit 1
printf '{"state":"live","address":"t7","pid":%s,"name":"child","parent":"t2","status":{"app":"claude-code"},"foreground":{"cwd":"/project"}}\n' "$TEST_PARENT_PID"
"#).unwrap();
    fs::set_permissions(bin.join("hc"), fs::Permissions::from_mode(0o755)).unwrap();
    let mut c = Command::new(binary);
    c.env_clear()
        .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
        .env("HOME", dir.path())
        .env("XDG_STATE_HOME", dir.path().join("state"))
        .env("XDG_CONFIG_HOME", dir.path().join("config"))
        .env("VEGA_POST_HOST", "worker")
        .env("VEGA_NATS_URL", "nats://127.0.0.1:1")
        .env("HC_TERMINAL_ID", "t7")
        .env("CODEX_THREAD_ID", "session-7")
        .env("TEST_PARENT_PID", std::process::id().to_string());
    (dir, c)
}
