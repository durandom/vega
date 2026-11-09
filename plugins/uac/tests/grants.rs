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
    let file = std::fs::read_dir(root.path().join(".local/state/vega/mail-grants"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let mut record: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
    record["expires"] = 0.into();
    std::fs::write(&file, serde_json::to_vec(&record).unwrap()).unwrap();
    assert!(String::from_utf8_lossy(&call(&["grants"]).stdout).contains("expired"));
    assert!(call(&grant).status.success());
    let revoke = ["revoke", "deimos:w15W:p2", "proteus:w3V:p1"];
    assert!(call(&revoke).status.success());
    assert!(call(&revoke).status.success());
    assert!(String::from_utf8_lossy(&call(&["grants"]).stdout).contains("revoked"));
    assert!(call(&grant).status.success());
    assert!(String::from_utf8_lossy(&call(&["grants"]).stdout).contains("live"));
    assert!(call(&["grant", "--help"]).status.success());
}

#[test]
fn grant_and_revoke_accept_alphanumeric_pane_ids() {
    let root = tempfile::tempdir().unwrap();
    let grant = [
        "grant",
        "deimos:w1C:pB",
        "proteus:w3V:p1C",
        "--expires",
        "1h",
    ];
    let output = run(root.path(), &grant);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
    let listing = run(root.path(), &["grants"]);
    let listing = String::from_utf8_lossy(&listing.stdout);
    assert!(listing.contains("deimos:w1C:pB"));
    assert!(listing.contains("proteus:w3V:p1C"));
    assert!(
        run(root.path(), &["revoke", "deimos:w1C:pB", "proteus:w3V:p1C"])
            .status
            .success()
    );
}

#[test]
fn grant_and_revoke_accept_hc_terminal_ids() {
    let root = tempfile::tempdir().unwrap();
    let call = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_vega-uac"))
            .args(args)
            .env("HOME", root.path())
            .env_remove("XDG_STATE_HOME")
            .env("VEGA_POST_HOST", "proteus")
            .env("VEGA_POST_INBOX_ONLY", "deimos,puck")
            .output()
            .unwrap()
    };
    let grant = call(&["grant", "puck:t1", "proteus:t6p", "--expires", "7d"]);
    assert!(
        grant.status.success(),
        "{}",
        String::from_utf8_lossy(&grant.stdout)
    );
    let listing = String::from_utf8_lossy(&call(&["grants"]).stdout).to_string();
    assert!(listing.contains("puck:t1,proteus:t6p,live"), "{listing}");
    // Wildcards, names and a non-inbox-only sender stay refused.
    for bad in [
        ["grant", "puck:t*", "proteus:t6p", "--expires", "1h"],
        ["grant", "puck:t1", "proteus:main", "--expires", "1h"],
        ["grant", "proteus:t1", "proteus:t6p", "--expires", "1h"],
    ] {
        assert_eq!(call(&bad).status.code(), Some(2), "{bad:?}");
    }
    assert!(call(&["revoke", "puck:t1", "proteus:t6p"]).status.success());
    assert!(String::from_utf8_lossy(&call(&["grants"]).stdout).contains("revoked"));
}
