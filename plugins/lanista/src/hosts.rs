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
        let pins = seats::effective(&self.seat)?;
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
    pins: &[seats::Pin],
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
    // Pi is an override lane: an overlay pi pin for the tier (e.g. an
    // EnMaaS gateway) wins over the historical zai chain. Otherwise the
    // entire zai chain is preserved.
    if lane == "pi" {
        if let Some(pin) = seats::pick(pins, "pi", tier) {
            return Some(Resolution {
                model: pin.model.clone(),
                source: "overlay",
                until: None,
                lane: lane.into(),
                writer: None,
            });
        }
        return resolve_cell(view, config, defaults, pins, project, seat, "zai", tier);
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
    let pin = seats::global(pins, lane, tier)?;
    Some(Resolution {
        model: pin.model.clone(),
        source: "global",
        until: None,
        lane: lane.into(),
        writer: None,
    })
}

pub fn rows(host: &str, config: &Config, seats_pins: &[(String, Vec<seats::Pin>)]) -> Vec<Value> {
    let mut rows = Vec::new();
    for (seat, pins) in seats_pins {
        let seat = seat.as_str();
        for lane in seats::providers(pins) {
            let allowed: std::collections::BTreeSet<_> = pins
                .iter()
                .filter(|p| p.provider == lane)
                .map(|p| p.model.clone())
                .collect();
            for tier in seats::TIERS {
                let selected = config.active(seat, &lane, tier, vega_herdr::capacity::now());
                let model = selected
                    .map(|p| p.model.as_str())
                    .or_else(|| seats::global(pins, &lane, tier).map(|p| p.model.as_str()));
                rows.push(json!({"host":host,"seat":seat,"lane":&lane,"tier":tier,"model":model,"source":if selected.is_some(){"host"}else{"global"},"resolved_lane":if lane == "pi" && selected.is_none(){"zai"}else{&lane},"until":selected.and_then(|p|p.until),"allowed":allowed}));
            }
        }
    }
    rows
}
/// Overlay default then project, keeping host values above defaults. A limited
/// target's rows must never inherit defaults even when inspected privately.
/// `table` is lanista's routes (read once at the CLI boundary): each row's
/// model carries the resolved route facts — cost class and origin — with
/// the seats overlay's deprecated `canonical`/`cost` as the fallback.
pub fn layered_rows(
    host: &str,
    config: &Config,
    defaults: &Config,
    view: &vega_registry::View,
    project: Option<&str>,
    seats_pins: &[(String, Vec<seats::Pin>)],
    table: &vega_herdr::routes::Table,
) -> Vec<Value> {
    let mut result = rows(host, config, seats_pins);
    let empty = Config::default();
    let limited = vega_herdr::state::is_inbox_only(host);
    let defaults = if limited { &empty } else { defaults };
    let project = if limited { None } else { project };
    for row in &mut result {
        if let Some(pin) = resolve_cell(
            view,
            config,
            defaults,
            seats_pins
                .iter()
                .find(|(s, _)| s == row["seat"].as_str().unwrap())
                .map(|(_, p)| p.as_slice())
                .unwrap_or_default(),
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
        let seat_pins = seats_pins
            .iter()
            .find(|(s, _)| s == row["seat"].as_str().unwrap())
            .map(|(_, p)| p.as_slice())
            .unwrap_or_default();
        let model = row["model"].as_str().unwrap_or_default();
        let (facts, warnings) = crate::routes::resolve(model, seat_pins, table);
        for warning in warnings {
            eprintln!("vega lanista: {warning}");
        }
        row["canonical"] = json!(facts.as_ref().and_then(|f| f.canonical.clone()));
        row["cost"] = json!(facts.as_ref().and_then(|f| f.cost.clone()));
        row["cost_origin"] = json!(facts.as_ref().map(|f| f.origin));
    }
    result
}

pub fn list(host: Option<&str>, table: &vega_herdr::routes::Table) -> Result<Vec<Value>, String> {
    let seats_pins = seats::effective_seats();
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
            &seats_pins,
            table,
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
                result.extend(layered_rows(
                    h,
                    &config,
                    &defaults,
                    &view,
                    project,
                    &seats_pins,
                    table,
                ));
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
    /// Repo seats only: hermetic, no host-local overlay.
    fn repo_seats() -> Vec<(String, Vec<seats::Pin>)> {
        seats::effective_seats_at(std::path::Path::new("/nonexistent-overlay"))
    }
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
                &repo_seats()
                    .into_iter()
                    .find(|(s, _)| s == seat)
                    .map(|(_, p)| p)
                    .unwrap(),
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
            layered_rows(
                host_name,
                h,
                d,
                v,
                Some("github.com/team/project"),
                &repo_seats(),
                &vega_herdr::routes::Table::default(),
            )
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
        let seat_pins: Vec<seats::Pin> = repo_seats()
            .into_iter()
            .find(|(s, _)| s == seat)
            .map(|(_, p)| p)
            .unwrap();
        assert_eq!(
            crate::select_layers(
                &view,
                &host,
                &defaults,
                &seat_pins,
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
            crate::select_layers(&view, &host, &defaults, &seat_pins, "p", seat, "codex", "budget")
                .unwrap(),
            "gpt-6-astra"
        );
        host.seats.clear();
        assert_eq!(
            crate::select_layers(&view, &host, &defaults, &seat_pins, "p", seat, "codex", "budget")
                .unwrap(),
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
        let row = rows("puck", &cached, &repo_seats())
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

    #[test]
    fn overlay_pi_wins_over_zai_chain_but_not_project_or_host() {
        let seat = seats::DEFAULT_SEAT;
        let mut pins = seats::pins(seats::SEATS, seat).unwrap();
        pins.insert(
            0,
            seats::Pin {
                provider: "pi".into(),
                model: "enmaas/gome-3.1".into(),
                engine: "pi".into(),
                role: String::new(),
                tier: "budget".into(),
                canonical: Some("gome-3.1".into()),
                cost: "subscription".into(),
            },
        );
        let mut host = Config::default();
        let mut defaults = Config::default();
        let mut view = vega_registry::View::default();
        let pick = |v: &vega_registry::View, h: &Config, d: &Config| {
            resolve_cell(
                v,
                h,
                d,
                &pins,
                Some("github.com/team/project"),
                seat,
                "pi",
                "budget",
            )
            .unwrap()
        };
        // Overlay beats the entire zai chain, including a zai default.
        defaults.apply(&Change {
            seat: seat.into(),
            lane: "zai".into(),
            tier: "budget".into(),
            model: Some("glm-5.2".into()),
            clear: false,
            until: None,
        });
        let resolved = pick(&view, &host, &defaults);
        assert_eq!(
            (
                resolved.model.as_str(),
                resolved.source,
                resolved.lane.as_str()
            ),
            ("enmaas/gome-3.1", "overlay", "pi")
        );
        // Host and project pi pins still outrank the overlay.
        host.apply(&Change {
            seat: seat.into(),
            lane: "pi".into(),
            tier: "budget".into(),
            model: Some("google/gemini-3.8-flash".into()),
            clear: false,
            until: None,
        });
        assert_eq!(
            pick(&view, &host, &defaults).model,
            "google/gemini-3.8-flash"
        );
        view.entries.insert(
            vega_registry::pins::key("proteus", "github.com/team/project", seat, "pi", "budget"),
            json!({"schema":2,"project":"github.com/team/project","seat":seat,"lane":"pi","tier":"budget","model":"google/gemini-4-argon","at":1,"expires":null}),
        );
        assert_eq!(pick(&view, &host, &defaults).model, "google/gemini-4-argon");
        // Without the overlay pin the historical zai chain returns
        // (default zai beats repo zai).
        let repo = seats::pins(seats::SEATS, seat).unwrap();
        assert_eq!(
            resolve_cell(
                &view,
                &Config::default(),
                &defaults,
                &repo,
                None,
                seat,
                "pi",
                "budget"
            )
            .unwrap()
            .model,
            "glm-5.2"
        );
    }

    #[test]
    fn overlay_rows_list_gateway_pins_and_allowlist() {
        let seat = seats::DEFAULT_SEAT;
        let mut seats_pins = repo_seats();
        let (_, pins) = seats_pins.iter_mut().find(|(s, _)| s == seat).unwrap();
        pins.insert(
            0,
            seats::Pin {
                provider: "pi".into(),
                model: "enmaas/gome-3.1".into(),
                engine: "pi".into(),
                role: String::new(),
                tier: "flagship".into(),
                canonical: None,
                cost: "subscription".into(),
            },
        );
        let table = vega_herdr::routes::Table {
            routes: [(
                "enmaas/gome-3.1".to_string(),
                vega_herdr::routes::Route {
                    route_id: "enmaas/gome-3.1".into(),
                    canonical: Some("gome-3.1".into()),
                    cost: Some("budget".into()),
                    cost_origin: "local".into(),
                },
            )]
            .into_iter()
            .collect(),
            ..Default::default()
        };
        let row = layered_rows(
            "puck",
            &Config::default(),
            &Config::default(),
            &vega_registry::View::default(),
            None,
            &seats_pins,
            &table,
        )
        .into_iter()
        .find(|r| r["lane"] == "pi" && r["tier"] == "flagship")
        .unwrap();
        assert_eq!(row["model"], "enmaas/gome-3.1");
        assert_eq!(row["source"], "overlay");
        assert_eq!(row["resolved_lane"], "pi");
        assert!(row["allowed"]
            .as_array()
            .unwrap()
            .contains(&json!("enmaas/gome-3.1")));
        // Route facts come from lanista (cost class and origin), not the
        // seats overlay's deprecated canonical/cost.
        assert_eq!(row["canonical"], "gome-3.1");
        assert_eq!(row["cost"], "budget");
        assert_eq!(row["cost_origin"], "local");
        // A repo pin without a lanista entry carries no facts.
        let row = layered_rows(
            "puck",
            &Config::default(),
            &Config::default(),
            &vega_registry::View::default(),
            None,
            &seats_pins,
            &table,
        )
        .into_iter()
        .find(|r| r["lane"] == "codex" && r["tier"] == "budget")
        .unwrap();
        assert_eq!(row["model"], "gpt-6-luna");
        assert_eq!(row["cost"], Value::Null);
        assert_eq!(row["cost_origin"], Value::Null);
    }

    /// The puck story end to end on a fake host: an overlay file pins pi
    /// tiers to an EnMaaS gateway id and `hosts` resolves it without repo
    /// edits, with route facts from a fake lanista (`lanista --json routes`).
    #[test]
    fn fake_host_listing_resolves_overlay_pins() {
        const MARKER: &str = "VEGA_HOSTS_OVERLAY_TEST";
        if std::env::var_os(MARKER).is_none() {
            let dir = tempfile::tempdir().unwrap();
            let config = dir.path().join("vega/lanista");
            std::fs::create_dir_all(&config).unwrap();
            std::fs::write(
                config.join("seats.local.json"),
                r#"{"schema":1,"seats":{"herdr-agents":[
                    {"provider":"pi","pin":"enmaas/gome-3.1","tier":"flagship","cost":"subscription"},
                    {"provider":"pi","pin":"enmaas/gome-2.4","tier":"balanced","cost":"budget"}
                ]}}"#,
            )
            .unwrap();
            let fake = dir.path().join("fake-lanista");
            std::fs::write(
                &fake,
                format!(
                    "#!/bin/sh\ncat <<'JSON'\n{}\nJSON\n",
                    r#"{"overlay":{"loaded":true,"route_count":2},"routes":{
                        "enmaas/gome-3.1":{"route_id":"enmaas/gome-3.1","canonical":"gome-3.1",
                            "fields":{"cost":{"value":"subscription","origin":"local"}}},
                        "enmaas/gome-2.4":{"route_id":"enmaas/gome-2.4","canonical":"gome-2.4",
                            "fields":{"cost":{"value":"budget","origin":"local"}}}
                    }}"#
                    .replace('\n', "")
                ),
            )
            .unwrap();
            {
                use std::os::unix::fs::PermissionsExt;
                let mut perms = std::fs::metadata(&fake).unwrap().permissions();
                perms.set_mode(0o755);
                std::fs::set_permissions(&fake, perms).unwrap();
            }
            let cache = dir.path().join("vega/lanista");
            std::fs::create_dir_all(&cache).unwrap();
            std::fs::write(cache.join("puck.json"), "{\"schema\":1,\"seats\":{}}").unwrap();
            let out = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "hosts::tests::fake_host_listing_resolves_overlay_pins",
                    "--nocapture",
                ])
                .env(MARKER, "1")
                .env("XDG_CONFIG_HOME", dir.path())
                .env("XDG_CACHE_HOME", dir.path())
                .env("VEGA_LANISTA_BIN", &fake)
                .env("VEGA_POST_HOST", "puck")
                .env("VEGA_NATS_URL", "nats://127.0.0.1:1")
                .env("VEGA_NATS_NKEY", dir.path().join("missing"))
                .env_remove("VEGA_LANISTA_PIN_HOSTS")
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{}{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
            return;
        }
        let table = vega_herdr::routes::read().expect("fake lanista answers");
        let rows = list(None, &table).unwrap();
        let row = rows
            .iter()
            .find(|r| r["lane"] == "pi" && r["tier"] == "flagship")
            .unwrap();
        assert_eq!(row["model"], "enmaas/gome-3.1");
        assert_eq!(row["source"], "overlay");
        assert_eq!(row["resolved_lane"], "pi");
        // Route facts through the fake lanista: cost class and origin.
        assert_eq!(row["canonical"], "gome-3.1");
        assert_eq!(row["cost"], "subscription");
        assert_eq!(row["cost_origin"], "local");
        let row = rows
            .iter()
            .find(|r| r["lane"] == "pi" && r["tier"] == "balanced")
            .unwrap();
        assert_eq!(row["model"], "enmaas/gome-2.4");
        // The seats overlay says budget here too; lanista's entry agrees.
        assert_eq!(row["cost"], "budget");
        assert_eq!(row["cost_origin"], "local");
        // Untouched tiers keep the zai fallback, with no route facts.
        let row = rows
            .iter()
            .find(|r| r["lane"] == "pi" && r["tier"] == "budget")
            .unwrap();
        assert_eq!(row["model"], "glm-4.7");
        assert_eq!(row["resolved_lane"], "zai");
        assert_eq!(row["cost"], serde_json::Value::Null);
    }
}
