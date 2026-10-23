//! Flag-day registry projection. The startup coordinator journals these plans
//! and owned pin backups, validates every host together, then applies them
//! while all project writers remain stopped. No legacy steady-state reads.
use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use vega_herdr::project::ProjectId;

use crate::{pins, token, uac_key};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Plan {
    host: String,
    /// Persist before applying: rollback must restore even expired/cleared pins.
    pub backup: BTreeMap<String, Value>,
    pub projection: BTreeMap<String, Value>,
}

impl Plan {
    /// `ids` comes from the owning host's local, resolved migration journal.
    /// `migrated` is publication from migrated local state. No remote path IO.
    pub fn prepare(
        host: &str,
        snapshot: &BTreeMap<String, Value>,
        ids: &BTreeMap<String, ProjectId>,
        migrated: BTreeMap<String, Value>,
    ) -> Result<Self, String> {
        let host = token(host);
        let mut plan = Self {
            host: host.clone(),
            backup: BTreeMap::new(),
            projection: BTreeMap::new(),
        };
        for (key, value) in snapshot {
            let parts: Vec<_> = key.split('.').collect();
            if parts.first().copied() != Some(host.as_str())
                || !matches!(parts.get(1), Some(&"uac" | &"lanista"))
                || parts.get(2) == Some(&"v2")
            {
                continue;
            }
            let old = value["project"]
                .as_str()
                .ok_or("legacy project payload missing project")?;
            let id = ids
                .get(old)
                .ok_or_else(|| format!("unresolved owned project {old}"))?;
            let mut v = value.clone();
            v["project"] = serde_json::to_value(id).map_err(|e| e.to_string())?;
            v["schema"] = 2.into();
            let new = if parts[1] == "uac" {
                v["host"] = host.clone().into();
                if !v["folder_name"].is_string() {
                    v["folder_name"] = crate::project_folder(old).into();
                }
                uac_key(&host, id)
            } else {
                let pin: pins::Pin =
                    serde_json::from_value(v.clone()).map_err(|e| e.to_string())?;
                pins::key(&host, id, &pin.seat, &pin.lane, &pin.tier)
            };
            if let Some(prior) = plan.projection.get(&new) {
                if parts[1] == "uac" {
                    merge_claim(prior, &mut v)?;
                } else if prior != &v {
                    return Err(format!("migration collision at {new}"));
                }
            }
            plan.backup.insert(key.clone(), value.clone());
            plan.projection.insert(new, v);
        }
        for (key, mut value) in migrated {
            if !matches!(key.split('.').nth(1), Some("uac" | "lanista")) {
                continue;
            }
            if !key.starts_with(&format!("{host}.")) {
                return Err("foreign projection key".into());
            }
            if let Some(prior) = plan.projection.get(&key) {
                if crate::pins::is_pin(&key) {
                    if prior != &value {
                        return Err(format!("divergent pin at {key}"));
                    }
                } else {
                    merge_claim(prior, &mut value)?;
                }
            }
            plan.projection.insert(key, value);
        }
        plan.validate_local()?;
        Ok(plan)
    }

    fn validate_local(&self) -> Result<(), String> {
        let prefix = format!("{}.", self.host);
        for key in self.backup.keys() {
            let parts: Vec<_> = key.split('.').collect();
            if !key.starts_with(&prefix)
                || !matches!(parts.get(1), Some(&"uac" | &"lanista"))
                || parts.get(2) == Some(&"v2")
            {
                return Err("invalid owned legacy backup key".into());
            }
        }
        for (key, value) in &self.projection {
            if !key.starts_with(&prefix) {
                return Err("foreign projection key".into());
            }
            match key.split('.').nth(1) {
                Some("uac") => {
                    let project = value["project"].as_str().ok_or("missing project")?;
                    if value["schema"] != 2
                        || value["host"] != self.host
                        || value["pane"].as_str().is_none()
                        || *key != crate::uac_key(&self.host, project)
                    {
                        return Err("invalid v2 UAC projection".into());
                    }
                }
                Some("lanista") => {
                    let pin: crate::pins::Pin =
                        serde_json::from_value(value.clone()).map_err(|e| e.to_string())?;
                    if pin.schema != 2
                        || *key
                            != crate::pins::key(
                                &self.host,
                                &pin.project,
                                &pin.seat,
                                &pin.lane,
                                &pin.tier,
                            )
                    {
                        return Err("invalid v2 pin projection".into());
                    }
                }
                // Agents and other migrated local publications have pane/local
                // keys; reconciliation publishes them independently.
                _ => return Err("cutover projection must contain only v2 UACs and pins".into()),
            }
        }
        Ok(())
    }

