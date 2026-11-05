//! Opt-in inbox retention update. Preserve every other raw server field.
use crate::{Failure, ARCHIVE_MAX_BYTES};
use serde_json::value::RawValue;
use std::collections::BTreeMap;
type Config = BTreeMap<String, Box<RawValue>>;

pub fn validate_target(hosts: &[String], archive: bool) -> Result<(), Failure> {
    if archive
        && (hosts.is_empty()
            || hosts
                .iter()
                .any(|h| matches!(h.as_str(), "history" | "registry" | "lanista")))
    {
        return Err(Failure::new(
            "usage",
            "--archive requires inbox host names only",
            "vega post provision <host> --archive",
        ));
    }
    Ok(())
}

fn error(e: impl std::fmt::Display) -> Failure {
    Failure::new("archive_update_refused", e.to_string(), "use `vega post provision <host> --archive` with temporary `auth-config --create archive` admin rights; remove that grant and reload afterwards")
}

fn plan(raw: &str, name: &str) -> Result<Option<Vec<u8>>, Failure> {
    let mut config: Config = serde_json::from_str(raw).map_err(error)?;
    let actual: String = serde_json::from_str(
        config
            .get("name")
            .ok_or_else(|| error("missing stream name"))?
            .get(),
    )
    .map_err(error)?;
    if actual != name || !name.starts_with("VEGA_INBOX_") {
        return Err(error("unexpected stream name"));
    }
    if config.get("max_age").map(|v| v.get()) == Some("0")
        && config.get("max_bytes").map(|v| v.get()) == Some(ARCHIVE_MAX_BYTES.to_string().as_str())
    {
        return Ok(None);
    }
    config.insert("max_age".into(), RawValue::from_string("0".into()).unwrap());
    config.insert(
        "max_bytes".into(),
        RawValue::from_string(ARCHIVE_MAX_BYTES.to_string()).unwrap(),
    );
    serde_json::to_vec(&config).map(Some).map_err(error)
}

async fn request(
    client: &async_nats::Client,
    subject: String,
    payload: Vec<u8>,
) -> Result<Box<RawValue>, Failure> {
    let reply = client
        .request(subject, payload.into())
        .await
        .map_err(error)?;
    let mut reply: Config = serde_json::from_slice(&reply.payload).map_err(error)?;
    if let Some(e) = reply.get("error") {
        return Err(error(e.get()));
    }
    reply
        .remove("config")
        .ok_or_else(|| error("broker response has no config"))
}

pub async fn update(client: &async_nats::Client, host: &str) -> Result<bool, Failure> {
    let name = crate::stream_name(host);
    let raw = request(
        client,
        format!("$JS.API.STREAM.INFO.{name}"),
        b"{}".to_vec(),
    )
    .await?;
    let Some(payload) = plan(raw.get(), &name)? else {
        return Ok(false);
    };
    let expected: Config = serde_json::from_slice(&payload).map_err(error)?;
    let after = request(client, format!("$JS.API.STREAM.UPDATE.{name}"), payload).await?;
    let after: Config = serde_json::from_str(after.get()).map_err(error)?;
    if expected.len() != after.len()
        || expected
            .iter()
            .any(|(k, v)| after.get(k).map(|n| n.get()) != Some(v.get()))
    {
        return Err(error(
            "UPDATE response changed fields beyond max_age/max_bytes; inspect stream configuration",
        ));
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn archive_requires_inbox_hosts_before_connecting() {
        for hosts in [
            vec![],
            vec!["history".into()],
            vec!["registry".into()],
            vec!["lanista".into()],
            vec!["proteus".into(), "history".into()],
        ] {
            assert_eq!(
                crate::provision(&hosts, false, true).unwrap_err().code,
                "usage"
            );
        }
        assert!(validate_target(&["proteus".into()], true).is_ok());
        assert!(validate_target(&["proteus".into(), "deimos".into()], true).is_ok());
        assert!(validate_target(&[], false).is_ok());
        assert!(validate_target(&["history".into()], false).is_ok());
    }

    #[test]
    fn only_changes_two_fields() {
        let raw = r#"{"name":"VEGA_INBOX_h","max_age":1,"max_bytes":123,"future":{ "big":18446744073709551616 },"discard":"old"}"#;
        let before: Config = serde_json::from_str(raw).unwrap();
        let payload = plan(raw, "VEGA_INBOX_h").unwrap().unwrap();
        let after: Config = serde_json::from_slice(&payload).unwrap();
        for (k, v) in before {
            if k != "max_age" && k != "max_bytes" {
                assert_eq!(after[&k].get(), v.get());
            }
        }
        assert!(plan(std::str::from_utf8(&payload).unwrap(), "VEGA_INBOX_h")
            .unwrap()
            .is_none());
        assert!(plan(raw, "OTHER").is_err());
    }
}
