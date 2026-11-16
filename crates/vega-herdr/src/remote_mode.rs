//! Bounded target-local JSON transport shared by mode, spawn and adoption.
use crate::Failure;
use base64::Engine;
use serde_json::Value;
use std::time::Duration;

/// Shared bounded target-local execution; callers validate their versioned reply.
pub fn execute(
    host: &str,
    args: &[&str],
    fail: &impl Fn(String) -> Failure,
) -> Result<Value, Failure> {
    let wrapped = crate::hc::hc_on(Some(host), args, Some(Duration::from_secs(20)))
        .map_err(|f| fail(f.message))?;
    if wrapped["timed_out"] != false || wrapped["truncated"] != false {
        return Err(fail(format!(
            "remote command timed out or returned incomplete output (exit {})",
            wrapped["code"]
        )));
    }
    let stdout = wrapped["stdout"]
        .as_str()
        .and_then(|s| base64::engine::general_purpose::STANDARD.decode(s).ok())
        .ok_or_else(|| fail("missing or invalid stdout".into()))?;
    let result: Value = serde_json::from_slice(&stdout)
        .map_err(|_| fail("incompatible remote reply; expected JSON".into()))?;
    if wrapped["code"] != 0 {
        let reason = result["error"]["message"]
            .as_str()
            .unwrap_or("remote command failed");
        return Err(fail(format!("{reason} (exit {})", wrapped["code"])));
    }
    Ok(result)
}
