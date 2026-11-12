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
