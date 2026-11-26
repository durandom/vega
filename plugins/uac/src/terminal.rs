//! Identity and inventory for the two terminal worlds.
use super::*;
use vega_herdr::hc::{hc_local, own_terminal, Terminal};
use vega_herdr::pane::{parse, Backend};

pub fn identity() -> Result<Me, Failure> {
    match own_terminal()? {
        Terminal::Herdr(_) => vega_herdr::me(),
        terminal @ Terminal::Hc(_) => {
            let a = terminal.agent()?;
            Ok(Me {
                pane: terminal.pane().into(),
                workspace: String::new(),
                agent: str_of(&a, "kind").into(),
                program: str_of(&a, "kind").into(),
                cwd: str_of(&a, "cwd").into(),
            })
        }
    }
}

fn normalize(t: &Value) -> Value {
    let kind = match t["status"]["app"].as_str().unwrap_or("") {
        "claude-code" => "claude",
        k => k,
    };
    serde_json::json!({"pane_id":t["address"], "backend":"hc", "name":t["name"],
        "agent":kind, "agent_status":t["status"]["state"],
        "cwd":t["foreground"]["cwd"].as_str().or(t["cwd"].as_str()).unwrap_or("")})
}

pub fn local() -> Result<Snapshot, Failure> {
    let tui = snapshot(None);
    let hc = vega_herdr::hc::terminal_list().and_then(|v| {
        v["terminals"]
            .as_array()
            .map(|rows| {
                rows.iter()
                    .filter(|t| {
                        t["state"] == "live"
                            && t["address"]
                                .as_str()
                                .is_some_and(vega_herdr::pane::is_hc_id)
                    })
                    .map(normalize)
                    .collect::<Vec<_>>()
            })
            .ok_or_else(|| {
                Failure::new(
                    "hc_bad_output",
                    "terminal.list has no terminals list",
                    "check hc --version",
                )
            })
    });
    // A caller's own backend must answer; the other backend may not be installed.
    let caller = own_terminal().ok();
    match (tui, hc) {
        (Ok(mut s), Ok(agents)) => {
            s.agents.extend(agents);
            s.backends.push("hc");
            Ok(s)
        }
        (Ok(s), Err(_)) if !matches!(caller, Some(Terminal::Hc(_))) => Ok(s),
        (Err(_), Ok(agents)) if !matches!(caller, Some(Terminal::Herdr(_))) => Ok(Snapshot {
            machine: None,
            agents,
            workspaces: vec![],
            tabs: vec![],
            backends: vec!["hc"],
        }),
        (Err(e), _) | (_, Err(e)) => Err(e),
    }
}

/// A remote machine's terminals. hc's own `terminal.list` already carries every connected machine's
/// terminals as `machine/tN`, so one local call answers for all of them; the herdr API socket of an hc
/// machine does not exist. Only a host without hc asks the machine through herdr.
pub fn remote(machine: &str) -> Result<Snapshot, Failure> {
    match vega_herdr::hc::terminal_list() {
        Ok(v) => {
            let prefix = format!("{machine}/");
            let agents = v["terminals"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|t| t["state"] == "live")
                .filter_map(|t| {
                    let local = t["address"].as_str()?.strip_prefix(&prefix)?;
                    let mut a = normalize(t);
                    a["pane_id"] = local.into();
                    Some(a)
                })
                .collect();
            Ok(Snapshot {
                machine: Some(machine.to_string()),
                agents,
                workspaces: vec![],
                tabs: vec![],
                backends: vec!["hc"],
            })
        }
        Err(f) if f.code == "hc_not_found" => snapshot(Some(machine)),
        Err(f) => Err(f),
    }
}

pub fn agent(target: &str, machine: Option<&str>) -> Result<Value, Failure> {
    let hc = match parse(target) {
        Some(p) => p.backend == Backend::Hc,
        None if target.starts_with("hc:") => true,
        None if target.starts_with("herdr:") => false,
        None => {
            let records: Vec<_> = st::spawn_records()
                .into_iter()
                .filter(|r| r.name == target)
                .collect();
            if records.len() == 1 {
                records[0].backend == "hc"
            } else {
                matches!(own_terminal()?, Terminal::Hc(_))
            }
        }
    };
    if hc {
        let target = target.strip_prefix("hc:").unwrap_or(target);
        let target = match machine {
            Some(m) if !target.contains('/') => format!("{m}/{target}"),
            _ => target.into(),
        };
        let t = hc_local(&["get", &target])?;
        let mut a = normalize(&t);
        // State records use the selected machine plus its local pane ID.
        if machine.is_some() {
            if let Some((_, pane)) = str_of(&a, "pane_id").rsplit_once('/') {
                a["pane_id"] = pane.to_string().into();
            }
        }
        Ok(a)
    } else {
        Ok(herdr_on(
            machine,
            &[
                "agent",
                "get",
                target.strip_prefix("herdr:").unwrap_or(target),
            ],
            MACHINE_TIMEOUT,
        )?["result"]["agent"]
            .clone())
    }
}

