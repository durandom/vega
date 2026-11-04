//! The sole opt-in stream update: preserve raw fields, enable registry TTL.
//! Never round-trip through async-nats Config: it can drop newer server fields.
use std::collections::BTreeMap;

use serde_json::value::RawValue;

use crate::Failure;

const INFO: &str = "$JS.API.STREAM.INFO.KV_VEGA_REGISTRY";
const UPDATE: &str = "$JS.API.STREAM.UPDATE.KV_VEGA_REGISTRY";
const TTL: &str = "allow_msg_ttl";
type Config = BTreeMap<String, Box<RawValue>>;

fn refused(message: impl std::fmt::Display) -> Failure {
    Failure::new(
        "registry_ttl_refused",
        message.to_string(),
        "use `vega post provision registry --ttl` with the temporary `auth-config --create registry-ttl` admin grant; if the bucket is missing, create it first with `auth-config --create registry` and `vega post provision registry`; remove the grant and reload afterwards",
    )
}

pub fn validate_target(hosts: &[String], ttl: bool) -> Result<(), Failure> {
    if ttl && hosts != ["registry"] {
        return Err(Failure::new(
            "usage",
            "--ttl requires exactly one target: registry",
            "vega post provision registry --ttl",
        ));
    }
    Ok(())
}

fn fields(raw: &str) -> Result<Config, Failure> {
    serde_json::from_str(raw).map_err(refused)
}

/// Compare the exact JSON values, including unknown/nested fields and numbers.
/// Only adding allow_msg_ttl=true or changing false to true is permitted.
fn check_update(before: &Config, after: &Config) -> Result<(), Failure> {
    if before.get(TTL).is_some_and(|v| v.get() != "false")
        || after.get(TTL).map(|v| v.get()) != Some("true")
        || before.iter().filter(|(k, _)| k.as_str() != TTL).count() + 1 != after.len()
        || before
            .iter()
            .any(|(k, v)| k != TTL && after.get(k).map(|a| a.get()) != Some(v.get()))
    {
        return Err(refused("refusing to change any field except allow_msg_ttl"));
    }
    Ok(())
}

fn plan(raw: &str) -> Result<Option<Vec<u8>>, Failure> {
    let before = fields(raw)?;
    if before.get("name").map(|v| v.get()) != Some("\"KV_VEGA_REGISTRY\"") {
        return Err(refused("STREAM.INFO did not name KV_VEGA_REGISTRY"));
    }
    if before.get(TTL).map(|v| v.get()) == Some("true") {
        return Ok(None);
    }
    let mut after = fields(raw)?;
    after.insert(TTL.into(), RawValue::from_string("true".into()).unwrap());
    let payload = serde_json::to_vec(&after).map_err(refused)?;
    // Check the actual outbound bytes, not just the intended in-memory change.
    check_update(&before, &serde_json::from_slice(&payload).map_err(refused)?)?;
    Ok(Some(payload))
}

/// NATS raises this server-owned capability marker when enabling TTL. This
/// exception is response-only: the outbound payload must preserve it verbatim.
fn check_response(before: &Config, mut after: Config) -> Result<(), Failure> {
    if let (Some(old), Some(new)) = (before.get("metadata"), after.get("metadata")) {
        let old = fields(old.get())?;
        let mut new = fields(new.get())?;
        let level = "_nats.req.level";
        if old.get(level).map(|v| v.get()) == Some("\"0\"")
            && new.get(level).map(|v| v.get()) == Some("\"1\"")
        {
            new.insert(level.into(), RawValue::from_string("\"0\"".into()).unwrap());
            // Compare all remaining metadata values too; never permit republish
            // or other config changes under the server-normalization exception.
            if old.len() != new.len()
                || old
                    .iter()
                    .any(|(k, v)| new.get(k).map(|n| n.get()) != Some(v.get()))
            {
                return Err(refused("unexpected metadata change in UPDATE response"));
            }
            after.insert(
                "metadata".into(),
                RawValue::from_string(before["metadata"].get().into()).map_err(refused)?,
            );
        }
    }
    check_update(before, &after)
}

async fn request(
    client: &async_nats::Client,
    subject: &'static str,
    payload: Vec<u8>,
) -> Result<Config, Failure> {
    let reply = client
        .request(subject, payload.into())
        .await
        .map_err(refused)?;
    let response: Config = serde_json::from_slice(&reply.payload).map_err(refused)?;
    if let Some(error) = response.get("error") {
        return Err(refused(error.get()));
    }
    let config = response
        .get("config")
        .ok_or_else(|| refused("broker response has no config"))?;
    fields(config.get())
}

