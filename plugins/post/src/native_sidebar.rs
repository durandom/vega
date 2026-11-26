//! Local hc labels continue to reconcile while the broker is unavailable.
use crate::{native_project::Projects, terminals};
use serde_json::{json, Value};
use std::{path::Path, sync::mpsc, time::Duration};
use vega_herdr::{hc, state, Failure};

pub fn start(projects: Projects) -> mpsc::Sender<()> {
    let (lifetime, stop) = mpsc::channel();
    std::thread::spawn(move || {
        let mut previous_error = None;
        let mut hc = hc::Connection::default();
        let mut health = vega_herdr::daemon_health::Health {
            native: Some(
                vega_herdr::daemon_health::read()
                    .and_then(|h| h.native)
                    .unwrap_or_default(),
            ),
            ..Default::default()
        };
        loop {
            let native = health.native.as_mut().unwrap();
            let inventory = hc.call("terminal.list", json!({})).and_then(|value| {
                let mut agents = terminals::hc_agents(&value)?;
                if let Ok(doc) = hc.call("doc.get", json!({"name":"herdr-app"})) {
                    terminals::apply_spaces(&mut agents, &doc);
                }
                Ok(agents)
            });
            native.inventory.record(
                vega_herdr::capacity::now(),
                inventory.as_ref().err().map(|e| e.message.clone()),
            );
            let result = inventory.and_then(|agents| reconcile(agents, &projects, &mut hc));
            projects.prune();
            health.at = vega_herdr::capacity::now();
            let error = result.err().map(|e| (e.code, e.message));
            native.labels.record(
                health.at,
                error.as_ref().map(|(_, message)| message.clone()),
            );
            if let Some(path) = vega_herdr::daemon_health::path() {
                if let Err(e) = health.write(&path) {
                    eprintln!("sidebar health: {e}");
                }
            }
            if error != previous_error {
                if let Some((code, message)) = &error {
                    eprintln!("sidebar: {code}: {message}");
                }
                previous_error = error;
            }
            if stop.recv_timeout(Duration::from_secs(15)) != Err(mpsc::RecvTimeoutError::Timeout) {
                break;
            }
        }
    });
    lifetime
}

fn reconcile(agents: Vec<Value>, projects: &Projects, hc: &mut hc::Connection) -> Result<(), Failure> {
    let host = crate::host();
    let glyph = vega_herdr::theme::symbols().host_label(&host);
    let claims = state::project_uac_records();
    let uber =
        vega_registry::uber_state::file().and_then(|p| vega_registry::uber_state::local(&p, &host));
    let mut first_error = None;
    for agent in agents {
        let pane = agent["pane_id"].as_str().expect("normalized hc pane");
        let claim = claims.iter().rev().find(|(_, owner, _)| owner == pane);
        let role = if claim.is_some() {
            "uac"
        } else if uber.as_ref().is_some_and(|c| c["pane"] == pane) {
            "uber"
        } else {
            match agent["labels"]["vega.role"].as_str() {
                Some("uac" | "uber" | "") | None => {
                    if state::parent_of_on(None, pane, vega_herdr::agent_name(&agent)).is_some()
                        || agent["parent"].as_str().is_some_and(|p| !p.is_empty())
                    {
                        "sentinel"
                    } else {
                        ""
                    }
                }
                Some(other) => other,
            }
        };
        let project = match claim {
            Some((id, _, _)) => Ok(Some(terminals::project_display_name(id))),
            None => local_project(&agent, projects),
        };
        let mut desired = vec![("vega.host", glyph.clone()), ("vega.role", role.to_owned())];
        match project {
            Ok(Some(label)) => desired.push(("vega.project", label)),
            Ok(None) => {},
            Err(e) => {
                first_error.get_or_insert(e);
            }
        }
        let changed: serde_json::Map<String, Value> = desired
            .into_iter()
            .filter(|(key, value)| agent["labels"][key].as_str().unwrap_or("") != value)
            .map(|(key, value)| (key.into(), value.into()))
            .collect();
        if !changed.is_empty() {
            if let Err(e) = hc.call("terminal.label", json!({"terminal": pane, "labels": changed})) {
                first_error.get_or_insert(e);
            }
        }
    }
    first_error.map_or(Ok(()), Err)
}

/// None means a lookup is in progress. An empty label clears a lost project.
fn local_project(agent: &Value, projects: &Projects) -> Result<Option<String>, Failure> {
    for cwd in [
        agent["cwd"].as_str(),
        agent["space_folder"]
            .as_str()
            .filter(|_| agent["launch_shell"] != true),
    ]
    .into_iter()
    .flatten()
    .filter(|p| !p.is_empty())
    {
        match projects.lookup(Path::new(cwd)).as_deref() {
            Some(Ok(Some(project))) => {
                return Ok(Some(terminals::project_display_name(project.id.as_str())));
            }
            Some(Ok(None)) => {}
            Some(Err(error)) => return Err(Failure::new(&error.code, &error.message, &error.hint)),
            None => return Ok(None),
        }
    }
    Ok(Some(String::new()))
}
