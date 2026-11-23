//! Adoption is acknowledged only after the pane host confirms its local records.
use super::*;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    schema: u8,
    target: String,
    name: String,
    parent: st::Parent,
    mode: String,
    brief_ms: u128,
}

pub fn save(
    machine: Option<&str>,
    pane: &str,
    name: &str,
    parent: &str,
    mode: st::Mode,
) -> Result<(), Failure> {
    let request = Request {
        schema: 1,
        target: format!(
            "{}:{pane}",
            if vega_herdr::pane::is_hc_id(pane) {
                "hc"
            } else {
                "herdr"
            }
        ),
        name: name.into(),
        parent: st::Parent {
            pane: parent.into(),
            host: st::post_host(),
        },
        mode: mode.as_str().into(),
        brief_ms: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
    };
    if let Some(host) = machine {
        vega_herdr::hc::check_direction(host)?;
        let fail = |message: String| {
            Failure::new(
            "remote_adoption_failed",
            format!("cannot confirm adoption of {pane} on {host}: {message}"),
            "check the host connection, matching vega versions and target state directory; retry adoption",
        )
        };
        let payload = serde_json::to_string(&request).expect("serializable adoption");
        let reply = vega_herdr::remote_mode::execute(
            host,
            &[
                "exec",
                "--timeout",
                "15",
                "--",
                "vega-uac",
                "record-adoption",
                &payload,
            ],
            &fail,
        )?;
        if reply != serde_json::to_value(&request).expect("serializable adoption") {
            return Err(fail("incompatible or unconfirmed adoption reply".into()));
        }
    }
    // Retain the existing caller cache for UAC controls, only after confirmation.
    persist(machine, pane, &request)
}

pub fn receive(args: &[String]) -> Result<(), Failure> {
    let invalid = || {
        Failure::new(
            "usage",
            "invalid local adoption request",
            "use matching vega versions on both hosts",
        )
    };
    let [payload] = args else {
        return Err(invalid());
    };
    let r: Request = serde_json::from_str(payload).map_err(|_| invalid())?;
    let address = vega_herdr::pane::parse(&r.target)
        .filter(|a| a.machine.is_none())
        .ok_or_else(invalid)?;
    if r.schema != 1
        || r.parent.host.is_empty()
        || vega_herdr::pane::parse(&r.parent.pane).is_none_or(|p| p.machine.is_some())
        || st::Mode::parse(&r.mode).is_none()
    {
        return Err(invalid());
    }
    // Do not attach records to a different agent that has reused the pane.
    let a = terminal::agent(&r.target, None)?;
    if agent_name(&a) != r.name {
        return Err(Failure::new(
            "agent_changed",
            "adoption target changed",
            "inspect the pane and retry adoption",
        ));
    }
    persist(None, &address.pane, &r)?;
    println!(
        "{}",
        serde_json::to_string(&r).expect("serializable adoption")
    );
    Ok(())
}

fn persist(machine: Option<&str>, pane: &str, r: &Request) -> Result<(), Failure> {
    for (kind, text) in [
        (
            "parents",
            serde_json::json!({"name":r.name,"parent":r.parent.pane,"host":r.parent.host})
                .to_string(),
        ),
        (
            "modes",
            serde_json::json!({"name":r.name,"mode":r.mode}).to_string(),
        ),
        ("briefs", r.brief_ms.to_string()),
    ] {
        let write = || -> std::io::Result<()> {
            let path = st::state_file_on(kind, machine, pane)
                .ok_or_else(|| std::io::Error::other("no state directory"))?;
            std::fs::create_dir_all(path.parent().expect("state file parent"))?;
            let tmp = path.with_extension(format!("{}.tmp", std::process::id()));
            std::fs::write(&tmp, &text)?;
            if let Err(e) = vega_herdr::platform::replace(&tmp, &path) {
                let _ = std::fs::remove_file(tmp);
                return Err(e);
            }
            if std::fs::read_to_string(path)? != text {
                return Err(std::io::Error::other("record read-back mismatch"));
            }
            Ok(())
        };
        write().map_err(|e| {
            Failure::new(
                "adoption_write_failed",
                format!("cannot save {kind} for {pane}: {e}"),
                "check the state directory permissions and disk space; retry adoption",
            )
        })?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn invalid_receivers_fail_before_resolving_a_terminal() {
        assert!(receive(&[]).is_err());
        assert!(receive(&["{}".into()]).is_err());
        let base = serde_json::json!({"schema":1,"target":"hc:t9","name":"worker","parent":{"pane":"t1","host":"uac-host"},"mode":"headed","brief_ms":1});
        for (field, value) in [
            ("schema", serde_json::json!(2)),
            ("target", serde_json::json!("hc:io/t9")),
            ("target", serde_json::json!("hc:worker")),
            ("mode", serde_json::json!("unknown")),
            (
                "parent",
                serde_json::json!({"pane":"hc:other/t1","host":"uac-host"}),
            ),
            ("parent", serde_json::json!({"pane":"t1","host":""})),
        ] {
            let mut request = base.clone();
            request[field] = value;
            let error = receive(&[request.to_string()]).unwrap_err();
            assert_eq!(error.code, "usage");
        }
    }
}