pub async fn enable(client: &async_nats::Client) -> Result<bool, Failure> {
    let before = request(client, INFO, b"{}".to_vec()).await?;
    let raw = serde_json::to_string(&before).map_err(refused)?;
    let Some(payload) = plan(&raw)? else {
        return Ok(false);
    };
    let after = request(client, UPDATE, payload).await?;
    check_response(&before, after)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_unknown_fields_and_exact_numbers() {
        for flag in ["", ",\"allow_msg_ttl\":false"] {
            let raw = format!(
                r#"{{"name":"KV_VEGA_REGISTRY","max_age":0,"future":{{ "big":18446744073709551616, "nested": [1, 2] }},"description":"allow_msg_ttl:false"{flag}}}"#
            );
            let payload = plan(&raw).unwrap().unwrap();
            let before = fields(&raw).unwrap();
            let after: Config = serde_json::from_slice(&payload).unwrap();
            check_update(&before, &after).unwrap();
            assert_eq!(before["future"].get(), after["future"].get());
            assert!(plan(std::str::from_utf8(&payload).unwrap())
                .unwrap()
                .is_none());
            for (key, value) in [("max_age", "1"), ("republish", "{}"), (TTL, "false")] {
                let mut bad = fields(std::str::from_utf8(&payload).unwrap()).unwrap();
                bad.insert(key.into(), RawValue::from_string(value.into()).unwrap());
                assert!(check_update(&before, &bad).is_err());
            }
            let mut bad = after;
            bad.remove("future");
            assert!(check_update(&before, &bad).is_err());
        }
        assert!(plan(r#"{"name":"OTHER","allow_msg_ttl":false}"#).is_err());
        assert!(plan(r#"{"name":"KV_VEGA_REGISTRY","allow_msg_ttl":null}"#).is_err());
    }

    #[test]
    fn server_level_exception_is_response_only_and_narrow() {
        let raw = r#"{"name":"KV_VEGA_REGISTRY","allow_msg_ttl":false,"metadata":{"_nats.req.level":"0","keep":"yes"}}"#;
        let before = fields(raw).unwrap();
        let changed = raw.replace("false", "true").replace("\"0\"", "\"1\"");
        assert!(check_update(&before, &fields(&changed).unwrap()).is_err());
        check_response(&before, fields(&changed).unwrap()).unwrap();
        for bad in [
            changed.replace("yes", "no"),
            changed.replace("\"1\"", "\"2\""),
            changed.replace("KV_VEGA_REGISTRY", "OTHER"),
        ] {
            assert!(check_response(&before, fields(&bad).unwrap()).is_err());
        }
    }

    #[test]
    fn registry_ttl_local_broker() {
        use crate::auth::{self, Entry};
        use crate::registry_read_test::{connect, denied, start, user};
        for granted in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let (admin_seed, admin) = user();
            let (host_seed, host) = user();
            let entries = vec![Entry {
                host: "proteus".into(),
                key: host,
            }];
            let mut grants = vec!["registry".into()];
            if granted {
                grants.push("registry-ttl".into());
            }
            let authz = auth::render(
                &entries,
                Some(&admin),
                &grants,
                &[],
                None,
                &[],
                &[],
                &[],
                None,
            );
            let (_server, url) = start(dir.path(), &authz);
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(async {
                    let admin = connect(&url, &admin_seed, crate::ADMIN_INBOX).await;
                    // TTL provisioning never creates a missing stream.
                    assert!(enable(&admin).await.is_err());
                    async_nats::jetstream::new(admin.clone())
                        .create_key_value(async_nats::jetstream::kv::Config {
                            bucket: vega_registry::BUCKET.into(),
                            history: 1,
                            max_bytes: 64 << 20,
                            description: "preserve me".into(),
                            ..Default::default()
                        })
                        .await
                        .unwrap();
                    let before = request(&admin, INFO, b"{}".to_vec()).await.unwrap();
                    let host = connect(&url, &host_seed, "_INBOX_proteus").await;
                    assert!(denied(&host, UPDATE, "{}").await);
                    if granted {
                        assert!(enable(&admin).await.unwrap());
                        let after = request(&admin, INFO, b"{}".to_vec()).await.unwrap();
                        check_response(&before, after).unwrap();
                        assert!(!enable(&admin).await.unwrap());
                        // Host has INFO but no UPDATE: proves the no-op sends no UPDATE.
                        assert!(!enable(&host).await.unwrap());
                        assert!(
                            denied(&admin, "$JS.API.STREAM.UPDATE.VEGA_INBOX_proteus", "{}").await
                        );
                    } else {
                        assert!(enable(&admin).await.is_err());
                        let after = request(&admin, INFO, b"{}".to_vec()).await.unwrap();
                        assert_eq!(
                            serde_json::to_string(&before).unwrap(),
                            serde_json::to_string(&after).unwrap()
                        );
                    }
                });
        }
    }

    #[test]
    fn ttl_target_is_exclusive() {
        for hosts in [
            vec![],
            vec!["proteus"],
            vec!["registry", "proteus"],
            vec!["registry", "registry"],
        ] {
            assert!(validate_target(
                &hosts.into_iter().map(String::from).collect::<Vec<_>>(),
                true
            )
            .is_err());
        }
        assert!(validate_target(&["registry".into()], true).is_ok());
    }
}
