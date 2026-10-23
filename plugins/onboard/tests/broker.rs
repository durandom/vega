use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::process::Command;

#[test]
fn home_uses_endpoint_configuration_instead_of_tailscale_for_vegad() {
    let root = tempfile::tempdir().unwrap();
    let bin = root.path().join("bin");
    let config = root.path().join("config/vega");
    let data = root.path().join("data/vega/herdr-plugin");
    fs::create_dir_all(&bin).unwrap();
    fs::create_dir_all(&config).unwrap();
    fs::create_dir_all(&data).unwrap();
    fs::write(data.join("herdr-plugin.toml"), "").unwrap();
    let tailscale = bin.join("tailscale");
    fs::write(&tailscale, "#!/bin/sh\nexit 0\n").unwrap();
    fs::set_permissions(&tailscale, fs::Permissions::from_mode(0o755)).unwrap();
    let line = || {
        let output = Command::new(env!("CARGO_BIN_EXE_vega-onboard"))
            .env_clear()
            .env("PATH", &bin)
            .env("HOME", root.path())
            .env("XDG_CONFIG_HOME", root.path().join("config"))
            .env("XDG_STATE_HOME", root.path().join("state"))
            .env("XDG_DATA_HOME", root.path().join("data"))
            .env("VEGA_POST_HOST", "testhost")
            .output()
            .unwrap();
        let text = String::from_utf8(output.stdout).unwrap();
        text.lines()
            .find(|l| l.starts_with("vegad:"))
            .expect(&text)
            .to_string()
    };
    let no_endpoint = line();
    assert!(no_endpoint.contains("local mode"), "{no_endpoint}");
    assert!(no_endpoint.contains("identity migration"), "{no_endpoint}");
    fs::remove_file(tailscale).unwrap();
    fs::write(config.join("nats.url"), "nats://broker.example:4222\n").unwrap();
    assert_eq!(line(), "vegad: installed");
}
