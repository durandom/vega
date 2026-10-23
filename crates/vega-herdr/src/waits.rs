//! Explicit waits, owned by a coordinator host. Runtime state only; no project ID
//! protocol, task DAG, approvals or inbox read acknowledgements enter this ledger.
use crate::Failure;
use rusqlite::{Connection, OpenFlags, TransactionBehavior};
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    time::Duration,
};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Endpoint {
    pub host: String,
    pub pane: String,
    pub name: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Actor {
    pub endpoint: Endpoint,
    pub session: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Wait {
    pub version: u8,
    pub id: String,
    pub waiter: Actor,
    pub coordinator: Endpoint,
    pub on: String,
    pub reason: String,
    pub bead: Option<String>,
    pub thread: Option<String>,
    pub created: i64,
    pub resolved: Option<i64>,
    pub evidence: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum Change {
    Waiting {
        coordinator: Endpoint,
        on: String,
        reason: String,
        bead: Option<String>,
        thread: Option<String>,
    },
    Clear {
        id: String,
        evidence: String,
    },
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub version: u8,
    pub actor: Actor,
    pub change: Change,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Receipt {
    pub id: String,
    pub status: String,
}

/// Facts checked by the coordinator against live agents and its own lineage.
#[derive(Clone, Copy, Default)]
pub struct Proof {
    pub actor_known: bool,
    pub coordinator_known: bool,
    pub child_of_coordinator: bool,
}

pub fn path() -> Option<PathBuf> {
    crate::state::state_file("orchestration", "x")?
        .parent()
        .map(|p| p.join("waits.sqlite3"))
}
fn failure(code: &str, message: &str) -> Failure {
    Failure::new(
        code,
        message,
        "vega orchestration --help; inspect the coordinator's local wait state before retrying",
    )
}
fn storage(_: impl std::fmt::Display) -> Failure {
    failure(
        "wait_state",
        "explicit wait state is unavailable or invalid; no wait was changed",
    )
}
fn pane(p: &str) -> bool {
    p.split_once(':').is_some_and(|(a, b)| {
        !a.is_empty()
            && !b.is_empty()
            && a.chars().all(|c| c.is_ascii_alphanumeric())
            && b.chars().all(|c| c.is_ascii_alphanumeric())
    })
}
fn endpoint(e: &Endpoint) -> bool {
    !e.host.is_empty()
        && e.host == crate::hosts::key(&e.host)
        && pane(&e.pane)
        && e.name.len() <= 256
}
pub fn id_host(id: &str) -> Option<&str> {
    let (host, hex) = id.split_once(':')?;
    (!host.is_empty()
        && host == crate::hosts::key(host)
        && hex.len() == 32
        && hex.chars().all(|c| c.is_ascii_hexdigit()))
    .then_some(host)
}
pub fn valid(w: &Wait) -> bool {
    w.version == 1
        && endpoint(&w.waiter.endpoint)
        && endpoint(&w.coordinator)
        && !w.waiter.session.is_empty()
        && id_host(&w.id) == Some(w.coordinator.host.as_str())
        && !w.on.trim().is_empty()
        && !w.reason.trim().is_empty()
        && (w.resolved.is_some() == w.evidence.is_some())
}

pub struct Store {
    conn: Connection,
}
impl Store {
    pub fn open(file: &Path) -> Result<Self, Failure> {
        std::fs::create_dir_all(file.parent().ok_or_else(|| storage("path"))?).map_err(storage)?;
        let conn = Connection::open(file).map_err(storage)?;
        conn.busy_timeout(Duration::from_secs(3)).map_err(storage)?;
        let version: i64 = conn
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .map_err(storage)?;
        if version > 1 {
            return Err(storage("schema"));
        }
        conn.execute_batch("PRAGMA journal_mode=WAL; CREATE TABLE IF NOT EXISTS waits (id TEXT PRIMARY KEY, record TEXT NOT NULL); PRAGMA user_version=1;").map_err(storage)?;
        Ok(Self { conn })
    }
    pub fn read(file: &Path) -> Result<Vec<Wait>, Failure> {
        let conn =
            Connection::open_with_flags(file, OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(storage)?;
        conn.busy_timeout(Duration::from_secs(3)).map_err(storage)?;
        let version: i64 = conn
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .map_err(storage)?;
        if version != 1 {
            return Err(storage("schema"));
        }
        records(&conn)
    }
    pub fn get(&self, id: &str) -> Result<Option<Wait>, Failure> {
        Ok(records(&self.conn)?.into_iter().find(|w| w.id == id))
    }
    pub fn apply(
        &mut self,
        server: &str,
        from: &str,
        req: &Request,
        proof: Proof,
        now: i64,
    ) -> Result<Receipt, Failure> {
        if req.version != 1
            || !endpoint(&req.actor.endpoint)
            || req.actor.session.is_empty()
            || req.actor.endpoint.host != from
            || !proof.actor_known
        {
            return Err(failure(
                "wait_identity",
                "sender does not own a verified current waiter identity",
            ));
        }
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage)?;
        let all = records(&tx)?;
        let receipt = match &req.change {
            Change::Waiting {
                coordinator,
                on,
                reason,
                bead,
                thread,
            } => {
                if !endpoint(coordinator)
                    || coordinator.host != server
                    || !proof.coordinator_known
                    || (coordinator != &req.actor.endpoint && !proof.child_of_coordinator)
                {
                    return Err(failure(
                        "wait_coordinator",
                        "the coordinator does not own this waiter",
                    ));
                }
                if on.trim().is_empty() || reason.trim().is_empty() {
                    return Err(failure("usage", "--on and --for must be nonempty"));
                }
                if let Some(w) = all.iter().find(|w| {
                    w.resolved.is_none()
                        && w.waiter == req.actor
                        && w.coordinator == *coordinator
                        && w.bead == *bead
                        && w.thread == *thread
                }) {
                    if w.on != *on || w.reason != *reason {
                        return Err(failure("wait_exists","this waiter already has a different wait in this bead/thread; clear it with evidence first"));
                    }
                    Receipt {
                        id: w.id.clone(),
                        status: "already_waiting".into(),
                    }
                } else {
                    let hex: String = tx
                        .query_row("SELECT lower(hex(randomblob(16)))", [], |r| r.get(0))
                        .map_err(storage)?;
                    let w = Wait {
                        version: 1,
                        id: format!("{server}:{hex}"),
                        waiter: req.actor.clone(),
                        coordinator: coordinator.clone(),
                        on: on.clone(),
                        reason: reason.clone(),
                        bead: bead.clone(),
                        thread: thread.clone(),
                        created: now,
                        resolved: None,
                        evidence: None,
                    };
                    save(&tx, &w)?;
                    Receipt {
                        id: w.id,
                        status: "waiting".into(),
                    }
                }
            }
            Change::Clear { id, evidence } => {
                if evidence.trim().is_empty() {
                    return Err(failure("usage", "--evidence must be nonempty"));
                }
                let mut w = all.into_iter().find(|w| w.id == *id).ok_or_else(|| {
                    failure("unknown_wait", "wait ID is unknown on this coordinator")
                })?;
                if w.coordinator.host != server
                    || (req.actor != w.waiter && req.actor.endpoint != w.coordinator)
                {
                    return Err(failure(
                        "wait_identity",
                        "only the waiter session or its recorded coordinator may clear this wait",
                    ));
                }
                let status = if w.resolved.is_some() {
                    "already_cleared"
                } else {
                    w.resolved = Some(now);
                    w.evidence = Some(evidence.clone());
                    save(&tx, &w)?;
                    "cleared"
                };
                Receipt {
                    id: id.clone(),
                    status: status.into(),
                }
            }
        };
        tx.commit().map_err(storage)?;
        Ok(receipt)
    }
}
fn records(conn: &Connection) -> Result<Vec<Wait>, Failure> {
    let mut stmt = conn
        .prepare("SELECT id,record FROM waits ORDER BY id")
        .map_err(storage)?;
    let rows = stmt
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
        .map_err(storage)?;
    let mut out = Vec::new();
    for row in rows {
        let (id, text) = row.map_err(storage)?;
        let w: Wait = serde_json::from_str(&text).map_err(storage)?;
        if !valid(&w) || w.id != id {
            return Err(storage("record"));
        }
        out.push(w);
    }
    Ok(out)
}
fn save(conn: &Connection, w: &Wait) -> Result<(), Failure> {
    conn.execute("INSERT INTO waits(id,record) VALUES (?1,?2) ON CONFLICT(id) DO UPDATE SET record=excluded.record",rusqlite::params![w.id,serde_json::to_string(w).map_err(storage)?]).map_err(storage)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn endpoint_(host: &str, pane: &str) -> Endpoint {
        Endpoint {
            host: host.into(),
            pane: pane.into(),
            name: pane.into(),
        }
    }
    fn request() -> Request {
        Request {
            version: 1,
            actor: Actor {
                endpoint: endpoint_("worker", "w1:p1"),
                session: "s1".into(),
            },
            change: Change::Waiting {
                coordinator: endpoint_("owner", "w2:p1"),
                on: "Slayer".into(),
                reason: "decision pending".into(),
                bead: Some("b-1".into()),
                thread: None,
            },
        }
    }
    fn proof() -> Proof {
        Proof {
            actor_known: true,
            coordinator_known: true,
            child_of_coordinator: true,
        }
    }
    #[test]
    fn restart_idempotence_explicit_evidence_and_session_fencing() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("waits.sqlite3");
        assert!(Store::read(&file).is_err());
        let mut store = Store::open(&file).unwrap();
        let req = request();
        let first = store.apply("owner", "worker", &req, proof(), 100).unwrap();
        let again = store.apply("owner", "worker", &req, proof(), 200).unwrap();
        assert_eq!(first.id, again.id);
        drop(store);
        let rows = Store::read(&file).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].created, 100);
        assert!(rows[0].resolved.is_none());
        let mut store = Store::open(&file).unwrap();
        let mut clear = Request {
            version: 1,
            actor: req.actor.clone(),
            change: Change::Clear {
                id: first.id.clone(),
                evidence: "inspected answer".into(),
            },
        };
        clear.actor.session = "reused-session".into();
        assert!(store
            .apply("owner", "worker", &clear, proof(), 250)
            .is_err());
        clear.actor = req.actor.clone();
        assert_eq!(
            store
                .apply("owner", "worker", &clear, proof(), 300)
                .unwrap()
                .status,
            "cleared"
        );
        clear.change = Change::Clear {
            id: first.id,
            evidence: "different repeat".into(),
        };
        assert_eq!(
            store
                .apply("owner", "worker", &clear, proof(), 400)
                .unwrap()
                .status,
            "already_cleared"
        );
        let rows = Store::read(&file).unwrap();
        assert_eq!(rows[0].resolved, Some(300));
        assert_eq!(rows[0].evidence.as_deref(), Some("inspected answer"));
    }
    #[test]
    fn remote_sender_and_lineage_are_required_before_mutation() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(&dir.path().join("db")).unwrap();
        let req = request();
        assert!(store.apply("owner", "impostor", &req, proof(), 0).is_err());
        assert!(store
            .apply(
                "owner",
                "worker",
                &req,
                Proof {
                    child_of_coordinator: false,
                    ..proof()
                },
                0
            )
            .is_err());
        assert!(store
            .apply(
                "owner",
                "worker",
                &req,
                Proof {
                    actor_known: false,
                    ..proof()
                },
                0
            )
            .is_err());
        assert!(Store::read(&dir.path().join("db")).unwrap().is_empty());
        let id = store.apply("owner", "worker", &req, proof(), 1).unwrap().id;
        let clear = Request {
            version: 1,
            actor: Actor {
                endpoint: endpoint_("owner", "w2:p1"),
                session: "coordinator-session".into(),
            },
            change: Change::Clear {
                id,
                evidence: "reviewed evidence".into(),
            },
        };
        assert_eq!(
            store
                .apply("owner", "owner", &clear, proof(), 2)
                .unwrap()
                .status,
            "cleared"
        );
    }
    #[test]
    fn concurrent_retries_return_one_stable_wait() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("db");
        drop(Store::open(&file).unwrap());
        let ids = std::thread::scope(|s| {
            let handles: Vec<_> = (0..2)
                .map(|_| {
                    let f = &file;
                    s.spawn(move || {
                        Store::open(f)
                            .unwrap()
                            .apply("owner", "worker", &request(), proof(), 1)
                            .unwrap()
                            .id
                    })
                })
                .collect();
            handles
                .into_iter()
                .map(|h| h.join().unwrap())
                .collect::<Vec<_>>()
        });
        assert_eq!(ids[0], ids[1]);
        assert_eq!(Store::read(&file).unwrap().len(), 1);
    }
}
