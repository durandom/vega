//! Wait mutations: local ownership facts or the existing authenticated post command
//! channel. A host asserts its own harness session; this is not a process sandbox.
use serde_json::Value;
use std::{io::Read, time::Duration};
use vega_herdr::{
    state,
    waits::{self, Actor, Change, Endpoint, Proof, Receipt, Request, Store},
    Failure,
};

fn fail(code: &str, message: &str) -> Failure {
    Failure::new(
        code,
        message,
        "vega orchestration waiting --help; inspect the coordinator and its waiter relationship",
    )
}
fn host() -> String {
    vega_herdr::hosts::key(&state::post_host())
}
fn agent(pane: &str) -> Option<Value> {
    vega_herdr::herdr_on(None, &["agent", "get", pane], Duration::from_secs(2))
        .ok()
        .and_then(|v| v["result"].get("agent").cloned())
}
fn local_endpoint(pane: &str) -> Result<Endpoint, Failure> {
    let a = agent(pane).ok_or_else(|| fail("wait_identity", "local agent identity is unknown"))?;
    if a["pane_id"].as_str() != Some(pane) {
        return Err(fail(
            "wait_identity",
            "local agent identity does not match this pane",
        ));
    }
    Ok(Endpoint {
        host: host(),
        pane: pane.into(),
        name: vega_herdr::agent_name(&a).into(),
    })
}
pub fn caller() -> Result<Actor, Failure> {
    let pane = std::env::var("HERDR_PANE_ID")
        .ok()
        .filter(|p| vega_shim::is_pane(p))
        .ok_or_else(|| fail("wait_identity", "a herdr agent pane is required"))?;
    let session = ["CODEX_THREAD_ID", "CLAUDE_CODE_SESSION_ID", "PI_SESSION_ID"]
        .into_iter()
        .find_map(|k| std::env::var(k).ok().filter(|s| !s.is_empty()))
        .ok_or_else(|| fail("wait_identity", "harness session identity is unavailable"))?;
    Ok(Actor {
        endpoint: local_endpoint(&pane)?,
        session,
    })
}
pub fn coordinator(actor: &Actor, explicit: Option<&str>) -> Result<Endpoint, Failure> {
    let (pane, h) = if let Some(address) = explicit {
        let (p, h) = address
            .split_once('@')
            .ok_or_else(|| fail("usage", "--coordinator must be pane@host"))?;
        if !vega_shim::is_pane(p) || h.is_empty() {
            return Err(fail("usage", "--coordinator must be pane@host"));
        }
        (p.into(), vega_herdr::hosts::key(h))
    } else if let Some(p) = state::parent_of(&actor.endpoint.pane, &actor.endpoint.name) {
        (p.pane, vega_herdr::hosts::key(&p.host))
    } else {
        return Ok(actor.endpoint.clone());
    };
    if h == host() {
        return local_endpoint(&pane);
    }
    let view = vega_registry::read_blocking(Duration::from_secs(3)).map_err(|_| {
        fail(
            "wait_coordinator",
            "coordinator identity is unknown (registry unavailable)",
        )
    })?;
    let a = view.agent(&h, &pane).ok_or_else(|| {
        fail(
            "wait_coordinator",
            "coordinator identity is unknown or stale",
        )
    })?;
    Ok(Endpoint {
        host: h,
        pane,
        name: a["name"].as_str().unwrap_or("").into(),
    })
}
fn known(endpoint: &Endpoint, view: Option<&vega_registry::View>) -> bool {
    if endpoint.host == host() {
        return local_endpoint(&endpoint.pane).is_ok_and(|e| e == *endpoint);
    }
    view.and_then(|v| v.agent(&endpoint.host, &endpoint.pane))
        .is_some_and(|a| a["name"].as_str() == Some(endpoint.name.as_str()))
}
fn child_of(actor: &Actor, coordinator: &Endpoint, machines: &Value) -> bool {
    let here = host();
    lineage(&state::children(), actor, coordinator, machines, &here)
}
fn lineage(
    children: &[state::Child],
    actor: &Actor,
    coordinator: &Endpoint,
    machines: &Value,
    here: &str,
) -> bool {
    children.iter().any(|c| {
        c.pane == actor.endpoint.pane
            && c.name == actor.endpoint.name
            && c.parent.pane == coordinator.pane
            && vega_herdr::hosts::key(&c.parent.host) == coordinator.host
            && match &c.machine {
                None => actor.endpoint.host == here,
                Some(m) => {
                    vega_herdr::hosts::registry_for_machine(m, machines, here).as_deref()
                        == Some(actor.endpoint.host.as_str())
                }
            }
    })
}

