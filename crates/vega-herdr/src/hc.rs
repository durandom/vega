//! hc transport and ancestry-verified identity, alongside the unchanged TUI path.
use crate::{Failure, BOUNDED, DAEMON_CALL_TIMEOUT};
use serde_json::Value;
use std::{env, process::Command, time::Duration};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Terminal {
    Herdr(String),
    Hc(String),
}

impl Terminal {
    pub fn pane(&self) -> &str {
        match self {
            Self::Herdr(pane) | Self::Hc(pane) => pane,
        }
    }

    /// Local agent metadata, in the existing TUI agent shape.
    pub fn agent(&self) -> Result<Value, Failure> {
        match self {
            Self::Herdr(pane) => {
                Ok(crate::herdr_local(&["agent", "get", pane])?["result"]["agent"].clone())
            }
            Self::Hc(pane) => {
                let info = hc_local(&["get", pane])?;
                let kind = match info["status"]["app"].as_str().unwrap_or("") {
                    "claude-code" => "claude",
                    k => k,
                };
                Ok(serde_json::json!({
                    "pane_id": info["address"], "name": info["name"],
                    "kind": kind, "parent": info["parent"],
                    "cwd": info["foreground"]["cwd"].as_str().or(info["cwd"].as_str()).unwrap_or("")
                }))
            }
        }
    }

    /// An adoption in vega overrides the native hc parent link.
    pub fn parent(&self, agent: &Value) -> Option<crate::state::Parent> {
        crate::state::parent_of(self.pane(), crate::agent_name(agent)).or_else(|| {
            if !matches!(self, Self::Hc(_)) {
                return None;
            }
            let parent = agent["parent"].as_str().filter(|p| !p.is_empty())?;
            let address = crate::pane::parse(parent)?;
            Some(crate::state::Parent {
                pane: address.pane,
                host: address.machine.unwrap_or_else(crate::state::post_host),
            })
        })
    }
}

/// Select the most specific environment first; stale TUI context never falls
/// through to an enclosing hc terminal. No environment means no terminal.
pub fn own_terminal() -> Result<Terminal, Failure> {
    if env::var_os("HERDR_PANE_ID").is_some_and(|v| !v.is_empty()) {
        return crate::me().map(|m| Terminal::Herdr(m.pane));
    }
    let missing = || {
        Failure::new(
            "not_in_terminal",
            "no terminal hosts this process",
            "run this from a terminal with its context and process ancestry intact",
        )
    };
    let id = env::var("HC_TERMINAL_ID")
        .ok()
        .filter(|s| !s.is_empty())
        .ok_or_else(missing)?;
    if !crate::pane::is_hc_id(&id) {
        return Err(missing());
    }
    let info = hc_local(&["get", &id])?;
    if info["state"] != "live" || info["address"] != id {
        return Err(missing());
    }
    let pids: Vec<u32> = [info["pid"].as_u64(), info["foreground"]["pid"].as_u64()]
        .into_iter()
        .flatten()
        .filter_map(|p| u32::try_from(p).ok())
        .filter(|p| *p > 1)
        .collect();
    if crate::reaches(std::process::id(), &pids, crate::parent_of) {
        Ok(Terminal::Hc(id))
    } else {
        Err(missing())
    }
}

pub fn hc(args: &[&str]) -> Result<Value, Failure> {
    hc_on(crate::machine(), args, None)
}

pub fn hc_local(args: &[&str]) -> Result<Value, Failure> {
    hc_on(None, args, None)
}

/// Target run/exec/file/doc commands with `--on`; terminal commands accept a
/// machine-qualified address in argv with `target = None` (get/send/ls have
/// no --on flag). Put program argv after `--`, as with the hc CLI.
pub fn hc_on(
    target: Option<&str>,
    args: &[&str],
    timeout: Option<Duration>,
) -> Result<Value, Failure> {
    let mut cmd = Command::new("hc");
    cmd.arg("--json");
    let split = args.iter().position(|a| *a == "--").unwrap_or(args.len());
    cmd.args(&args[..split]);
    if let Some(m) = target {
        cmd.args(["--on", m]);
    }
    cmd.args(&args[split..]);
    let timeout = timeout.or_else(|| {
        BOUNDED
            .load(std::sync::atomic::Ordering::Relaxed)
            .then_some(DAEMON_CALL_TIMEOUT)
    });
    let io = |e: std::io::Error| {
        Failure::new(
            "hc_not_found",
            e.to_string(),
            "install hc or put it on PATH",
        )
    };
    let out = match timeout {
        None => cmd.output().map_err(io)?,
        Some(t) => crate::output_within(&mut cmd, t)
            .map_err(io)?
            .ok_or_else(|| {
                Failure::new(
                    "hc_timeout",
                    "hc did not answer before the deadline",
                    "check that the core and target machine are reachable",
                )
            })?,
    };
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    for stream in [&*stdout, &*stderr] {
        if let Ok(v) = serde_json::from_str::<Value>(stream.trim()) {
            if let Some(e) = v.get("error") {
                let kind = e["kind"].as_str().unwrap_or("error");
                let code = match kind {
                    "not_found" => "pane_not_found".to_string(),
                    "machine_unknown" => "unknown_machine".to_string(),
                    "blocked" => "agent_blocked".to_string(),
                    "invalid_params" | "needs_target" => "usage".to_string(),
                    _ => format!("hc_{kind}"),
                };
                return Err(Failure::new(
                    &code,
                    e["message"].as_str().unwrap_or("hc failed"),
                    e["hint"]
                        .as_str()
                        .unwrap_or("check the terminal and core state"),
                ));
            }
        }
    }
    if !out.status.success() {
        return Err(Failure::new(
            "hc_error",
            "hc exited without a structured error",
            "check hc and the core state",
        ));
    }
    serde_json::from_str(stdout.trim()).map_err(|_| {
        Failure::new(
            "hc_bad_output",
            "hc returned non-JSON output",
            "check hc --version",
        )
    })
}