/// Keep remote identity separate from the local pane key in lineage state.
pub fn scope(target: &str, machine: Option<&str>) -> Result<(String, Option<String>), Failure> {
    if let Some(p) = parse(target) {
        if let (Some(a), Some(b)) = (p.machine.as_deref(), machine) {
            if a != b {
                return Err(Failure::new(
                    "usage",
                    "target and --machine name different machines",
                    "use one machine for the target",
                ));
            }
        }
        let prefix = match p.backend {
            Backend::Hc => "hc",
            Backend::Herdr => "herdr",
        };
        return Ok((
            format!("{prefix}:{}", p.pane),
            p.machine.or_else(|| machine.map(String::from)),
        ));
    }
    if target.contains('/') {
        return Err(Failure::new(
            "usage",
            "a qualified target must contain a terminal ID, not a name",
            "use hc:<machine>/t7 or <name> --machine <machine>",
        ));
    }
    Ok((target.into(), machine.map(String::from)))
}

pub fn screen(pane: &str) -> Option<String> {
    if parse(pane).is_some_and(|p| p.backend == Backend::Hc) {
        hc_local(&["read", "--lines", "0", pane]).ok()?["text"]
            .as_str()
            .map(String::from)
    } else {
        vega_herdr::herdr_text(&["pane", "read", pane, "--source", "visible"]).ok()
    }
}

pub fn backend(pane: &str) -> &'static str {
    if parse(pane).is_some_and(|p| p.backend == Backend::Hc) {
        "hc"
    } else {
        "herdr"
    }
}

/// Fresh registry publications supply remote hc agents absent from herdr lists.
pub fn add_registry_hc(snapshots: &mut Vec<Snapshot>, view: Option<&vega_registry::View>) {
    let Some(view) = view else {
        return;
    };
    let local = vega_registry::token(&st::post_host().to_lowercase());
    for (host, a) in view.agents() {
        if host == local || a["backend"] != "hc" || a["schema"] != 2 {
            continue;
        }
        let Some(pane) = a["pane"].as_str().filter(|p| vega_herdr::pane::is_hc_id(p)) else {
            continue;
        };
        let index = snapshots.iter().position(|s| {
            s.machine
                .as_deref()
                .is_some_and(|m| vega_registry::token(&m.to_lowercase()) == host)
        });
        let index = index.unwrap_or_else(|| {
            snapshots.push(Snapshot {
                machine: Some(host.clone()),
                agents: vec![],
                workspaces: vec![],
                tabs: vec![],
                backends: vec![],
            });
            snapshots.len() - 1
        });
        let snapshot = &mut snapshots[index];
        if !snapshot.backends.contains(&"hc") {
            snapshot.backends.push("hc");
        }
        if snapshot
            .agents
            .iter()
            .any(|existing| existing["pane_id"] == pane)
        {
            continue;
        }
        snapshot.agents.push(serde_json::json!({
            "pane_id":pane,"backend":"hc","name":a["name"],
            "agent":a["kind"],"agent_status":a["status"],"cwd":a["cwd"]
        }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fresh_registry_hc_agents_reach_remote_project_inventory() {
        use serde_json::json;
        use vega_registry::{agent_key, beat_key, View};
        let mut view = View::default();
        view.entries
            .insert(beat_key("io"), json!({"at":unix_now()}));
        view.entries.insert(beat_key("stale"), json!({"at":0}));
        let a = json!({"schema":2,"backend":"hc","pane":"t9","name":"remote-worker","kind":"codex","status":"idle","project":"github.com/example/project","cwd":"/remote/never/local"});
        for host in ["io", "stale"] {
            view.entries.insert(agent_key(host, "t9"), a.clone());
        }
        let mut snapshots = vec![Snapshot {
            machine: Some("io".into()),
            agents: vec![],
            workspaces: vec![],
            tabs: vec![],
            backends: vec!["herdr"],
        }];
        add_registry_hc(&mut snapshots, Some(&view));
        add_registry_hc(&mut snapshots, Some(&view));
        assert_eq!(snapshots.len(), 1);
        let snapshot = &snapshots[0];
        assert_eq!(snapshot.backends, ["herdr", "hc"]);
        assert_eq!(snapshot.agents.len(), 1);
        let agent = &snapshot.agents[0];
        assert_eq!(agent["pane_id"], "t9");
        assert_eq!(agent["agent"], "codex");
        assert!(remote_in_project(
            Some(&view),
            "io",
            str_of(agent, "pane_id"),
            "github.com/example/project"
        ));
        assert!(!remote_in_project(
            Some(&view),
            "io",
            "t9",
            "github.com/example/other"
        ));
        assert_eq!(agent["cwd"], "/remote/never/local");
        let mut hc_only = vec![];
        add_registry_hc(&mut hc_only, Some(&view));
        assert_eq!(hc_only.len(), 1);
        assert_eq!(hc_only[0].machine.as_deref(), Some("io"));
    }
}
