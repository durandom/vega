//! Validated host configuration, shared by the CLI and the trusted command receiver.
use crate::seats;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{collections::BTreeMap, path::PathBuf, time::Duration};

pub const MAX_PAYLOAD: usize = 4096;
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Change {
    pub lane: String,
    pub tier: String,
    #[serde(default = "default_seat")]
    pub seat: String,
    pub model: Option<String>,
    #[serde(default)]
    pub clear: bool,
    pub until: Option<i64>,
}
fn default_seat() -> String {
    seats::DEFAULT_SEAT.into()
}

impl Change {
    pub fn validate(&self, now: i64) -> Result<(), String> {
        let pins = seats::pins(seats::SEATS, &self.seat)?;
        if !seats::TIERS.contains(&self.tier.as_str())
            || !pins.iter().any(|p| p.provider == self.lane)
        {
            return Err("unknown lane or tier".into());
        }
        if self.clear == self.model.is_some() || (self.clear && self.until.is_some()) {
            return Err("choose exactly one of model or clear; clear has no until".into());
        }
        if self.model.as_ref().is_some_and(|m| {
            !pins
                .iter()
                .any(|p| p.provider == self.lane && p.model == *m)
        }) {
            return Err("model is not allowed in this seat and lane".into());
        }
        if self.until.is_some_and(|t| t <= now) {
            return Err("until must be in the future".into());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Selection {
    pub model: String,
    pub until: Option<i64>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub schema: u8,
    pub seats: BTreeMap<String, BTreeMap<String, BTreeMap<String, Selection>>>,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            schema: 1,
            seats: BTreeMap::new(),
        }
    }
}
impl Config {
    pub fn parse(value: Value) -> Result<Self, String> {
        let config: Self = serde_json::from_value(value).map_err(|e| e.to_string())?;
        if config.schema != 1 {
            return Err("unknown host pin schema".into());
        }
        for (seat, lanes) in &config.seats {
            for (lane, tiers) in lanes {
                for (tier, pin) in tiers {
                    Change {
                        seat: seat.clone(),
                        lane: lane.clone(),
                        tier: tier.clone(),
                        model: Some(pin.model.clone()),
                        clear: false,
                        until: None,
                    }
                    .validate(0)?;
                }
            }
        }
        Ok(config)
    }
    pub fn apply(&mut self, c: &Change) {
        let tiers = self
            .seats
            .entry(c.seat.clone())
            .or_default()
            .entry(c.lane.clone())
            .or_default();
        if let Some(model) = &c.model {
            tiers.insert(
                c.tier.clone(),
                Selection {
                    model: model.clone(),
                    until: c.until,
                },
            );
        } else {
            tiers.remove(&c.tier);
        }
    }
    pub fn active(&self, seat: &str, lane: &str, tier: &str, now: i64) -> Option<&Selection> {
        self.seats
            .get(seat)?
            .get(lane)?
            .get(tier)
            .filter(|p| p.until.is_none_or(|t| t > now))
    }
}
pub fn validate_host(host: &str) -> Result<(), String> {
    if host.is_empty() || host.len() > 128 || host != vega_registry::token(host) {
        return Err("invalid host token".into());
    }
    Ok(())
}
pub fn key(host: &str) -> String {
    format!("{host}.lanista.host")
}
pub fn decode(from: &str, payload: &[u8]) -> Result<Change, String> {
    validate_host(from)?;
    if !vega_herdr::state::is_inbox_only(from) {
        return Err("sender is not an inbox-only host".into());
    }
    if payload.len() > MAX_PAYLOAD {
        return Err("lanista payload exceeds 4096 bytes".into());
    }
    if payload.iter().copied().find(|b| !b.is_ascii_whitespace()) != Some(b'{') {
        return Err("lanista change must be an object".into());
    }
    let c: Change = serde_json::from_slice(payload).map_err(|e| e.to_string())?;
    c.validate(vega_herdr::capacity::now())?;
    Ok(c)
}

/// CAS preserves unrelated lane edits. All headers are constructed by the KV
/// client; caller-supplied headers/keys/operations never reach this API.
pub async fn write(
    client: &async_nats::Client,
    host: &str,
    change: &Change,
) -> Result<Config, String> {
    validate_host(host)?;
    change.validate(vega_herdr::capacity::now())?;
    let kv = vega_registry::bucket(client).await?;
    let key = key(host);
    for _ in 0..8 {
        let entry = kv.entry(&key).await.map_err(|e| e.to_string())?;
        let (mut config, revision) = match entry {
            Some(e) if e.operation == async_nats::jetstream::kv::Operation::Put => (
                Config::parse(serde_json::from_slice(&e.value).map_err(|e| e.to_string())?)?,
                e.revision,
            ),
            Some(e) => (Config::default(), e.revision),
            None => (Config::default(), 0),
        };
        config.apply(change);
        let bytes = serde_json::to_vec(&config).map_err(|e| e.to_string())?;
        match kv.update(&key, bytes.into(), revision).await {
            Ok(_) => return Ok(config),
            Err(e) if e.kind() == async_nats::jetstream::kv::UpdateErrorKind::WrongLastRevision => {
                continue
            }
            Err(e) => return Err(e.to_string()),
        }
    }
    Err("host pins changed concurrently; retry".into())
}
pub async fn receive(
    client: &async_nats::Client,
    receiver: &str,
    from: &str,
    payload: &[u8],
) -> Result<(), (&'static str, String)> {
    if vega_herdr::state::is_inbox_only(receiver) {
        return Err(("denied", "requires a private receiver".into()));
    }
    let change = decode(from, payload).map_err(|e| ("denied", e))?;
    tokio::time::timeout(Duration::from_secs(5), write(client, from, &change))
        .await
        .map_err(|_| ("handler_failed", "host write timed out".into()))?
        .map(|_| ())
        .map_err(|e| ("handler_failed", e))
}

fn cache_path(host: &str) -> PathBuf {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| seats::home().join(".cache"));
    base.join("vega/lanista").join(format!("{host}.json"))
}
/// Successful absence replaces the cache too, so removed pins cannot return.
pub fn cached_read(
    path: &std::path::Path,
    result: Result<Option<Value>, String>,
) -> Result<Config, String> {
    match result {
        Ok(value) => {
            let config = value.map(Config::parse).transpose()?.unwrap_or_default();
            if let Some(parent) = path.parent() {
                if std::fs::create_dir_all(parent).is_ok() {
                    // Unique file per process; rename gives readers an entire snapshot.
                    let tmp = path.with_extension(format!("{}.tmp", std::process::id()));
                    if std::fs::write(&tmp, serde_json::to_vec(&config).unwrap()).is_ok() {
                        let _ = std::fs::rename(tmp, path);
                    }
                }
            }
            Ok(config)
        }
        Err(e) => {
            let config = std::fs::read(path)
                .ok()
                .and_then(|b| serde_json::from_slice(&b).ok())
                .and_then(|v| Config::parse(v).ok());
            match config {
                Some(c) => {
                    eprintln!("vega lanista: registry unavailable ({e}); using cached host pins");
                    Ok(c)
                }
                None => Err(e),
            }
        }
    }
}
pub fn read(host: &str) -> Result<Config, String> {
    validate_host(host)?;
    let local = vega_herdr::state::post_host();
    if vega_herdr::state::is_inbox_only(&local) && local != host {
        return Err("inbox-only hosts may read only their own host pins".into());
    }
    cached_read(
        &cache_path(host),
        vega_registry::get_one_blocking(&key(host), Duration::from_secs(5)),
    )
}
pub fn mutate(host: &str, change: &Change) -> Result<Config, String> {
    validate_host(host)?;
    change.validate(vega_herdr::capacity::now())?;
    let local = vega_herdr::state::post_host();
    let limited = vega_herdr::state::is_inbox_only(&local);
    if limited && local != host {
        return Err("inbox-only hosts may change only their own host pins".into());
    }
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;
    let config = rt.block_on(async {
        tokio::time::timeout(Duration::from_secs(5), async {
            let client = vega_registry::connect("lanista host").await?;
            if limited {
                let env = vega_theme_sync::Env::from_env().ok_or("no theme control environment")?;
                let control = vega_theme_sync::prefs::load(&env)?.control;
                validate_host(&control)?;
                let answer = client
                    .request(
                        format!("vega.cmd.{control}.lanista.{local}"),
                        serde_json::to_vec(change)
                            .map_err(|e| e.to_string())?
                            .into(),
                    )
                    .await
                    .map_err(|e| e.to_string())?;
                let answer: Value =
                    serde_json::from_slice(&answer.payload).map_err(|e| e.to_string())?;
                if answer["ok"] != true {
                    return Err(format!("relay denied: {}", answer["message"]));
                }
                Config::parse(
                    vega_registry::get_one(&client, &key(host))
                        .await?
                        .ok_or("host pin missing after write")?,
                )
            } else {
                write(&client, host, change).await
            }
        })
        .await
        .map_err(|_| "host pin request timed out".to_string())?
    })?;
    cached_read(
        &cache_path(host),
        Ok(Some(serde_json::to_value(config).unwrap())),
    )
}

