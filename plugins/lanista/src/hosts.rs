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
    write_key(client, &key(host), change).await
}
async fn write_key(
    client: &async_nats::Client,
    key: &str,
    change: &Change,
) -> Result<Config, String> {
    change.validate(vega_herdr::capacity::now())?;
    let kv = vega_registry::bucket(client).await?;
    for _ in 0..8 {
        let entry = kv.entry(key).await.map_err(|e| e.to_string())?;
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
        match kv.update(key, bytes.into(), revision).await {
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
/// Use the same control identity as the bounded host relay.
pub fn control() -> Result<String, String> {
    let env = vega_theme_sync::Env::from_env().ok_or("no control environment")?;
    let host = vega_theme_sync::prefs::load(&env)?.control;
    validate_host(&host)?;
    Ok(host)
}
pub fn default_key(control: &str) -> String {
    format!("{control}.lanista.default")
}
/// Limited hosts never even resolve or request a default key.
pub fn read_default(reader: &str) -> Result<Config, String> {
    if vega_herdr::state::is_inbox_only(reader) {
        return Ok(Config::default());
    }
    let control = control()?;
    let value = vega_registry::get_one_blocking(&default_key(&control), Duration::from_secs(5))?;
    value
        .map(Config::parse)
        .transpose()
        .map(Option::unwrap_or_default)
}
pub async fn write_default(
    client: &async_nats::Client,
    writer: &str,
    control: &str,
    change: &Change,
) -> Result<Config, String> {
    validate_host(control)?;
    validate_default(change)?;
    if vega_herdr::state::is_inbox_only(writer) {
        return Err("inbox-only hosts cannot edit registry defaults".into());
    }
    // The broker enforces ownership or an explicit foreign-key grant.
    write_key(client, &default_key(control), change).await
}
pub fn validate_default(change: &Change) -> Result<(), String> {
    if change.lane == "pi" && !change.clear {
        return Err("pi has no registry default; use pin zai --default --set <glm-model>".into());
    }
    change.validate(vega_herdr::capacity::now())
}
pub fn mutate_default(change: &Change) -> Result<Config, String> {
    validate_default(change)?;
    let local = vega_herdr::state::post_host();
    if vega_herdr::state::is_inbox_only(&local) {
        return Err("inbox-only hosts cannot edit registry defaults".into());
    }
    let control = control()?;
    change.validate(vega_herdr::capacity::now())?;
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;
    rt.block_on(async {
        tokio::time::timeout(Duration::from_secs(5), async {
            let client = vega_registry::connect("lanista default").await?;
            write_default(&client, &local, &control, change).await
        })
        .await
        .map_err(|_| "default write timed out".to_string())?
    })
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

pub struct Resolution {
    pub model: String,
    pub source: &'static str,
    pub until: Option<i64>,
    pub lane: String,
    pub writer: Option<String>,
}
#[allow(clippy::too_many_arguments)]
pub fn resolve_cell(
    view: &vega_registry::View,
    config: &Config,
    defaults: &Config,
    project: Option<&str>,
    seat: &str,
    lane: &str,
    tier: &str,
) -> Option<Resolution> {
    if let Some(project) = project {
        if let Some((writer, model, until)) =
            crate::effective_model(view, project, seat, lane, tier)
        {
            return Some(Resolution {
                model,
                source: "project",
                until,
                lane: lane.into(),
                writer: Some(writer),
            });
        }
    }
    if let Some(pin) = config.active(seat, lane, tier, vega_herdr::capacity::now()) {
        return Some(Resolution {
            model: pin.model.clone(),
            source: "host",
            until: pin.until,
            lane: lane.into(),
            writer: None,
        });
    }
    // Pi is an override lane. Otherwise preserve the entire zai chain.
    if lane == "pi" {
        return resolve_cell(view, config, defaults, project, seat, "zai", tier);
    }
    if let Some(pin) = defaults.active(seat, lane, tier, vega_herdr::capacity::now()) {
        return Some(Resolution {
            model: pin.model.clone(),
            source: "default",
            until: pin.until,
            lane: lane.into(),
            writer: None,
        });
    }
    let pins = seats::pins(seats::SEATS, seat).ok()?;
    let pin = seats::global(&pins, lane, tier)?;
    Some(Resolution {
        model: pin.model.clone(),
        source: "global",
        until: None,
        lane: lane.into(),
        writer: None,
    })
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
/// Overlay default then project, keeping host values above defaults. A limited
/// target's rows must never inherit defaults even when inspected privately.
pub fn layered_rows(
    host: &str,
    config: &Config,
    defaults: &Config,
    view: &vega_registry::View,
    project: Option<&str>,
) -> Vec<Value> {
    let mut result = rows(host, config);
    let empty = Config::default();
    let limited = vega_herdr::state::is_inbox_only(host);
    let defaults = if limited { &empty } else { defaults };
    let project = if limited { None } else { project };
    for row in &mut result {
        if let Some(pin) = resolve_cell(
            view,
            config,
            defaults,
            project,
            row["seat"].as_str().unwrap(),
            row["lane"].as_str().unwrap(),
            row["tier"].as_str().unwrap(),
        ) {
            row["model"] = json!(pin.model);
            row["until"] = json!(pin.until);
            row["source"] = json!(pin.source);
            row["resolved_lane"] = json!(pin.lane);
        }
    }
    result
}

pub fn list(host: Option<&str>) -> Result<Vec<Value>, String> {
    let local = vega_herdr::state::post_host();
    if let Some(h) = host {
        validate_host(h)?;
    }
    if vega_herdr::state::is_inbox_only(&local) {
        let target = host.unwrap_or(&local);
        return Ok(layered_rows(
            target,
            &read(target)?,
            &Config::default(),
            &vega_registry::View::default(),
            None,
        ));
    }
    let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
    let project = vega_herdr::project::resolve_optional(&cwd, &local).map_err(|e| e.message)?;
    let control = control()?;
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;
    rt.block_on(async {
        tokio::time::timeout(Duration::from_secs(5), async {
            let c = vega_registry::connect("lanista hosts").await?;
            let kv = vega_registry::bucket(&c).await?;
            let view = vega_registry::read_kinds(&c, &kv, Some(&["lanista"])).await?;
            let defaults = view
                .entries
                .get(&default_key(&control))
                .cloned()
                .map(Config::parse)
                .transpose()?
                .unwrap_or_default();
            let hosts: std::collections::BTreeSet<_> = match host {
                Some(h) => [h].into_iter().collect(),
                None => view
                    .keys
                    .iter()
                    .filter_map(|k| k.split('.').next())
                    .collect(),
            };
            let mut result = Vec::new();
            for h in hosts {
                let config = view
                    .entries
                    .get(&key(h))
                    .cloned()
                    .map(Config::parse)
                    .transpose()?
                    .unwrap_or_default();
                // Inbox-only targets do not have this private project's view.
                let project = if vega_herdr::state::is_inbox_only(h) {
                    None
                } else {
                    project.as_ref().map(|p| p.id.as_str())
                };
                result.extend(layered_rows(h, &config, &defaults, &view, project));
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
    fn pi_override_then_entire_zai_chain_and_no_pi_default() {
        let seat = seats::DEFAULT_SEAT;
        let change = |lane: &str, model: &str| Change {
            seat: seat.into(),
            lane: lane.into(),
            tier: "budget".into(),
            model: Some(model.into()),
            clear: false,
            until: None,
        };
        let mut host = Config::default();
        let mut defaults = Config::default();
        let mut view = vega_registry::View::default();
        let pick = |v: &vega_registry::View, h: &Config, d: &Config| {
            resolve_cell(
                v,
                h,
                d,
                Some("github.com/team/project"),
                seat,
                "pi",
                "budget",
            )
            .unwrap()
        };
        assert_eq!(pick(&view, &host, &defaults).model, "glm-4.7");
        let google = change("pi", "google/gemini-3.8-flash");
        assert!(validate_default(&google)
            .unwrap_err()
            .contains("pin zai --default"));
        defaults.apply(&google); // Ignore even a previously stored pi default.
        assert_eq!(pick(&view, &host, &defaults).model, "glm-4.7");
        defaults.apply(&change("zai", "glm-5.2"));
        assert_eq!(pick(&view, &host, &defaults).source, "default");
        assert_eq!(pick(&view, &host, &defaults).model, "glm-5.2");
        host.apply(&change("zai", "glm-5.3"));
        assert_eq!(pick(&view, &host, &defaults).model, "glm-5.3");
        let project_pin = |lane: &str, model: &str| json!({"schema":2,"project":"github.com/team/project","seat":seat,"lane":lane,"tier":"budget","model":model,"at":1,"expires":null});
        view.entries.insert(
            vega_registry::pins::key("proteus", "github.com/team/project", seat, "zai", "budget"),
            project_pin("zai", "glm-4.7"),
        );
        assert_eq!(pick(&view, &host, &defaults).source, "project");
        assert_eq!(pick(&view, &host, &defaults).model, "glm-4.7");
        host.apply(&google);
        assert_eq!(pick(&view, &host, &defaults).source, "host");
        assert_eq!(pick(&view, &host, &defaults).lane, "pi");
        view.entries.insert(
            vega_registry::pins::key("proteus", "github.com/team/project", seat, "pi", "budget"),
            project_pin("pi", "google/gemini-4-argon"),
        );
        assert_eq!(pick(&view, &host, &defaults).model, "google/gemini-4-argon");
    }

    #[test]
    fn every_layer_and_limited_host_default_exclusion() {
        let seat = seats::DEFAULT_SEAT;
        let change = |model: &str| Change {
            seat: seat.into(),
            lane: "codex".into(),
            tier: "budget".into(),
            model: Some(model.into()),
            clear: false,
            until: None,
        };
        let mut host = Config::default();
        let mut defaults = Config::default();
        let mut view = vega_registry::View::default();
        let row = |host_name: &str, h: &Config, d: &Config, v: &vega_registry::View| {
            layered_rows(host_name, h, d, v, Some("github.com/team/project"))
                .into_iter()
                .find(|r| r["lane"] == "codex" && r["tier"] == "budget")
                .unwrap()
        };
        assert_eq!(row("proteus", &host, &defaults, &view)["source"], "global");
        defaults.apply(&change("gpt-6.1-sol"));
        assert_eq!(row("proteus", &host, &defaults, &view)["source"], "default");
        assert_eq!(
            row("ganymede", &host, &defaults, &view)["model"],
            "gpt-6.1-sol"
        );
        assert_eq!(row("puck", &host, &defaults, &view)["source"], "global");
        host.apply(&change("gpt-6-astra"));
        assert_eq!(row("proteus", &host, &defaults, &view)["source"], "host");
        assert_eq!(row("puck", &host, &defaults, &view)["model"], "gpt-6-astra");
        let key = vega_registry::pins::key(
            "proteus",
            "github.com/team/project",
            seat,
            "codex",
            "budget",
        );
        view.entries.insert(key, json!({"schema":2,"project":"github.com/team/project","seat":seat,"lane":"codex","tier":"budget","model":"gpt-6-luna","at":1,"expires":null}));
        assert_eq!(row("proteus", &host, &defaults, &view)["source"], "project");
        assert_eq!(
            crate::select_layers(
                &view,
                &host,
                &defaults,
                "github.com/team/project",
                seat,
                "codex",
                "budget"
            )
            .unwrap(),
            "gpt-6-luna"
        );
        view.entries.clear();
        assert_eq!(
            crate::select_layers(&view, &host, &defaults, "p", seat, "codex", "budget").unwrap(),
            "gpt-6-astra"
        );
        host.seats.clear();
        assert_eq!(
            crate::select_layers(&view, &host, &defaults, "p", seat, "codex", "budget").unwrap(),
            "gpt-6.1-sol"
        );
        defaults
            .seats
            .get_mut(seat)
            .unwrap()
            .get_mut("codex")
            .unwrap()
            .get_mut("budget")
            .unwrap()
            .until = Some(1);
        assert_eq!(row("proteus", &host, &defaults, &view)["source"], "global");
        // This returns before loading control config or making any broker request.
        assert!(read_default("puck").unwrap().seats.is_empty());
    }

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
