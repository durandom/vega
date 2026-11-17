//! Shared target-side record protocol; no policy or terminal creation.
use base64::Engine;
use serde_json::Value;
use std::path::Path;
use vega_herdr::state::{state_file_on, Mode, SpawnRecord};
use vega_herdr::{machine, Failure};

pub(super) fn valid_agent_name(name: &str) -> bool {
    let mut chars = name.chars();
    name.len() <= 24
        && chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
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
    if (cfg!(any(windows, feature = "target-only"))
        && (record.backend != "hc" || !matches!(record.kind.as_str(), "codex" | "claude")))
        || machine().is_some()
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
        let result = vega_herdr::platform::replace(&tmp, &path);
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
