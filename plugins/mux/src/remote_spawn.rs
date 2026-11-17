//! Spawn policy and lineage must be readable on the pane host by review/hooks.
use super::*;
use base64::Engine;
use vega_herdr::state::SpawnRecord;

pub fn save(host: &str, record: &SpawnRecord, undelivered: Option<&str>) -> Result<(), Failure> {
    let fail = |message: String| {
        Failure::new(
        "remote_spawn_record_failed",
        format!("terminal {} started, but cannot confirm its records on {host}: {message}", record.pane),
        "inspect the existing terminal before retrying spawn; check the host connection, matching vega versions and target state directory",
    )
    };
    let address = vega_herdr::pane::parse(&record.pane)
        .filter(|p| {
            (p.machine.as_deref().is_none() || p.machine.as_deref() == Some(host))
                && ((record.backend == "hc" && p.backend == vega_herdr::pane::Backend::Hc)
                    || (record.backend == "herdr" && p.backend == vega_herdr::pane::Backend::Herdr))
        })
        .ok_or_else(|| fail("spawn returned an address for a different host".into()))?;
    let mut local = record.clone();
    local.pane = address.pane;
    let request = serde_json::json!({"schema":1,"record":local,"undelivered":undelivered});
    let payload = base64::engine::general_purpose::STANDARD.encode(request.to_string());
    let reply = remote_mode::execute(
        host,
        &[
            "exec",
            "--timeout",
            "15",
            "--",
            "vega-mux",
            "record-spawn",
            &payload,
        ],
        &fail,
    )?;
    if reply != request {
        return Err(fail(
            "incompatible or unconfirmed spawn-record reply".into(),
        ));
    }
    Ok(())
}
