//! Local discovery and target transport for the two terminal backends.
use serde_json::{json, Value};
use vega_herdr::{hc::hc_local, Failure};

/// Internal pane spelling. Canonical addresses and bare IDs share an inbox.
pub fn bare(pane: &str) -> &str {
    if vega_herdr::pane::is_address(pane) {
        pane.strip_prefix("hc:")
            .or_else(|| pane.strip_prefix("herdr:"))
            .unwrap_or(pane)
    } else {
        pane
    }
}

pub fn is_hc(pane: &str) -> bool {
    vega_herdr::pane::parse(pane).is_some_and(|p| p.backend == vega_herdr::pane::Backend::Hc)
}

/// Only this core's terminals: remote cwd/identity belongs to its own vegad.
pub fn local_terminal(t: &Value) -> bool {
    t["address"]
        .as_str()
        .is_some_and(vega_herdr::pane::is_hc_id)
}

pub fn hc_agents(v: &Value) -> Result<Vec<Value>, Failure> {
    let terminals = v["terminals"].as_array().ok_or_else(|| {
        Failure::new(
            "hc_bad_output",
            "hc ls has no terminals list",
            "check hc --version",
        )
    })?;
    Ok(terminals
        .iter()
        .filter(|t| local_terminal(t) && t["state"] == "live")
        .map(|t| {
            let kind = match t["status"]["app"]
                .as_str()
                .filter(|s| !s.is_empty())
                .or(t["labels"]["vega.harness"].as_str())
                .unwrap_or("")
            {
                "claude-code" => "claude",
                k => k,
            };
            json!({
                "pane_id": t["address"], "backend": "hc",
                "address": format!("hc:{}", t["address"].as_str().unwrap()),
                "name": t["name"], "agent": kind, "labels": t["labels"],
                "agent_status": t["status"]["state"].as_str().unwrap_or("unknown"),
                "completion_seq": t["turn"], "parent": t["parent"],
                "cwd": t["foreground"]["cwd"].as_str().or(t["cwd"].as_str()).unwrap_or("")
            })
        })
        .collect())
}

pub fn hc_list() -> Result<Vec<Value>, Failure> {
    let mut agents = hc_agents(&hc_local(&["ls"])?)?;
    // This machine's shared layout supplies a project folder when the launch
    // shell and foreground leader both remain at home. Never inspect remote paths.
    if let Ok(doc) = vega_herdr::hc::rpc(
        "doc.get",
        json!({"name":"herdr-app"}),
        std::time::Duration::from_secs(2),
    ) {
        apply_spaces(&mut agents, &doc);
    }
    Ok(agents)
}

fn apply_spaces(agents: &mut [Value], doc: &Value) {
    for agent in agents {
        let space = agent["labels"]["herdr.space"].as_str();
        if let Some(s) = doc["value"]["spaces"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|s| space.is_some() && s["id"].as_str() == space)
        {
            agent["space_folder"] = s["folder"].clone();
            agent["space_name"] = s["name"].clone();
        }
    }
}

/// Keep source failures separate: a failed backend is not an empty backend.
pub struct Inventory {
    pub herdr: Result<Vec<Value>, String>,
    pub hc: Result<Vec<Value>, String>,
    pub workspaces: Vec<Value>,
    pub workspace_error: Option<String>,
}

impl Inventory {
    pub fn read() -> Self {
        let list = |what: &str, key: &str| {
            vega_herdr::herdr_local(&[what, "list"]).map_err(|e| e.message)?["result"][key]
                .as_array()
                .cloned()
                .ok_or_else(|| format!("invalid herdr {what} snapshot"))
        };
        let workspaces = list("workspace", "workspaces");
        let herdr = list("agent", "agents");
        Self {
            herdr,
            hc: hc_list().map_err(|e| e.message),
            workspace_error: workspaces.as_ref().err().cloned(),
            workspaces: workspaces.unwrap_or_default(),
        }
    }

    pub fn agents(&self) -> Vec<Value> {
        self.herdr
            .as_ref()
            .into_iter()
            .chain(self.hc.as_ref())
            .flatten()
            .cloned()
            .collect()
    }

    /// Do not renew stale agents as fresh, or delete them, on a source outage.
    /// An unavailable backend with no previously published agents can coexist
    /// with the other backend (in particular an hc-only host without herdr).
    pub fn checked(&self, host: &str, view: &vega_registry::View) -> Result<Vec<Value>, String> {
        if self.herdr.is_ok() {
            if let Some(why) = &self.workspace_error {
                return Err(format!("herdr snapshot unavailable: {why}"));
            }
        }
        if self.herdr.is_err() && self.hc.is_err() {
            return Err("neither terminal backend answered".into());
        }
        for (backend, source) in [("herdr", &self.herdr), ("hc", &self.hc)] {
            if let Err(why) = source {
                let prefix = format!("{}.agent.", vega_registry::token(host));
                if view.entries.iter().any(|(k, v)| {
                    k.starts_with(&prefix) && v["backend"].as_str().unwrap_or("herdr") == backend
                }) {
                    return Err(format!("{backend} snapshot unavailable: {why}"));
                }
            }
        }
        Ok(self.agents())
    }
}

