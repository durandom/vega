//! Project model overrides. Hosts write only their own namespace; the latest
//! timestamp (host token breaks ties) wins, including clears and expired pins.
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::{token, View};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Pin {
    pub project: vega_herdr::project::ProjectId,
    pub schema: u8,
    pub seat: String,
    pub lane: String,
    pub tier: String,
    pub model: Option<String>,
    /// Unix nanoseconds; not changed by expiry pruning.
    pub at: u64,
    pub expires: Option<i64>,
}

impl Pin {
    pub fn active(&self, now: i64) -> Option<&str> {
        self.expires.is_none_or(|t| t > now).then_some(())?;
        self.model.as_deref()
    }
}

use vega_herdr::project::encode_component as hex;

/// Hex components preserve project identity (unlike lossy registry tokens).
pub fn key(host: &str, project: &str, seat: &str, lane: &str, tier: &str) -> String {
    format!(
        "{}.lanista.v2.{}.{}.{}.{}",
        token(host),
        hex(project),
        hex(seat),
        hex(lane),
        hex(tier)
    )
}

pub fn is_pin(key: &str) -> bool {
    key.split('.').nth(1) == Some("lanista") && key.split('.').nth(2) == Some("v2")
}

/// Expired records still compete: an older host's pin must never resurrect.
pub fn latest(
    view: &View,
    project: &str,
    seat: &str,
    lane: &str,
    tier: &str,
) -> Option<(String, Pin)> {
    view.entries
        .iter()
        .filter_map(|(k, v)| {
            if !is_pin(k) {
                return None;
            }
            let host = k.split('.').next()?;
            let pin: Pin = serde_json::from_value(v.clone()).ok()?;
            (pin.schema == 2
                && pin.project.as_str() == project
                && pin.seat == seat
                && pin.lane == lane
                && pin.tier == tier
                && *k == key(host, project, seat, lane, tier))
            .then(|| (host.to_string(), pin))
        })
        .max_by(|(ah, a), (bh, b)| (a.at, ah).cmp(&(b.at, bh)))
}

pub fn timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64
}

/// `Some(None)` clears; `Some(Some(model))` sets; `None` resolves only.
/// Reads fail as a unit rather than claiming a partial view is authoritative.
pub fn access(pin: Pin, change: Option<Option<String>>) -> Result<View, String> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;
    rt.block_on(async {
        tokio::time::timeout(Duration::from_secs(5), async {
            let client = crate::connect("lanista pin").await?;
            let hosts = std::env::var("VEGA_LANISTA_PIN_HOSTS").ok().map(|s| {
                s.split(',')
                    .map(str::trim)
                    .filter(|h| !h.is_empty())
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            });
            access_with(
                &client,
                &vega_herdr::state::post_host(),
                pin,
                change,
                hosts.as_deref(),
                crate::unix_now(),
            )
            .await
        })
        .await
        .map_err(|_| "project pin registry timed out".to_string())?
    })
}

/// The same resolver on an existing connection (also used by broker tests).
/// `reader_hosts` limits discovery to exact keys granted to an inbox-only host.
pub async fn access_with(
    client: &async_nats::Client,
    host: &str,
    mut pin: Pin,
    change: Option<Option<String>>,
    reader_hosts: Option<&[String]>,
    now: i64,
) -> Result<View, String> {
    if pin.schema != 2 {
        return Err("project pin requires schema 2".into());
    }
    let host = token(host);
    let mut view = View::default();
    // Inbox-only hosts cannot enumerate. An explicit writer list allows
    // exact direct gets when the broker grants those individual keys.
    if let Some(hosts) = reader_hosts {
        for h in hosts {
            let k = key(h, &pin.project, &pin.seat, &pin.lane, &pin.tier);
            if let Some(v) = crate::get_one(client, &k).await? {
                view.entries.insert(k, v);
            }
        }
    } else {
        let kv = crate::bucket(client).await?;
        // Enumerate without fetching: read_kinds intentionally tolerates
        // individual get errors, whereas this resolver must not.
        let listed = crate::read_kinds(client, &kv, Some(&[])).await?;
        for k in listed.keys.iter().filter(|k| is_pin(k)) {
            if let Some(v) = crate::get_one(client, k).await? {
                view.entries.insert(k.clone(), v);
            }
        }
    }
    let k = key(&host, &pin.project, &pin.seat, &pin.lane, &pin.tier);
    if let Some(model) = change {
        let kv = crate::bucket(client).await?;
        // Include our key even with an explicit reader list.
        if let Some(v) = crate::get_one(client, &k).await? {
            view.entries.insert(k.clone(), v);
        }
        let prior =
            latest(&view, &pin.project, &pin.seat, &pin.lane, &pin.tier).map_or(0, |(_, p)| p.at);
        pin.at = timestamp().max(prior.saturating_add(1));
        pin.model = model;
        let value = serde_json::to_value(&pin).map_err(|e| e.to_string())?;
        kv.put(&k, value.to_string().into())
            .await
            .map_err(|e| e.to_string())?;
        view.entries.insert(k, value);
    } else {
        // Prune the expired payload on this writer only, retaining its
        // timestamp as a tombstone. CAS never overwrites a concurrent set.

        if view
            .entries
            .get(&k)
            .and_then(|v| serde_json::from_value::<Pin>(v.clone()).ok())
            .is_some_and(|p| p.model.is_some() && p.expires.is_some_and(|t| t <= now))
        {
            if let Ok(kv) = crate::bucket(client).await {
                if let Ok(Some(entry)) = kv.entry(&k).await {
                    if let Ok(mut p) = serde_json::from_slice::<Pin>(&entry.value) {
                        if p.model.is_some() && p.expires.is_some_and(|t| t <= now) {
                            p.model = None;
                            let _ = kv
                                .update(&k, serde_json::to_vec(&p).unwrap().into(), entry.revision)
                                .await;
                        }
                    }
                }
            }
        }
    }
    Ok::<_, String>(view)
}