    pub fn host(&self) -> &str {
        &self.host
    }

    /// Restore exactly the owned legacy payloads, including pin tombstones.
    /// Replay after interruption while both old and new writers remain stopped.
    pub async fn rollback(&self, kv: &async_nats::jetstream::kv::Store) -> Result<(), String> {
        self.validate_local()?;
        for key in self.projection.keys() {
            kv.delete(key).await.map_err(|e| e.to_string())?;
        }
        for (key, value) in &self.backup {
            kv.put(key, value.to_string().into())
                .await
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    pub async fn verify(
        &self,
        client: &async_nats::Client,
        kv: &async_nats::jetstream::kv::Store,
    ) -> Result<(), String> {
        for key in self.backup.keys() {
            if kv.get(key).await.map_err(|e| e.to_string())?.is_some() {
                return Err(format!("legacy key remains: {key}"));
            }
        }
        for (key, value) in &self.projection {
            if crate::get_one(client, key).await?.as_ref() != Some(value) {
                return Err(format!("v2 direct get mismatch: {key}"));
            }
        }
        Ok(())
    }

    /// Exact grant candidates derived from the snapshotted/migrated owner
    /// projection. The broker administrator selects the reader's allowlist;
    /// this function neither grants access nor reads credentials.
    pub fn direct_get_subjects(&self) -> Vec<String> {
        self.projection
            .keys()
            .map(|key| crate::direct_get_subject(key))
            .collect()
    }

    /// Idempotent after interruption: replay the durable backup/plan, never
    /// snapshot the partially deleted registry as a new source of truth.
    /// Caller must first persist plans and call `validate` over all hosts.
    pub async fn apply(&self, kv: &async_nats::jetstream::kv::Store) -> Result<(), String> {
        self.validate_local()?;
        for key in self.backup.keys() {
            kv.delete(key).await.map_err(|e| e.to_string())?;
        }
        for (key, value) in &self.projection {
            kv.put(key, value.to_string().into())
                .await
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    }
}

// Several local locations can retain the same claim; retain every published
// alias and the earliest claim time, but never choose between different panes.
fn merge_claim(prior: &Value, next: &mut Value) -> Result<(), String> {
    if prior["pane"] != next["pane"] || prior["host"] != next["host"] {
        return Err("duplicate claim during migration".into());
    }
    let mut aliases = std::collections::BTreeSet::new();
    for v in [prior, &*next] {
        for field in ["folder_name", "origin_name"] {
            if let Some(alias) = v[field].as_str() {
                aliases.insert(alias.to_owned());
            }
        }
        if let Some(list) = v["aliases"].as_array() {
            aliases.extend(list.iter().filter_map(|a| a.as_str().map(str::to_owned)));
        }
    }
    next["aliases"] = serde_json::json!(aliases);
    if let (Some(a), Some(b)) = (prior["at"].as_i64(), next["at"].as_i64()) {
        next["at"] = a.min(b).into();
    }
    Ok(())
}

/// Migration has no timestamp winner: even expired pins and clears can conflict.
pub fn validate(plans: &[Plan]) -> Result<(), String> {
    let mut claims = BTreeMap::new();
    let mut pins = BTreeMap::new();
    for plan in plans {
        plan.validate_local()?;
        for (key, value) in &plan.projection {
            if key.split('.').nth(1) == Some("uac") {
                let project = value["project"].as_str().ok_or("missing project")?;
                let claim = (
                    plan.host.as_str(),
                    value["pane"].as_str().ok_or("missing pane")?,
                );
                if claims
                    .insert(project, claim)
                    .is_some_and(|prior| prior != claim)
                {
                    return Err(format!("duplicate claim for {project}"));
                }
            } else if crate::pins::is_pin(key) {
                let pin: crate::pins::Pin =
                    serde_json::from_value(value.clone()).map_err(|e| e.to_string())?;
                let slot = (
                    pin.project.clone(),
                    pin.seat.clone(),
                    pin.lane.clone(),
                    pin.tier.clone(),
                );
                if pins
                    .insert(slot, pin.clone())
                    .is_some_and(|prior| prior != pin)
                {
                    return Err(format!("divergent pin for {}", pin.project));
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn id() -> ProjectId {
        serde_json::from_value(json!("github.com/team/repo")).unwrap()
    }
    #[test]
    fn owner_only_backup_preserves_tombstones_and_blocks_collisions() {
        let old = "~/src/repo/.git";
        let pin = json!({"project":old,"seat":"agents","lane":"codex","tier":"budget","model":null,"at":7,"expires":1});
        let snapshot = BTreeMap::from([
            ("a.lanista.legacy".into(), pin.clone()),
            ("b.lanista.legacy".into(), pin.clone()),
        ]);
        let ids = BTreeMap::from([(old.into(), id())]);
        let a = Plan::prepare("a", &snapshot, &ids, BTreeMap::new()).unwrap();
        assert_eq!(a.backup.len(), 1);
        let v = a.projection.values().next().unwrap();
        assert_eq!(v["at"], 7);
        assert_eq!(v["expires"], 1);
        assert!(v["model"].is_null());
        assert!(Plan::prepare("a", &snapshot, &BTreeMap::new(), BTreeMap::new()).is_err());
        let mut b = Plan::prepare("b", &snapshot, &ids, BTreeMap::new()).unwrap();
        validate(&[a.clone(), b.clone()]).unwrap();
        b.projection.values_mut().next().unwrap()["at"] = 8.into();
        assert!(validate(&[a, b]).unwrap_err().contains("divergent pin"));
    }
    #[test]
    fn duplicate_local_claim_keeps_aliases_and_earliest_time_and_pins_cannot_be_overridden() {
        let ids = BTreeMap::from([("/one/.git".into(), id()), ("/two/.bare".into(), id())]);
        let snapshot = BTreeMap::from([
            (
                "a.uac.one".into(),
                json!({"project":"/one/.git","pane":"p1","at":10}),
            ),
            (
                "a.uac.two".into(),
                json!({"project":"/two/.bare","pane":"p1","at":20}),
            ),
        ]);
        let plan = Plan::prepare("a", &snapshot, &ids, BTreeMap::new()).unwrap();
        let claim = plan.projection.values().next().unwrap();
        assert_eq!(claim["at"], 10);
        assert_eq!(claim["aliases"], json!(["one", "two"]));
        assert_eq!(
            plan.direct_get_subjects(),
            vec![crate::direct_get_subject(&crate::uac_key("a", &id()))]
        );
        let snapshot = BTreeMap::from([(
            "a.lanista.old".into(),
            json!({"project":"/one/.git","seat":"s","lane":"l","tier":"t","at":10,"expires":null,"model":"old"}),
        )]);
        let mut migrated = Plan::prepare("a", &snapshot, &ids, BTreeMap::new())
            .unwrap()
            .projection;
        migrated.values_mut().next().unwrap()["model"] = "different".into();
        assert!(Plan::prepare("a", &snapshot, &ids, migrated)
            .unwrap_err()
            .contains("divergent pin"));
    }

    #[test]
    fn duplicate_claims_across_hosts_and_clones_block() {
        let ids = BTreeMap::from([("/a/.git".into(), id()), ("/b/.bare".into(), id())]);
        let snapshot = BTreeMap::from([
            ("a.uac.old".into(), json!({"project":"/a/.git","pane":"p1"})),
            (
                "b.uac.old".into(),
                json!({"project":"/b/.bare","pane":"p2"}),
            ),
        ]);
        let a = Plan::prepare("a", &snapshot, &ids, BTreeMap::new()).unwrap();
        let b = Plan::prepare("b", &snapshot, &ids, BTreeMap::new()).unwrap();
        assert!(validate(&[a, b]).unwrap_err().contains("duplicate claim"));
        let mut same = snapshot;
        same.insert(
            "a.uac.other".into(),
            json!({"project":"/b/.bare","pane":"p2"}),
        );
        assert!(Plan::prepare("a", &same, &ids, BTreeMap::new()).is_err());
    }
}
