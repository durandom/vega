use std::process::Command;

#[test]
fn host_resolution_uses_process_then_pinned_file_then_hostname() {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let home = root.path();
    let bin = home.join("bin");
    fs::create_dir_all(&bin).unwrap();
    fs::write(bin.join("hostname"), "#!/bin/sh\necho IO-WSL\n").unwrap();
    fs::set_permissions(bin.join("hostname"), fs::Permissions::from_mode(0o755)).unwrap();
    let state = home.join(".local/state/vega");
    fs::create_dir_all(&state).unwrap();
    let pinned = state.join("vegad.env");
    let run = |process: Option<&str>, xdg: Option<&std::path::Path>, expected: &str| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_vega-post"));
        command.env_clear().env("HOME", home).env("PATH", &bin);
        if let Some(value) = process {
            command.env("VEGA_POST_HOST", value);
        }
        if let Some(path) = xdg {
            command.env("XDG_STATE_HOME", path);
        }
        let output = command.output().unwrap();
        let text = String::from_utf8(output.stdout).unwrap();
        assert!(
            text.lines().any(|l| l == format!("host: {expected}")),
            "{text}"
        );
    };
    run(None, None, "io-wsl");
    fs::write(
        &pinned,
        "# pinned environment\nbad line\nVEGA_POST_HOST=old\nVEGA_POST_HOST=IO.Registry*>\n",
    )
    .unwrap();
    run(None, None, "io_registry__");
    run(Some(""), None, "io_registry__");
    run(Some("Shell.Host"), None, "shell_host");
    let xdg = home.join("custom-state");
    fs::create_dir_all(xdg.join("vega")).unwrap();
    fs::write(xdg.join("vega/vegad.env"), "VEGA_POST_HOST=xdg-host\n").unwrap();
    run(None, Some(&xdg), "xdg-host");
    fs::write(&pinned, "VEGA_POST_HOST=\n").unwrap();
    run(None, None, "io-wsl");
    fs::write(&pinned, "PATH=/evil\n# VEGA_POST_HOST=ignored\n").unwrap();
    run(None, None, "io-wsl");
    fs::remove_file(&pinned).unwrap();
    fs::create_dir(&pinned).unwrap();
    run(None, None, "io-wsl");
}

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
