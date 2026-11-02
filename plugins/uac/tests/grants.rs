use std::process::{Command, Output};

fn run(root: &std::path::Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_vega-uac"))
        .args(args)
        .env("HOME", root)
        .env_remove("XDG_STATE_HOME")
        .env("VEGA_POST_HOST", "proteus")
        .env("VEGA_POST_INBOX_ONLY", "deimos")
        .output()
        .unwrap()
}

#[test]
fn grant_cli_requires_expiry_and_retains_revocation() {
    let root = tempfile::tempdir().unwrap();
    let call = |args: &[&str]| run(root.path(), args);
    let grant = [
        "grant",
        "deimos:w15W:p2",
        "proteus:w3V:p1",
        "--expires",
        "2h",
    ];
    for args in [
        &grant[..3],
        &[
            "grant",
            "deimos:w15W:p2",
            "proteus:w3V:p1",
            "--expires",
            "0h",
        ],
        &["grant", "deimos:*", "proteus:w3V:p1", "--expires", "1h"],
        &["grant", "deimos:w15W:p2", "other:w3V:p1", "--expires", "1h"],
        &[
            "grant",
            "deimos:w15W:p2",
            "proteus:w3V:p1",
            "--expire",
            "1h",
        ],
        &["revoke", "deimos:w15W:p2", "proteus:w3V:p1", "--unknown"],
    ] {
        assert_eq!(call(args).status.code(), Some(2));
    }
    assert!(String::from_utf8_lossy(&call(&["grants"]).stdout).contains("0 inbox-only"));
    assert!(call(&grant).status.success());
    let list = call(&["grants"]);
    let list = String::from_utf8_lossy(&list.stdout);
    assert!(list.contains("deimos:w15W:p2"));
    assert!(list.contains("live"));
    let revoke = ["revoke", "deimos:w15W:p2", "proteus:w3V:p1"];
    assert!(call(&revoke).status.success());
    assert!(call(&revoke).status.success());
    assert!(String::from_utf8_lossy(&call(&["grants"]).stdout).contains("revoked"));
    assert!(call(&grant).status.success());
    assert!(String::from_utf8_lossy(&call(&["grants"]).stdout).contains("live"));
    assert!(call(&["grant", "--help"]).status.success());
}
