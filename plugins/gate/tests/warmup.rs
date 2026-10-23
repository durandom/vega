mod common;

use std::io::{Error, ErrorKind};
use std::os::unix::process::ExitStatusExt;
use std::process::ExitStatus;

#[test]
fn warmup_recovers_only_from_transient_busy_errors() {
    let mut attempts = 0;
    let status = common::warm_with(|| {
        attempts += 1;
        if attempts < 3 {
            Err(Error::from(ErrorKind::ExecutableFileBusy))
        } else {
            Ok(ExitStatus::from_raw(0))
        }
    })
    .unwrap();
    assert!(status.success());
    assert_eq!(attempts, 3);
}

#[test]
fn warmup_does_not_retry_other_errors_or_unsuccessful_exits() {
    for kind in [ErrorKind::NotFound, ErrorKind::PermissionDenied] {
        let mut attempts = 0;
        let error = common::warm_with(|| {
            attempts += 1;
            Err(Error::from(kind))
        })
        .unwrap_err();
        assert_eq!(error.kind(), kind);
        assert_eq!(attempts, 1);
    }
    let mut attempts = 0;
    let status = common::warm_with(|| {
        attempts += 1;
        Ok(ExitStatus::from_raw(1 << 8))
    })
    .unwrap();
    assert!(!status.success());
    assert_eq!(attempts, 1);
}

#[test]
fn warmup_reports_a_persistent_writer_after_fifty_attempts() {
    let mut attempts = 0;
    let error = common::warm_with(|| {
        attempts += 1;
        Err(Error::from(ErrorKind::ExecutableFileBusy))
    })
    .unwrap_err();
    assert_eq!(error.kind(), ErrorKind::ExecutableFileBusy);
    assert_eq!(attempts, 50);
}

#[test]
fn script_warmup_has_no_side_effects_and_reuses_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("ran");
    let script = common::script(
        dir.path(),
        "fake",
        &format!("touch '{}'\n", marker.display()),
    );
    assert!(!marker.exists());
    let original = std::fs::read(&script).unwrap();
    assert_eq!(common::script(dir.path(), "fake", "exit 1\n"), script);
    assert_eq!(std::fs::read(&script).unwrap(), original);
    assert!(std::process::Command::new(script)
        .status()
        .unwrap()
        .success());
    assert!(marker.exists());
}
