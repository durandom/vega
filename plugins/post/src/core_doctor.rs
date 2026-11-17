//! Windows hc server status is existing-only; never use hc connect for this probe.
use std::{process::Command, time::Duration};
use vega_herdr::{output_within, platform, Failure};

const RECOVERY: &str = "on Windows, stop the core with 'hc server stop' in an elevated terminal, then run 'hc server start' in a normal desktop terminal; coordinate active agents first; see docs/design/windows.md";

pub fn doctor() -> Result<(), Failure> {
    let output = output_within(
        Command::new("hc.exe").args(["server", "status", "--json"]),
        Duration::from_secs(5),
    )
    .map_err(|e| Failure::new("hc_core_probe", e.to_string(), "install hc.exe on PATH"))?
    .ok_or_else(|| Failure::new("hc_core_probe", "hc server status timed out", RECOVERY))?;
    let pid = status_pid(output.status.success(), &output.stdout, &output.stderr)?;
    let session = platform::session_id(pid)
        .map_err(|e| Failure::new("hc_core_probe", e.to_string(), "retry vega post doctor"))?;
    check_session(pid, session)?;
    println!("hc_core: pid {pid}, session {session} (desktop)");
    Ok(())
}

fn status_pid(success: bool, stdout: &[u8], stderr: &[u8]) -> Result<u32, Failure> {
    if !success {
        let detail = format!(
            "{} {}",
            String::from_utf8_lossy(stdout),
            String::from_utf8_lossy(stderr)
        );
        let denied = detail.to_ascii_lowercase().contains("access is denied")
            || detail.contains("os error 5");
        return Err(Failure::new(
            if denied {
                "hc_core_access_denied"
            } else {
                "hc_core_unavailable"
            },
            if denied {
                "hc core pipe access denied; an elevated or session-0 core may be running"
            } else {
                "hc core is not reachable (no core started by this probe)"
            },
            if denied {
                RECOVERY
            } else {
                "run 'hc server start' in a normal Windows desktop terminal; then retry vega post doctor"
            },
        ));
    }
    serde_json::from_slice::<serde_json::Value>(stdout)
        .ok()
        .and_then(|v| v["pid"].as_u64())
        .and_then(|pid| u32::try_from(pid).ok())
        .filter(|pid| *pid != 0)
        .ok_or_else(|| {
            Failure::new(
                "hc_core_probe",
                "hc server status returned no valid core PID",
                "update hc, then retry vega post doctor",
            )
        })
}

fn check_session(pid: u32, session: u32) -> Result<(), Failure> {
    if session == 0 {
        Err(Failure::new("hc_core_session_zero", format!("hc core pid {pid} runs in session 0 (Services); desktop pipe access and file ownership may be broken"), RECOVERY))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn uses_the_server_pid_and_reports_session_zero() {
        let pid = status_pid(true, br#"{"pid":123,"holder":{"pid":456}}"#, b"").unwrap();
        assert_eq!(pid, 123);
        assert_eq!(
            check_session(pid, 0).unwrap_err().code,
            "hc_core_session_zero"
        );
        assert!(check_session(pid, 1).is_ok());
        assert!(check_session(pid, 2).is_ok()); // RDP desktop
        assert!(status_pid(true, br#"{"pid":null}"#, b"").is_err());
    }
    #[test]
    fn inaccessible_pipe_has_recovery_even_without_a_pid() {
        let f = status_pid(false, b"", b"Access is denied (os error 5)").unwrap_err();
        assert_eq!(f.code, "hc_core_access_denied");
        assert!(f.hint.contains("normal desktop terminal"));
        assert_eq!(
            status_pid(false, b"", b"missing").unwrap_err().code,
            "hc_core_unavailable"
        );
    }
}
