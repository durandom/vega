//! Target transport. Explicit addresses and durable records precede caller context.
use super::*;
use serde_json::json;
use std::sync::OnceLock;
use vega_herdr::hc::{hc_local, own_terminal, Terminal};
use vega_herdr::pane::{parse, Backend};
static OVERRIDE: OnceLock<Option<String>> = OnceLock::new();
pub fn set_override(value: Option<String>) {
    let _ = OVERRIDE.set(value);
}
pub fn spawn_hc() -> Result<bool, Failure> {
    if let Some(b) = OVERRIDE.get().and_then(|v| v.as_deref()) {
        return Ok(b == "hc");
    }
    caller_hc()
}
fn caller_hc() -> Result<bool, Failure> {
    if env::var_os("HERDR_PANE_ID").is_some_and(|v| !v.is_empty()) {
        return Ok(false);
    }
    if env::var_os("HC_TERMINAL_ID").is_some_and(|v| !v.is_empty()) {
        return own_terminal().map(|t| matches!(t, Terminal::Hc(_)));
    }
    Ok(false)
}
pub fn is_hc_target(target: &str) -> Result<bool, Failure> {
    if let Some(p) = parse(target) {
        return Ok(p.backend == Backend::Hc);
    }
    if target.starts_with("hc:") {
        return Ok(true);
    }
    if target.starts_with("herdr:") {
        return Ok(false);
    }
    let records: Vec<_> = vega_herdr::state::spawn_records()
        .into_iter()
        .filter(|r| r.name == target)
        .collect();
    if records.len() == 1 {
        return Ok(records[0].backend == "hc");
    }
    // Prefer the caller's namespace, but names in the other world remain usable.
    if caller_hc()? {
        match info(target) {
            Ok(_) => Ok(true),
            Err(f) if f.code == "pane_not_found" => {
                herdr(&["agent", "get", target])?;
                Ok(false)
            }
            Err(f) => Err(f),
        }
    } else {
        match herdr(&["agent", "get", target]) {
            Ok(_) => Ok(false),
            Err(f) if f.code == "agent_not_found" => {
                info(target)?;
                Ok(true)
            }
            Err(f) => Err(f),
        }
    }
}
pub fn address(target: &str) -> String {
    let t = target.strip_prefix("hc:").unwrap_or(target);
    match machine() {
        Some(m) if !t.contains('/') => format!("{m}/{t}"),
        _ => t.into(),
    }
}
pub fn info(target: &str) -> Result<Value, Failure> {
    hc_local(&["get", &address(target)])
}
pub fn normalize(t: &Value) -> Value {
    let kind = match t["status"]["app"]
        .as_str()
        .or(t["labels"]["vega.kind"].as_str())
        .unwrap_or("")
    {
        "claude-code" => "claude",
        k => k,
    };
    let pane = t["address"].as_str().unwrap_or("");
    let name = t["name"].as_str().unwrap_or("");
    let session =
        vega_herdr::state::spawn_of_on(machine(), pane, name, kind).and_then(|r| r.session);
    json!({"pane_id":pane,"name":name,"agent":kind,"agent_status":t["status"]["state"],"backend":"hc",
        "cwd":t["foreground"]["cwd"].as_str().or(t["cwd"].as_str()).unwrap_or(""),
        "agent_session":{"value":session},"completion_seq":t["turn"]})
}
pub fn agent(target: &str) -> Result<Value, Failure> {
    if is_hc_target(target)? {
        Ok(json!({"result":{"agent":normalize(&info(target)?)}}))
    } else {
        herdr(&[
            "agent",
            "get",
            target.strip_prefix("herdr:").unwrap_or(target),
        ])
    }
}
pub fn read(target: &str, lines: u32) -> Result<String, Failure> {
    let v = hc_local(&["read", &address(target), "--lines", &lines.to_string()])?;
    v["text"].as_str().map(String::from).ok_or_else(|| {
        Failure::new(
            "hc_bad_output",
            "terminal read has no text",
            "check hc --version",
        )
    })
}
pub fn body(text: &str, footer: Footer, remote: bool) -> Result<String, Failure> {
    let caller = own_terminal()?;
    let (origin, reply, host) = reply_origin(caller.pane(), remote);
    let f = footer_text(
        footer,
        caller.pane(),
        &reply,
        host.as_deref(),
        machine().is_some_and(vega_herdr::state::is_inbox_only),
    );
    Ok(format!("{text}\n\n---\n[vega-mux] from {origin}{f}"))
}
pub fn send(target: &str, text: &str, footer: Footer) -> Result<(), Failure> {
    let t = info(target)?;
    let pane = str_of(&t, "address");
    let caller = own_terminal()?;
    if pane == caller.pane() && machine().is_none() {
        return Err(Failure::new(
            "self_send",
            "target is your own terminal",
            "run `vega mux` to list other agents",
        ));
    }
    let remote = machine().is_some() || parse(pane).is_some_and(|p| p.machine.is_some());
    hc_local(&["send", pane, &body(text, footer, remote)?])?;
    Ok(())
}
pub fn close(target: &str) -> Result<(), Failure> {
    let t = match info(target) {
        Ok(t) => t,
        Err(f) if f.code == "pane_not_found" => {
            println!("closed: {target} (already absent)");
            return Ok(());
        }
        Err(f) => return Err(f),
    };
    let pane = str_of(&t, "address");
    if machine().is_none() && own_terminal().is_ok_and(|c| c.pane() == pane) {
        return Err(Failure::new(
            "self_close",
            "refusing to close your own terminal",
            "close another agent",
        ));
    }
    reopen::snapshot(&normalize(&t));
    if t["state"] == "live" {
        hc_local(&["kill", pane])?;
    }
    hc_local(&["rm", pane])?;
    forget(pane);
    println!("closed: {pane}");
    Ok(())
}
pub fn home(all: bool) -> Result<(), Failure> {
    let list = hc_local(&["ls"])?;
    let own = own_terminal().ok();
    let caller = own.as_ref().and_then(|o| info(o.pane()).ok());
    let space = caller
        .as_ref()
        .and_then(|v| v["labels"]["herdr.space"].as_str());
    let rows: Vec<_> = list["terminals"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|t| {
            t["state"] == "live"
                && (all || space.is_none() || t["labels"]["herdr.space"].as_str() == space)
        })
        .map(normalize)
        .collect();
    if rows.is_empty() {
        println!("agents: 0 live agents");
    } else {
        for line in agent_table(&rows.iter().collect::<Vec<_>>()) {
            println!("{line}");
        }
    }
    Ok(())
}
