//! Database validation through the init command, before any remote mutation.
use std::process::Command;

#[test]
fn adopt_accepts_hyphens_and_rejects_unsafe_database_names() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    assert!(Command::new("git")
        .args(["init", "-q"])
        .current_dir(&repo)
        .status()
        .unwrap()
        .success());
    let run = |database: &str, remote: &str| {
        Command::new(env!("CARGO_BIN_EXE_vega-bd"))
            .args(["init", "--adopt", "--database", database])
            .current_dir(&repo)
            .env("HOME", tmp.path())
            .env("VEGA_BD_REMOTE", remote)
            .env("XDG_CONFIG_HOME", tmp.path().join("config"))
            .env("XDG_STATE_HOME", tmp.path().join("state"))
            // No existing local beads: init must stop before probing the remote.
            .env("BD_REAL", "/usr/bin/false")
            .output()
            .unwrap()
    };
    let o = run("oparl-lite-2", "http://dolt.example:8080");
    let out = String::from_utf8_lossy(&o.stdout);
    assert_eq!(o.status.code(), Some(1), "{out}");
    assert!(out.contains("nothing_to_adopt"), "{out}");
    let o = run("oparl-lite-2", "");
    assert!(String::from_utf8_lossy(&o.stdout).contains("remote_unconfigured"));
    let config = tmp.path().join("config/vega");
    std::fs::create_dir_all(&config).unwrap();
    std::fs::write(config.join("bd.remote"), "http://dolt.example:8080\n").unwrap();
    let o = run("oparl-lite-2", "");
    assert!(String::from_utf8_lossy(&o.stdout).contains("nothing_to_adopt"));
    for name in ["a/b", "a`b", "a?b", ""] {
        let o = run(name, "http://dolt.example:8080");
        let out = String::from_utf8_lossy(&o.stdout);
        assert!(out.contains("usage"), "{name:?}: {out}");
        assert!(out.contains("underscores and hyphens"), "{out}");
    }
}

#[test]
fn init_missing_sql_endpoint_stops_before_credentials_or_sql() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    let bin = tmp.path().join("bin");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::create_dir_all(&bin).unwrap();
    assert!(Command::new("git")
        .args(["init", "-q"])
        .current_dir(&repo)
        .status()
        .unwrap()
        .success());
    let dolt = bin.join("dolt");
    // The remote probe reports a missing database. SQL must never be invoked.
    std::fs::write(&dolt, "#!/bin/sh\ncase \"$1\" in\ninit|remote) exit 0;;\nfetch) echo \"$*\" >\"$FETCH_ARGS\"; echo 'database not found'; exit 1;;\n*) touch \"$SQL_CALLED\"; exit 1;;\nesac\n").unwrap();
    std::fs::set_permissions(&dolt, std::fs::Permissions::from_mode(0o755)).unwrap();
    let path = std::env::join_paths(
        std::iter::once(bin).chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_vega-bd"))
        .args(["init", "--database", "TEST"])
        .current_dir(&repo)
        .env("HOME", tmp.path())
        .env("XDG_CONFIG_HOME", tmp.path().join("config"))
        .env("XDG_STATE_HOME", tmp.path().join("state"))
        .env("PATH", path)
        .env("BD_REAL", "/usr/bin/false")
        .env("VEGA_BD_REMOTE", "http://dolt.example:8080")
        .env_remove("VEGA_BD_SQL")
        .env("SQL_CALLED", tmp.path().join("sql-called"))
        .env("FETCH_ARGS", tmp.path().join("fetch-args"))
        .env("DOLT_REMOTE_USER", "goern")
        .output()
        .unwrap();
    let out = String::from_utf8_lossy(&output.stdout);
    assert_eq!(output.status.code(), Some(1), "{out}");
    assert!(out.contains("sql_unconfigured"), "{out}");
    assert!(!tmp.path().join("sql-called").exists());
    // The probe authenticates as DOLT_REMOTE_USER (dolt itself would default to root).
    let fetch = std::fs::read_to_string(tmp.path().join("fetch-args")).unwrap();
    assert!(fetch.starts_with("fetch --user goern probe "), "{fetch}");
    assert!(!tmp.path().join("config/vega/secrets.toml").exists());
}