pub fn apply(from: &str, req: &Request) -> Result<Receipt, Failure> {
    // Reject host impersonation before looking up agents or opening local state.
    if req.version != 1 || req.actor.endpoint.host != from {
        return Err(fail(
            "wait_identity",
            "authenticated sender host does not own the waiter",
        ));
    }
    let here = host();
    let registry = if from != here {
        Some(
            vega_registry::read_blocking(Duration::from_secs(3)).map_err(|_| {
                fail(
                    "wait_identity",
                    "remote waiter identity is unknown (registry unavailable)",
                )
            })?,
        )
    } else {
        None
    };
    let actor_known = known(&req.actor.endpoint, registry.as_ref());
    if !actor_known {
        return Err(fail(
            "wait_identity",
            "waiter identity is unknown, stale or reused",
        ));
    }
    let mut proof = Proof {
        actor_known,
        ..Proof::default()
    };
    if let Change::Waiting {
        coordinator, on, ..
    } = &req.change
    {
        validate_owner_shape(on)?;
        if from == here {
            validate_on(on, &req.actor)?;
        }
        if coordinator.host != here {
            return Err(fail(
                "wait_coordinator",
                "request reached a different coordinator host",
            ));
        }
        proof.coordinator_known = known(coordinator, registry.as_ref());
        let machines = if from != here {
            vega_herdr::herdr_on(None, &["machine", "list", "--json"], Duration::from_secs(1))
                .unwrap_or(Value::Null)
        } else {
            Value::Null
        };
        proof.child_of_coordinator = child_of(&req.actor, coordinator, &machines);
        if !proof.coordinator_known
            || (req.actor.endpoint != *coordinator && !proof.child_of_coordinator)
        {
            return Err(fail(
                "wait_coordinator",
                "coordinator ownership of this waiter is unverified",
            ));
        }
    }
    let file =
        waits::path().ok_or_else(|| fail("wait_state", "local state directory is unavailable"))?;
    Store::open(&file)?.apply(&here, from, req, proof, vega_ci_watch::unix_now())
}
pub fn submit(to: &str, req: &Request) -> Result<Receipt, Failure> {
    if to == host() {
        return apply(&host(), req);
    }
    let payload = serde_json::to_vec(req).map_err(|_| fail("usage", "invalid wait request"))?;
    if payload.len() > 4096 {
        return Err(fail(
            "usage",
            "remote wait request exceeds 4 KiB; shorten the reason or evidence",
        ));
    }
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|_| fail("remote_wait", "remote wait transport unavailable"))?;
    rt.block_on(async {
        tokio::time::timeout(Duration::from_secs(14), async {
            let client = vega_registry::connect("orchestration wait")
                .await
                .map_err(|_| {
                    fail(
                        "remote_wait",
                        "wait coordinator is unreachable; update not confirmed",
                    )
                })?;
            let subject = format!("vega.cmd.{to}.orchestration.{}", host());
            let request = async_nats::Request::new()
                .payload(payload.into())
                .timeout(Some(Duration::from_secs(11)));
            let reply = client.send_request(subject, request).await.map_err(|_| {
                fail(
                    "remote_wait",
                    "wait coordinator did not confirm the update; inspect before retrying",
                )
            })?;
            let envelope: Value = serde_json::from_slice(&reply.payload).map_err(|_| {
                fail(
                    "remote_wait",
                    "invalid coordinator reply; update not confirmed",
                )
            })?;
            if envelope["ok"] != true {
                return Err(fail(
                    "remote_wait",
                    "coordinator refused the wait update; inspect ownership and local state",
                ));
            }
            let receipt: Receipt = serde_json::from_str(envelope["message"].as_str().unwrap_or(""))
                .map_err(|_| {
                    fail(
                        "remote_wait",
                        "invalid coordinator receipt; inspect before retrying",
                    )
                })?;
            if waits::id_host(&receipt.id) != Some(to) {
                return Err(fail(
                    "remote_wait",
                    "coordinator receipt has the wrong owner host",
                ));
            }
            let valid_receipt = match &req.change {
                Change::Waiting { .. } => {
                    matches!(receipt.status.as_str(), "waiting" | "already_waiting")
                }
                Change::Clear { id, .. } => {
                    &receipt.id == id
                        && matches!(receipt.status.as_str(), "cleared" | "already_cleared")
                }
            };
            if !valid_receipt {
                return Err(fail(
                    "remote_wait",
                    "coordinator returned an unexpected receipt; inspect the stored wait",
                ));
            }
            Ok(receipt)
        })
        .await
        .map_err(|_| {
            fail(
                "remote_wait",
                "wait update timed out; inspect before retrying the same request",
            )
        })?
    })
}
pub fn remote(from: &str) -> Result<(), Failure> {
    let mut bytes = Vec::new();
    std::io::stdin()
        .take(4097)
        .read_to_end(&mut bytes)
        .map_err(|_| fail("usage", "cannot read wait update"))?;
    if bytes.len() > 4096 {
        return Err(fail("usage", "wait update exceeds 4 KiB"));
    }
    let req: Request =
        serde_json::from_slice(&bytes).map_err(|_| fail("usage", "invalid wait update schema"))?;
    let receipt = apply(from, &req)?;
    println!(
        "{}",
        serde_json::to_string(&receipt)
            .map_err(|_| fail("wait_state", "cannot encode wait receipt"))?
    );
    Ok(())
}
pub fn validate_on(on: &str, actor: &Actor) -> Result<(), Failure> {
    validate_owner_shape(on)?;
    if on == "Slayer" {
        return Ok(());
    }
    if let Some(reference) = on.strip_prefix("ci:") {
        let (target, head) = reference
            .rsplit_once('@')
            .ok_or_else(|| fail("usage", "CI wait requires ci:<watch-target>@<head>"))?;
        if head.is_empty()
            || !vega_ci_watch::store::watches().iter().any(|w| {
                w.pane == actor.endpoint.pane
                    && w.target.label() == target
                    && vega_ci_watch::store::load_state(&w.target)
                        .is_some_and(|s| s.head.as_deref() == Some(head))
            })
        {
            return Err(fail(
                "wait_ci",
                "no existing CI watch receipt matches this pane, target and head",
            ));
        }
        return Ok(());
    }
    if on.split_once('@').is_some_and(|(p, h)| {
        vega_shim::is_pane(p) && !h.is_empty() && h == vega_herdr::hosts::key(h)
    }) {
        return Ok(());
    }
    Err(fail(
        "usage",
        "--on must be Slayer, pane@host, or ci:<watch-target>@<head>",
    ))
}