/// Default expiry is the binding account window's reset; unknown/stale
/// capacity requires an explicit duration or `never` from the caller.
pub fn reset(sample: &vega_herdr::capacity::Sample, lane: &str, now: i64) -> Result<i64, String> {
    let provider = if lane == "agy" { "antigravity" } else { lane };
    let h = vega_herdr::capacity::for_provider(sample, provider, now)
        .ok_or_else(|| format!("no {provider} account quota window"))?;
    if h.age > vega_herdr::capacity::STALE_SECS {
        return Err("capacity sample is stale".into());
    }
    h.resets_in
        .and_then(|s| now.checked_add(s))
        .ok_or_else(|| "next quota reset is unknown".into())
}

pub fn expiry(until: &str, now: i64) -> Result<Option<i64>, String> {
    if until == "never" {
        return Ok(None);
    }
    let split = until
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(until.len());
    let (n, unit) = until.split_at(split);
    let n: i64 = n
        .parse()
        .map_err(|_| "want a duration such as 30m, 2h, 1d, or never".to_string())?;
    let factor = match unit {
        "s" => 1,
        "m" => 60,
        "h" => 3600,
        "d" => 86400,
        _ => return Err("duration needs s, m, h or d".into()),
    };
    n.checked_mul(factor)
        .filter(|s| *s > 0)
        .and_then(|s| now.checked_add(s))
        .map(Some)
        .ok_or_else(|| "duration must be positive and fit in Unix seconds".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn pin(project: &str, model: Option<&str>, at: u64, expires: Option<i64>) -> Pin {
        Pin {
            schema: 2,
            project: serde_json::from_value(serde_json::json!(project)).unwrap(),
            seat: "agents".into(),
            lane: "codex".into(),
            tier: "budget".into(),
            model: model.map(str::to_string),
            at,
            expires,
        }
    }
    fn put(view: &mut View, host: &str, p: Pin) {
        view.entries.insert(
            key(host, &p.project, &p.seat, &p.lane, &p.tier),
            serde_json::to_value(p).unwrap(),
        );
    }
    fn get(view: &View, project: &str, now: i64) -> Option<String> {
        latest(view, project, "agents", "codex", "budget")
            .and_then(|(_, p)| p.active(now).map(str::to_string))
    }
    #[test]
    fn set_cross_host_project_isolation_expire_clear_and_tie() {
        let mut v = View::default();
        put(
            &mut v,
            "io",
            pin("github.com/team/repo", Some("old"), 1, None),
        );
        put(
            &mut v,
            "proteus",
            pin("github.com/team/repo", Some("new"), 2, Some(100)),
        );
        assert_eq!(get(&v, "github.com/team/repo", 99).as_deref(), Some("new"));
        assert_eq!(
            latest(&v, "github.com/team/repo", "agents", "codex", "budget")
                .unwrap()
                .0,
            "proteus"
        );
        assert_eq!(get(&v, "github.com/team/other", 99), None);
        assert_eq!(get(&v, "github.com/team/repo", 100), None);
        put(&mut v, "io", pin("github.com/team/repo", None, 3, None));
        assert_eq!(get(&v, "github.com/team/repo", 99), None);
        put(
            &mut v,
            "proteus",
            pin("github.com/team/repo", Some("tie"), 3, None),
        );
        assert_eq!(get(&v, "github.com/team/repo", 99).as_deref(), Some("tie"));
        let mut pruned = pin("github.com/team/repo", None, 3, Some(100));
        put(&mut v, "proteus", pruned.clone());
        assert_eq!(get(&v, "github.com/team/repo", 101), None);
        pruned.model = Some("unpruned".into());
        put(&mut v, "proteus", pruned);
        assert_eq!(get(&v, "github.com/team/repo", 101), None);
        assert_ne!(
            key("io", "a/b", "s", "x", "t"),
            key("io", "a_b", "s", "x", "t")
        );
    }
    #[test]
    fn durations_are_bounded_and_explicit() {
        assert_eq!(expiry("never", 10), Ok(None));
        assert_eq!(expiry("2h", 10), Ok(Some(7210)));
        for s in [
            "0s",
            "-1h",
            "20",
            "1w",
            "9999999999999999999d",
            "9223372036854775807d",
        ] {
            assert!(expiry(s, 10).is_err(), "{s}");
        }
    }
    #[test]
    fn expiry_uses_binding_window_and_rejects_unknown_stale_or_passed_reset() {
        use vega_herdr::capacity::{Reading, Sample, Scope, STALE_SECS};
        let row = |left, reset| Reading {
            provider: "antigravity".into(),
            window: "test".into(),
            scope: Scope::Account,
            left,
            reset_at: Some(reset),
            runs_out_at: None,
            observed_at: Some(10),
        };
        let sample = Sample {
            readings: vec![row(80.0, 500), row(5.0, 1000)],
        };
        assert_eq!(reset(&sample, "agy", 20), Ok(1000));
        assert!(reset(&sample, "codex", 20).is_err());
        assert!(reset(&sample, "agy", 10 + STALE_SECS + 1).is_err());
        assert!(reset(&sample, "agy", 1001).is_err());
    }
}
