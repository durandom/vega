//! Retired hosts retain the exact admin/viewer reads captured before retirement.
use std::process::{Command, Output};

fn key() -> String {
    nkeys::KeyPair::new_user().public_key()
}

fn config(host: &str, admin: &str, viewer: &str) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_vega-post"));
    cmd.args(["auth-config", host, "--admin", admin, "--viewer", viewer]);
    cmd
}

fn rendered(out: Output) -> String {
    assert!(out.status.success(), "{out:?}");
    String::from_utf8(out.stdout).unwrap()
}

fn subjects<'a>(out: &'a str, role: &str) -> Vec<&'a str> {
    out.split_once(&format!("# {role}:"))
        .unwrap()
        .1
        .split_once("\n    }")
        .unwrap()
        .0
        .lines()
        .map(str::trim)
        .filter(|l| l.starts_with('"'))
        .map(|l| l.trim_end_matches(',').trim_matches('"'))
        .collect()
}

#[test]
fn retired_deimos_preserves_the_captured_two_admin_and_seven_viewer_subjects() {
    let host = format!("proteus={}", key());
    let deimos_key = key();
    let admin = key();
    let viewer = key();
    let baseline = rendered(
        config(&host, &admin, &viewer)
            .args([&format!("deimos={deimos_key}"), "--inbox-only", "deimos"])
            .output()
            .unwrap(),
    );
    // Captured from today's inbox-only rendering before adding --retired-host.
    let expected_admin = [
        "$JS.API.STREAM.INFO.VEGA_INBOX_deimos",
        "$JS.API.CONSUMER.LIST.VEGA_INBOX_deimos",
    ];
    let expected_viewer = [
        "$JS.API.STREAM.INFO.VEGA_INBOX_deimos",
        "$JS.API.STREAM.MSG.GET.VEGA_INBOX_deimos",
        "$JS.API.DIRECT.GET.VEGA_INBOX_deimos",
        "$JS.API.DIRECT.GET.VEGA_INBOX_deimos.>",
        "$JS.API.CONSUMER.LIST.VEGA_INBOX_deimos",
        "$JS.API.CONSUMER.NAMES.VEGA_INBOX_deimos",
        "$JS.API.CONSUMER.INFO.VEGA_INBOX_deimos.*",
    ];
    let retired = rendered(
        config(&host, &admin, &viewer)
            .args(["--retired-host", "deimos"])
            .output()
            .unwrap(),
    );
    for (role, expected) in [
        ("admin", expected_admin.as_slice()),
        ("viewer", expected_viewer.as_slice()),
    ] {
        let before = subjects(&baseline, role);
        let after = subjects(&retired, role);
        assert_eq!(before, after, "all {role} rights, including shared history");
        assert_eq!(
            after
                .into_iter()
                .filter(|s| s.contains("deimos"))
                .collect::<Vec<_>>(),
            expected,
            "{role} deimos baseline"
        );
    }
    assert_eq!(retired.matches("nkey:").count(), 3);
    assert!(!retired.contains(&deimos_key));
    assert!(!retired.contains("# deimos"));
    assert_eq!(
        retired
            .lines()
            .filter(|l| l.contains("deimos") && l.trim().starts_with('"'))
            .count(),
        9
    );
    assert!(retired.contains("max_file_store must be >= 20 GiB"));
    let repeated = rendered(
        config(&host, &admin, &viewer)
            .args([
                "--retired-host",
                "deimos",
                "--retired-host",
                "old-host",
                "--create",
                "archive",
            ])
            .output()
            .unwrap(),
    );
    assert!(repeated.contains("STREAM.INFO.VEGA_INBOX_old-host"));
    assert!(repeated.contains("STREAM.UPDATE.VEGA_INBOX_proteus"));
    assert!(!repeated.contains("STREAM.UPDATE.VEGA_INBOX_deimos"));
    assert!(!repeated.contains("STREAM.UPDATE.VEGA_INBOX_old-host"));
    assert_eq!(repeated.matches("nkey:").count(), 3);
}

#[test]
fn retired_hosts_reject_active_identities_and_grants() {
    let host = format!("proteus={}", key());
    let admin = key();
    let viewer = key();
    for args in [
        vec![format!("deimos={}", key())],
        vec!["--create".into(), "deimos".into()],
        vec!["--inbox-only".into(), "deimos".into()],
        vec![
            "--cmd-grant".into(),
            "deimos=vega.cmd.proteus.notify.deimos".into(),
        ],
        vec!["--quarantine".into(), "deimos".into()],
        vec!["--registry-read".into(), "deimos=proteus.theme".into()],
        vec!["--lanista-writer".into(), "deimos".into()],
        vec!["--retired-host".into(), "deimos".into()],
    ] {
        let out = config(&host, &admin, &viewer)
            .args(["--retired-host", "deimos"])
            .args(&args)
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(2), "{args:?}: {out:?}");
        assert!(!String::from_utf8_lossy(&out.stdout).contains("authorization {"));
    }
    let file = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(file.path(), format!("deimos={}\n", key())).unwrap();
    let out = config(&host, &admin, &viewer)
        .args(["--retired-host", "deimos", "--file"])
        .arg(file.path())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stdout).contains("host is also active"));
    for invalid in ["", "Deimos", "a.b", "*", "admin", "viewer"] {
        let out = config(&host, &admin, &viewer)
            .args(["--retired-host", invalid])
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(2), "{invalid}: {out:?}");
    }
}
