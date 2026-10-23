//! `vega mux next`: focus the agent that has waited longest for the Slayer,
//! on any host (docs/design/mux-next.md, vega-lhc.145).
//!
//! The queue is vegad's waiting ledger (vega_herdr::waiting): this host's
//! from its file, the other hosts' from the registry. A pane on another
//! machine is focused there; herdr cannot switch the Slayer's client, so a
//! toast names the machine to switch to.

use std::time::Duration;

use serde_json::Value;
use vega_herdr::pane::{self, Hit};
use vega_herdr::waiting::{self, Candidate};
use vega_herdr::{agent_name, herdr_local, local_host, Failure};

/// Statuses `--status` accepts.
pub const STATUSES: [&str; 2] = ["done", "blocked"];

pub fn run(statuses: &[String]) -> Result<(), Failure> {
    let here = vega_registry::token(&local_host().to_lowercase());
    let agents = herdr_local(&["agent", "list"])
        .ok()
        .and_then(|v| v["result"]["agents"].as_array().cloned())
        .unwrap_or_default();
    let mut all = local_candidates(&here, &waiting::read(), &agents);
    match vega_registry::read_blocking(Duration::from_secs(3)) {
        Ok(view) => all.extend(view.waiting().into_iter().filter(|c| c.host != here)),
        Err(why) => eprintln!("warning: registry: {why}; this host's agents only"),
    }
    let Some(c) = waiting::pick(&all, statuses) else {
        let what = statuses.join(" or ");
        toast(&format!("No {what} agent"), None);
        println!("none: no {what} agent waits");
        return Ok(());
    };
    let age = age(now_ms().saturating_sub(c.since) / 1000);
    // herdr's unnamed agents read `-`.
    let label = match c.name.as_str() {
        "" | "-" => &c.pane,
        n => n,
    };
    if c.host == here {
        pane::focus(&Hit::Local(c.pane.clone())).map_err(|e| focus_failed(c, &e))?;
        println!(
            "focused {} ({label}) on {}: {} for {age}",
            c.pane, c.host, c.status
        );
        return Ok(());
    }
    let machines = herdr_local(&["machine", "list", "--json"]).unwrap_or(Value::Null);
    let Some(machine) = machine_for(&c.host, &machines) else {
        return Err(Failure::new(
            "unknown_machine",
            format!(
                "{label} ({}) on {} waits, but no herdr machine is {}",
                c.pane, c.host, c.host
            ),
            &format!("add it with `herdr machine add {}`", c.host),
        ));
    };
    let hit = Hit::Remote {
        machine: machine.clone(),
        pane: c.pane.clone(),
    };
    pane::focus(&hit).map_err(|e| focus_failed(c, &e))?;
    toast(
        &format!("next: {label} on {}", c.host),
        Some(&format!(
            "{} for {age}; focused there, switch to {machine} with prefix+w",
            c.status
        )),
    );
    println!(
        "focused {} ({label}) on {}: {} for {age}; switch the client to machine {machine} (prefix+w)",
        c.pane, c.host, c.status
    );
    Ok(())
}

/// This host's waiting panes that herdr still lists, named as herdr names
/// them. Without an agent list (herdr away) the ledger stands as it is.
fn local_candidates(host: &str, ledger: &waiting::Ledger, agents: &[Value]) -> Vec<Candidate> {
    ledger
        .panes
        .iter()
        .filter_map(|(p, w)| {
            let agent = agents.iter().find(|a| a["pane_id"] == p.as_str());
            if agent.is_none() && !agents.is_empty() {
                return None;
            }
            Some(Candidate {
                host: host.into(),
                pane: p.clone(),
                name: agent.map(agent_name).unwrap_or_default().into(),
                status: w.status.clone(),
                since: w.since,
            })
        })
        .collect()
}

/// The herdr machine label of registry host `host`: a label or ssh target
/// (without `user@` and `:port`) whose token is the host's.
fn machine_for(host: &str, machines: &Value) -> Option<String> {
    let same = |s: &str| vega_registry::token(&s.to_lowercase()) == host;
    let list = machines.as_array()?;
    let label = |m: &Value| m["label"].as_str().map(String::from);
    list.iter()
        .find(|m| m["label"].as_str().is_some_and(same))
        .or_else(|| {
            list.iter().find(|m| {
                m["target"].as_str().is_some_and(|t| {
                    let t = t.trim_start_matches("ssh://");
                    let t = t.rsplit('@').next().unwrap_or(t);
                    same(t.split(':').next().unwrap_or(t))
                })
            })
        })
        .and_then(label)
}

fn focus_failed(c: &Candidate, e: &str) -> Failure {
    Failure::new(
        "focus_failed",
        format!("cannot focus {} on {}: {e}", c.pane, c.host),
        "the pane may have closed; run `vega mux next` again",
    )
}

/// A herdr toast on this client; best effort.
fn toast(title: &str, body: Option<&str>) {
    let mut args = vec!["notification", "show", title, "--position", "top-right"];
    if let Some(b) = body {
        args.extend(["--body", b]);
    }
    let _ = vega_herdr::herdr_text(&args);
}

fn now_ms() -> i64 {
    vega_herdr::notes::now_ms() as i64
}

/// `45s`, `25m`, `2h05m`, `3d`.
fn age(secs: i64) -> String {
    let secs = secs.max(0);
    match secs {
        0..60 => format!("{secs}s"),
        60..3600 => format!("{}m", secs / 60),
        3600..172800 => format!("{}h{:02}m", secs / 3600, secs % 3600 / 60),
        _ => format!("{}d", secs / 86400),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn machines_match_by_label_then_ssh_target() {
        let m = json!([
            {"label": "ganymede", "target": "ganymede"},
            {"label": "io", "target": "io-wsl"},
            {"label": "Build box", "target": "ssh://me@titan:2222"},
        ]);
        assert_eq!(machine_for("ganymede", &m).as_deref(), Some("ganymede"));
        assert_eq!(machine_for("io", &m).as_deref(), Some("io"));
        assert_eq!(machine_for("io-wsl", &m).as_deref(), Some("io"));
        assert_eq!(machine_for("titan", &m).as_deref(), Some("Build box"));
        assert_eq!(machine_for("deimos", &m), None);
        assert_eq!(machine_for("io", &Value::Null), None);
    }

    #[test]
    fn local_candidates_follow_herdr_and_carry_names() {
        let mut l = waiting::Ledger::default();
        l.status("w1:p1", "done", 5);
        l.status("w1:p2", "blocked", 6);
        let agents = [json!({"pane_id": "w1:p1", "agent": "claude", "name": "fix"})];
        let c = local_candidates("proteus", &l, &agents);
        assert_eq!(c.len(), 1);
        assert_eq!(
            (c[0].host.as_str(), c[0].pane.as_str(), c[0].since),
            ("proteus", "w1:p1", 5)
        );
        assert_eq!(c[0].name, agent_name(&agents[0]));
        // herdr away: the ledger stands.
        assert_eq!(local_candidates("proteus", &l, &[]).len(), 2);
    }

    #[test]
    fn ages_read_short() {
        assert_eq!(age(-3), "0s");
        assert_eq!(age(59), "59s");
        assert_eq!(age(125), "2m");
        assert_eq!(age(3700), "1h01m");
        assert_eq!(age(200_000), "2d");
    }
}