pub fn rows(host: &str, config: &Config) -> Vec<Value> {
    let mut rows = Vec::new();
    for seat in seats::seat_ids(seats::SEATS) {
        let pins = seats::pins(seats::SEATS, &seat).unwrap();
        for lane in seats::providers(&pins) {
            let allowed: std::collections::BTreeSet<_> = pins
                .iter()
                .filter(|p| p.provider == lane)
                .map(|p| p.model.clone())
                .collect();
            for tier in seats::TIERS {
                let selected = config.active(&seat, &lane, tier, vega_herdr::capacity::now());
                let model = selected
                    .map(|p| p.model.as_str())
                    .or_else(|| seats::global(&pins, &lane, tier).map(|p| p.model.as_str()));
                rows.push(json!({"host":host,"seat":seat,"lane":lane,"tier":tier,"model":model,"source":if selected.is_some(){"host"}else{"global"},"resolved_lane":if lane == "pi" && selected.is_none(){"zai"}else{&lane},"until":selected.and_then(|p|p.until),"allowed":allowed}));
            }
        }
    }
    rows
}
pub fn list(host: Option<&str>) -> Result<Vec<Value>, String> {
    let local = vega_herdr::state::post_host();
    if let Some(host) = host {
        return Ok(rows(host, &read(host)?));
    }
    if vega_herdr::state::is_inbox_only(&local) {
        return Ok(rows(&local, &read(&local)?));
    }
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;
    rt.block_on(async {
        tokio::time::timeout(Duration::from_secs(5), async {
            let c = vega_registry::connect("lanista hosts").await?;
            let kv = vega_registry::bucket(&c).await?;
            let view = vega_registry::read_kinds(&c, &kv, Some(&[])).await?;
            let hosts: std::collections::BTreeSet<_> = view
                .keys
                .iter()
                .filter_map(|k| k.split('.').next())
                .collect();
            let mut result = Vec::new();
            for h in hosts {
                let config = vega_registry::get_one(&c, &key(h))
                    .await?
                    .map(Config::parse)
                    .transpose()?
                    .unwrap_or_default();
                result.extend(rows(h, &config));
            }
            Ok(result)
        })
        .await
        .map_err(|_| "host listing timed out".to_string())?
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_untrusted_changes_and_expired_selections() {
        let valid = json!({"lane":"codex","tier":"budget","model":"gpt-6.1-sol"});
        let change = decode("puck", valid.to_string().as_bytes()).unwrap();
        assert!(decode("proteus", valid.to_string().as_bytes()).is_err());
        assert!(decode("puck.other", valid.to_string().as_bytes()).is_err());
        for (key, value) in [
            ("lane", json!("unknown")),
            ("tier", json!("unknown")),
            ("seat", json!("unknown")),
            ("model", json!("unknown")),
            ("until", json!(1)),
            ("host", json!("proteus")),
            ("clear", json!(true)),
        ] {
            let mut bad = valid.clone();
            bad[key] = value;
            assert!(decode("puck", bad.to_string().as_bytes()).is_err(), "{bad}");
        }
        assert!(decode("puck", &vec![b' '; 4097]).is_err());
        assert!(decode(
            "puck",
            br#"["codex","budget","herdr-agents","gpt-6.1-sol",false,null]"#
        )
        .is_err());
        let mut config = Config::default();
        let mut change = change;
        change.until = Some(100);
        config.apply(&change);
        assert!(config.active(&change.seat, "codex", "budget", 99).is_some());
        assert!(config
            .active(&change.seat, "codex", "budget", 100)
            .is_none());
        change.clear = true;
        change.model = None;
        change.until = None;
        config.apply(&change);
        assert!(config.active(&change.seat, "codex", "budget", 0).is_none());
    }
    #[test]
    fn cache_survives_outage_but_successful_absence_clears_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cache.json");
        let change = decode(
            "puck",
            br#"{"lane":"codex","tier":"budget","model":"gpt-6.1-sol"}"#,
        )
        .unwrap();
        let mut config = Config::default();
        config.apply(&change);
        cached_read(&path, Ok(Some(serde_json::to_value(config).unwrap()))).unwrap();
        let cached = cached_read(&path, Err("offline".into())).unwrap();
        assert_eq!(
            cached
                .active(&change.seat, "codex", "budget", 0)
                .unwrap()
                .model,
            "gpt-6.1-sol"
        );
        let row = rows("puck", &cached)
            .into_iter()
            .find(|r| r["lane"] == "codex" && r["tier"] == "budget")
            .unwrap();
        assert_eq!(row["source"], "host");
        assert!(row["allowed"]
            .as_array()
            .unwrap()
            .contains(&json!("gpt-6-luna")));
        cached_read(&path, Ok(None)).unwrap();
        assert!(cached_read(&path, Err("offline".into()))
            .unwrap()
            .seats
            .is_empty());
    }
}
