//! Local hc labels continue to reconcile while the broker is unavailable.
use serde_json::Value;
use std::{path::Path, sync::mpsc, time::Duration};
use vega_herdr::{project, state, Failure};

pub fn start() -> mpsc::Sender<()> {
    let (lifetime, stop) = mpsc::channel();
    std::thread::spawn(move || {
        let mut previous_error = None;
        loop {
            let error = reconcile().err().map(|e| (e.code, e.message));
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

fn reconcile() -> Result<(), Failure> {
    let agents = crate::terminals::hc_list()?;
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
            Some((id, _, _)) => Ok(Some(id.clone())),
            None => local_project(&agent, &host),
        };
        let mut desired = vec![("vega.host", glyph.clone()), ("vega.role", role.to_owned())];
        match project {
            Ok(id) => desired.push((
                "vega.project",
                id.as_deref()
                    .map(crate::terminals::project_display_name)
                    .unwrap_or_default(),
            )),
            Err(e) => {
                first_error.get_or_insert(e);
            }
        }
        let changed: Vec<_> = desired
            .into_iter()
            .filter(|(key, value)| agent["labels"][key].as_str().unwrap_or("") != value)
            .map(|(key, value)| format!("{key}={value}"))
            .collect();
        if !changed.is_empty() {
            let mut args = vec!["label", pane];
            args.extend(changed.iter().map(String::as_str));
            if let Err(e) = vega_herdr::hc::hc_local(&args) {
                first_error.get_or_insert(e);
            }
        }
    }
    first_error.map_or(Ok(()), Err)
}

fn local_project(agent: &Value, host: &str) -> Result<Option<String>, Failure> {
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
        if let Some(project) = project::resolve_optional(Path::new(cwd), host)? {
            return Ok(Some(project.id.to_string()));
        }
    }
    Ok(None)
}
