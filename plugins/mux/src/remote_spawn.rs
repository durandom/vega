//! Spawn policy and lineage must be readable on the pane host by review/hooks.
use super::*;
use base64::Engine;
use vega_herdr::state::{state_file_on, SpawnRecord};

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

/// Internal receiver: no remote scope, and never silently discard a failed write.
pub fn receive(payload: &str) -> Result<(), Failure> {
    let invalid = || {
        Failure::new(
            "usage",
            "invalid local spawn-record request",
            "use matching vega versions on both hosts",
        )
    };
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(payload)
        .map_err(|_| invalid())?;
    let request: Value = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
    let record: SpawnRecord =
        serde_json::from_value(request["record"].clone()).map_err(|_| invalid())?;
    if machine().is_some()
        || request["schema"] != 1
        || !((record.backend == "hc" && vega_herdr::pane::is_hc_id(&record.pane))
            || (record.backend == "herdr" && vega_herdr::pane::is_pane_id(&record.pane)))
        || !valid_agent_name(&record.name)
        || Mode::parse(&record.mode).is_none()
        || !Path::new(&record.cwd).is_absolute()
        || !request
            .get("undelivered")
            .is_some_and(|v| v.is_null() || v.is_string())
    {
        return Err(invalid());
    }
    let parent = record.parent.as_ref().map(|p| {
        serde_json::json!({
            "name":record.name,"parent":p.pane,"host":p.host
        })
        .to_string()
    });
    let mode = serde_json::json!({"name":record.name,"mode":record.mode}).to_string();
    let spawn = serde_json::to_string(&record).expect("serializable record");
    let brief = request["undelivered"].as_str().map(|text| {
        serde_json::json!({
            "name":record.name,"kind":record.kind,"cwd":record.cwd,"brief":text,
            "no_parent":record.parent.is_none()
        })
        .to_string()
    });
    let timestamp = record.started_ms.map(|ms| ms.to_string());
    for (kind, text) in [
        ("spawns", Some(spawn.as_str())),
        ("modes", Some(mode.as_str())),
        ("parents", parent.as_deref()),
        ("tasks", record.task.as_deref()),
        ("briefs", timestamp.as_deref()),
        ("undelivered", brief.as_deref()),
    ] {
        persist(kind, &record.pane, text).map_err(|e| {
            Failure::new(
                "spawn_record_write_failed",
                format!("cannot save {kind} for {}: {e}", record.pane),
                "check the target state directory permissions and disk space",
            )
        })?;
    }
    println!("{request}");
    Ok(())
}

/// Atomic per-file replacement plus read-back; stale optional records are removed.
fn persist(kind: &str, pane: &str, text: Option<&str>) -> std::io::Result<()> {
    let path = state_file_on(kind, None, pane)
        .ok_or_else(|| std::io::Error::other("no state directory"))?;
    if let Some(text) = text {
        std::fs::create_dir_all(path.parent().expect("state file has parent"))?;
        let tmp = path.with_extension(format!("{}.tmp", std::process::id()));
        std::fs::write(&tmp, text)?;
        let result = std::fs::rename(&tmp, &path);
        if result.is_err() {
            let _ = std::fs::remove_file(tmp);
        }
        result?;
        if std::fs::read_to_string(&path)? != text {
            return Err(std::io::Error::other("record read-back mismatch"));
        }
    } else {
        match std::fs::remove_file(&path) {
            Ok(()) => (),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
            Err(e) => return Err(e),
        }
        if path.try_exists()? {
            return Err(std::io::Error::other("stale record remains"));
        }
    }
    Ok(())
}