/// Seed native local hc lineage so mail/silent supervision works for children
/// started directly with hc too. Explicit vega adoption always wins. Creation
/// is atomic and does not fabricate a new brief at discovery time.
pub fn native_parents(agents: &[Value]) {
    use std::io::Write;
    for a in agents {
        let Some(pane) = a["pane_id"]
            .as_str()
            .filter(|p| vega_herdr::pane::is_hc_id(p))
        else {
            continue;
        };
        let Some(parent) = a["parent"]
            .as_str()
            .filter(|p| vega_herdr::pane::is_hc_id(p))
        else {
            continue;
        };
        if parent == pane {
            continue;
        }
        let Some(path) = vega_herdr::state::state_file_on("parents", None, pane) else {
            continue;
        };
        let Some(dir) = path.parent() else {
            continue;
        };
        if std::fs::create_dir_all(dir).is_err() {
            continue;
        }
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
        {
            let record = json!({"name":vega_herdr::agent_name(a),"parent":parent,
                "host":vega_herdr::state::post_host(),"backend":"hc","source":"hc"});
            let _ = write!(file, "{record}");
        }
    }
}

/// hc enforces blocked refusal even if status changes after our read. Idle
/// has an atomic guard; done and authorized queued mail use plain send until
/// hc offers an atomic free-state guard (vega-0h4c.17).
pub fn send_hc(pane: &str, text: &str, queued: bool) -> Result<(), Failure> {
    let pane = bare(pane);
    let t = hc_local(&["get", pane])?;
    let status = t["status"]["state"].as_str().unwrap_or("unknown");
    if t["state"] != "live" {
        return Err(Failure::new(
            "pane_gone",
            "terminal finished",
            "mail stays pending until closure is confirmed",
        ));
    }
    match status {
        "idle" => hc_local(&["send", "--if", "idle", pane, text])?,
        "done" => hc_local(&["send", pane, text])?,
        "working" if queued => hc_local(&["send", pane, text])?,
        _ => {
            return Err(Failure::new(
                "agent_not_idle",
                format!("{pane} is {status}"),
                "wait for an idle or done terminal",
            ))
        }
    };
    Ok(())
}

pub fn prompt(pane: &str, text: &str) -> Result<(), Failure> {
    if is_hc(pane) {
        send_hc(pane, text, true)
    } else {
        let pane = bare(pane);
        vega_herdr::herdr_local(&["agent", "prompt", pane, text])?;
        vega_herdr::prompt::confirm_taken(pane, text)
    }
}

/// The qualified hc machine name is the registry host by convention.
/// No SSH target, hostname heuristic, or persistent alias mapping participates.
pub fn machine_host(pane: &str) -> Option<String> {
    let p = vega_herdr::pane::parse(pane)?;
    (p.backend == vega_herdr::pane::Backend::Hc)
        .then_some(p.machine)
        .flatten()
}

pub fn mail_target(
    pane: &str,
    host: Option<&str>,
    local: &str,
) -> Result<(String, String), Failure> {
    if let Some(machine) = machine_host(pane) {
        if machine != vega_herdr::state::host_token(&machine)
            || machine != vega_registry::token(&machine)
            || host.is_some_and(|h| h != machine)
        {
            return Err(Failure::new("machine_host_mismatch",
                "an hc machine name must equal its registry host; --host cannot remap it",
                &format!("hc machine rename {machine} <host>; or address an unqualified terminal with --host <host>")));
        }
        let p = vega_herdr::pane::parse(pane).unwrap();
        return Ok((p.pane, machine));
    }
    Ok((
        bare(pane).into(),
        host.map(crate::subject_token)
            .unwrap_or_else(|| local.into()),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn local_hc_snapshot_uses_foreground_cwd_and_layout_doc_shape() {
        let snapshot = json!({"terminals":[
            {"address":"t2","state":"live","cwd":"/home/user","foreground":{"cwd":"/repo"},"status":{"app":"claude-code","state":"idle"},"labels":{"herdr.space":"sp1"}},
            {"address":"t3","state":"live","cwd":"/home/user","status":null,"labels":{"herdr.space":"sp1","vega.harness":"codex"}},
            {"address":"remote/t1","state":"live","cwd":"/remote"}
        ]});
        let mut agents = hc_agents(&snapshot).unwrap();
        apply_spaces(
            &mut agents,
            &json!({"version":1,"value":{"spaces":[{"id":"sp1","name":"vega","folder":"/repo","tabs":[]}]}}),
        );
        assert_eq!(agents.len(), 2);
        assert_eq!(agents[0]["cwd"], "/repo");
        assert_eq!(agents[0]["agent"], "claude");
        assert_eq!(agents[1]["agent"], "codex");
        assert_eq!(agents[1]["space_folder"], "/repo");
    }
    #[test]
    fn qualified_targets_use_the_machine_host_convention() {
        assert_eq!(
            mail_target("hc:io/t7", None, "here").unwrap(),
            ("t7".into(), "io".into())
        );
        assert!(mail_target("hc:io/t7", Some("receiver"), "here").is_err());
        assert_eq!(
            mail_target("hc:io/t7", Some("io"), "here").unwrap(),
            ("t7".into(), "io".into())
        );
        assert_eq!(
            mail_target("hc:t7", None, "here").unwrap(),
            ("t7".into(), "here".into())
        );
    }

    #[test]
    fn failed_backend_cannot_delete_or_renew_its_registry_entries() {
        let i = Inventory {
            herdr: Err("down".into()),
            hc: Ok(vec![]),
            workspaces: vec![],
            workspace_error: None,
        };
        let mut v = vega_registry::View::default();
        assert!(i.checked("host", &v).is_ok());
        v.entries.insert(
            vega_registry::agent_key("host", "w1:p1"),
            json!({"pane":"w1:p1"}),
        );
        assert!(i.checked("host", &v).is_err());
        let i = Inventory {
            herdr: Ok(vec![]),
            hc: Err("down".into()),
            workspaces: vec![],
            workspace_error: None,
        };
        assert!(i.checked("host", &v).is_ok());
        v.entries.insert(
            vega_registry::agent_key("host", "t1"),
            json!({"pane":"t1","backend":"hc"}),
        );
        assert!(i.checked("host", &v).is_err());
    }
}
