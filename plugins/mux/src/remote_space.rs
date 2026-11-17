//! A headed remote child belongs to its target's layout, wherever the window runs.
use super::*;
use base64::Engine;
use serde_json::json;
use std::time::Duration;
use vega_herdr::hc;

pub struct Placement {
    pub space: String,
    pub tab: String,
    placeholder: Option<String>,
    window: Window,
}

struct Window(Option<String>);
impl Window {
    fn call(&self, args: &[&str]) -> Result<Value, Failure> {
        let Some(host) = self.0.as_deref() else {
            return hc_spawn::app(args);
        };
        let mut argv = vec!["herdr-app"];
        argv.extend(args);
        let timeout = if args.first() == Some(&"do") { 30 } else { 3 };
        let v = hc::rpc(
            "machine.exec",
            json!({"on":host,"argv":argv,"timeout":timeout}),
            Duration::from_secs(timeout + 1),
        )?;
        if v["code"] != 0 || v["timed_out"] == true || v["truncated"] == true {
            return Err(unavailable());
        }
        let bytes = v["stdout"]
            .as_str()
            .and_then(|s| base64::engine::general_purpose::STANDARD.decode(s).ok())
            .ok_or_else(unavailable)?;
        let result: Value = serde_json::from_slice(&bytes).map_err(|_| unavailable())?;
        if result.get("error").is_some() {
            return Err(unavailable());
        }
        Ok(result)
    }
}
fn unavailable() -> Failure {
    Failure::new(
        "app_unavailable",
        "no reachable herdr-app placement",
        "open herdr-app on a saved hc host connected to the target machine",
    )
}

/// Only accept IDs from the target's namespace; never borrow the parent's IDs.
fn local_id(id: &Value, machine: &str) -> Option<String> {
    let id = id.as_str()?;
    let id = id.strip_prefix(&format!("{machine}/")).unwrap_or(id);
    (!id.is_empty() && !id.contains('/')).then(|| id.to_owned())
}

pub fn prepare(target: &str, cwd: &str, name: &str) -> Result<Placement, Failure> {
    let mut endpoints = vec![Window(None)];
    if let Ok(machines) = hc::machines() {
        endpoints.extend(
            machines
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|m| m["label"].as_str())
                .filter(|m| hc::check_direction(m).is_ok())
                .map(|m| Window(Some(m.into()))),
        );
    }
    for window in endpoints {
        let Ok(snapshot) = window.call(&["ls"]) else {
            continue;
        };
        let Some(spaces) = snapshot["spaces"].as_array() else {
            continue;
        };
        // From the target's own window its spaces are local (machine:null).
        let app_target = if window.0.as_deref() == Some(target) {
            ""
        } else {
            target
        };
        let space = spaces
            .iter()
            .find(|s| s["machine"].as_str().unwrap_or("") == app_target && s["folder"] == cwd);
        let args = if let Some(space) = space {
            json!({"space":space["id"],"name":name,"folder":cwd,"focus":false})
        } else {
            json!({"machine":app_target,"name":name,"folder":cwd,"focus":false})
        };
        let command = if space.is_some() {
            "tab.new"
        } else {
            "space.new"
        };
        // Once a mutation has been attempted, do not retry on another screen:
        // a lost response could otherwise create duplicate spaces.
        let made = window.call(&["do", command, &args.to_string()])?;
        let namespace = if app_target.is_empty() { "" } else { target };
        let space_id = made.get("space").or_else(|| space.map(|s| &s["id"]));
        let space = space_id
            .and_then(|id| local_id(id, namespace))
            .ok_or_else(unavailable)?;
        let tab = local_id(&made["tab"], namespace).ok_or_else(unavailable)?;
        let placeholder = local_id(&made["terminal"], namespace)
            .filter(|id| vega_herdr::pane::is_hc_id(id))
            .map(|id| format!("{target}/{id}"));
        return Ok(Placement {
            space,
            tab,
            placeholder,
            window,
        });
    }
    Err(unavailable())
}

impl Placement {
    pub fn cleanup(&self, pane: &str) {
        let Some(shell) = &self.placeholder else {
            return;
        };
        // Adoption is asynchronous. Closing the only pane before adoption can
        // destroy the space; leave the shell if the app cannot confirm it.
        for _ in 0..10 {
            let Ok(snapshot) = self.window.call(&["ls"]) else {
                break;
            };
            if snapshot["spaces"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|s| local_id(&s["id"], machine().unwrap_or("")) == Some(self.space.clone()))
                .flat_map(|s| s["tabs"].as_array().into_iter().flatten())
                .filter(|t| local_id(&t["id"], machine().unwrap_or("")) == Some(self.tab.clone()))
                .flat_map(|t| t["terminals"].as_array().into_iter().flatten())
                .any(|t| {
                    t["terminal"] == pane
                        || self.window.0.as_deref() == machine()
                            && t["terminal"].as_str() == pane.split('/').next_back()
                })
            {
                for verb in ["kill", "rm"] {
                    if let Err(f) = hc::hc_local(&[verb, shell]) {
                        eprintln!(
                            "warning: cannot {verb} placement shell {shell} ({})",
                            f.message
                        );
                        break;
                    }
                }
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        eprintln!(
            "warning: placement shell {shell} retained; app has not confirmed adoption of {pane}"
        );
    }
}