pub fn validate_owner_shape(on: &str) -> Result<(), Failure> {
    if on == "Slayer" {
        return Ok(());
    }
    if let Some(reference) = on.strip_prefix("ci:") {
        if reference
            .rsplit_once('@')
            .is_some_and(|(target, head)| !target.is_empty() && !head.is_empty())
        {
            return Ok(());
        }
    } else if on.split_once('@').is_some_and(|(p, h)| {
        vega_shim::is_pane(p) && !h.is_empty() && h == vega_herdr::hosts::key(h)
    }) {
        return Ok(());
    }
    Err(fail(
        "usage",
        "--on must be Slayer, pane@host, or ci:<watch-target>@<head>",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn remote_lineage_uses_actual_host_and_current_name_without_id_joins() {
        let actor = Actor {
            endpoint: Endpoint {
                host: "io-wsl".into(),
                pane: "w1:p1".into(),
                name: "child".into(),
            },
            session: "s".into(),
        };
        let owner = Endpoint {
            host: "here".into(),
            pane: "w2:p1".into(),
            name: "coordinator".into(),
        };
        let children = vec![state::Child {
            machine: Some("io".into()),
            pane: "w1:p1".into(),
            name: "child".into(),
            parent: state::Parent {
                host: "here".into(),
                pane: "w2:p1".into(),
            },
        }];
        let machines = json!([{"label":"io","target":"ssh://me@io-wsl:22"}]);
        assert!(lineage(&children, &actor, &owner, &machines, "here"));
        assert!(!lineage(&children, &actor, &owner, &Value::Null, "here"));
        let mut reused = actor.clone();
        reused.endpoint.name = "new-child".into();
        assert!(!lineage(&children, &reused, &owner, &machines, "here"));
        let mut foreign = actor;
        foreign.endpoint.host = "other".into();
        assert!(!lineage(&children, &foreign, &owner, &machines, "here"));
    }
}
