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
        // Only an ancestry-verified caller may bind its harness session.
        let kind = match info["status"]["app"].as_str().unwrap_or("") {
            "claude-code" => "claude",
            k => k,
        };
        let key = match kind {
            "claude" => "CLAUDE_CODE_SESSION_ID",
            "codex" => "CODEX_THREAD_ID",
            _ => "",
        };
        if let (Ok(session), Some(mut r)) = (
            env::var(key),
            crate::state::spawn_of_on(None, &id, info["name"].as_str().unwrap_or(""), kind),
        ) {
            if !session.is_empty() && r.session.as_deref() != Some(&session) {
                r.session = Some(session);
                if let Some(file) = crate::state::state_file_on("spawns", None, &id) {
                    // Independent of the command's remote --machine scope.
                    let tmp = file.with_extension(format!("{}.tmp", std::process::id()));
                    if let Ok(bytes) = serde_json::to_vec(&r) {
                        if std::fs::write(&tmp, bytes).is_ok() {
                            let _ = std::fs::rename(tmp, file);
                        }
                    }
                }
            }
        }
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

/// Compatibility argv for vega's existing callers, translated to schema-covered
/// RPC requests. No CLI display JSON is parsed. Unknown arguments fail closed.
pub fn hc_on(
    target: Option<&str>,
    args: &[&str],
    timeout: Option<Duration>,
) -> Result<Value, Failure> {
    let usage = || {
        Failure::new(
            "usage",
            "unsupported hc operation or arguments",
            "use hc::rpc with schema-covered parameters",
        )
    };
    let mut p = serde_json::json!({});
    let mut words = Vec::new();
    let mut i = 1;
    while i < args.len() {
        // Text/key payloads are literal argv, including strings beginning `--`.
        if words.len() == 1 && matches!(args.first(), Some(&"send" | &"keys" | &"label")) {
            words.extend_from_slice(&args[i..]);
            break;
        }
        let a = args[i];
        match a {
            "--" => {
                p["argv"] = serde_json::json!(&args[i + 1..]);
                break;
            }
            "--bg" => p["wait"] = false.into(),
            "--no-parent" => p["no_parent"] = true.into(),
            "--outlive-parent" => p["outlive_parent"] = true.into(),
            "--lines" | "--timeout" | "--if" | "--name" | "--cwd" | "--label" => {
                i += 1;
                let v = *args.get(i).ok_or_else(usage)?;
                match a {
                    "--lines" => p["lines"] = v.parse::<u64>().map_err(|_| usage())?.into(),
                    "--timeout" => p["timeout"] = v.parse::<f64>().map_err(|_| usage())?.into(),
                    "--label" => {
                        let (k, v) = v.split_once('=').ok_or_else(usage)?;
                        if p.get("labels").is_none() {
                            p["labels"] = serde_json::json!({});
                        }
                        p["labels"][k] = v.into();
                    }
                    _ => p[a.trim_start_matches("--")] = v.into(),
                }
            }
            _ if a.starts_with("--") => return Err(usage()),
            _ => words.push(a),
        }
        i += 1;
    }
    let method = match args.first().copied() {
        Some("machine") if words == ["ls"] => "machine.list".to_string(),
        Some("ls") if words.is_empty() => "terminal.list".to_string(),
        Some("exec") if words.is_empty() => "machine.exec".to_string(),
        Some("run") if words.is_empty() => "terminal.run".to_string(),
        Some(op @ ("get" | "kill" | "rm" | "read" | "send" | "keys" | "label")) => {
            p["terminal"] = (*words.first().ok_or_else(usage)?).into();
            match op {
                "send" if words.len() == 2 => p["text"] = words[1].into(),
                "keys" if words.len() >= 2 => p["keys"] = serde_json::json!(&words[1..]),
                "label" if words.len() >= 2 => {
                    p["labels"] = serde_json::json!({});
                    for pair in &words[1..] {
                        let (k, v) = pair.split_once('=').ok_or_else(usage)?;
                        p["labels"][k] = v.into();
                    }
                }
                "get" | "kill" | "rm" | "read" if words.len() == 1 => (),
                _ => return Err(usage()),
            }
            format!("terminal.{op}")
        }
        _ => return Err(usage()),
    };
    if let Some(m) = target {
        p["on"] = m.into();
    }
    if [
        "terminal.get",
        "terminal.read",
        "terminal.send",
        "terminal.keys",
        "terminal.kill",
        "terminal.rm",
        "terminal.run",
    ]
    .contains(&method.as_str())
    {
        if let Ok(caller) = env::var("HC_TERMINAL_ID") {
            if !caller.is_empty() {
                p["caller"] = caller.into();
            }
        }
    }
    rpc_with_timeout(
        &method,
        p,
        timeout.or_else(|| {
            BOUNDED
                .load(std::sync::atomic::Ordering::Relaxed)
                .then_some(DAEMON_CALL_TIMEOUT)
        }),
    )
}

/// Full Terminal inventory (including remote machine addresses). hc's CLI `ls`
/// display summaries are never parsed: they omit labels and foreground metadata.
/// Consumers must scope local-only operations by address themselves.
pub fn terminal_list() -> Result<Value, Failure> {
    let result = rpc("terminal.list", serde_json::json!({}), DAEMON_CALL_TIMEOUT)?;
    if !result["terminals"].is_array() {
        return Err(Failure::new(
            "hc_bad_output",
            "terminal.list has no terminals array",
            "check hc --version",
        ));
    }
    Ok(result)
}

/// hc owns the fleet. Before migration starts, retain the TUI inventory.
/// Errors are not an empty fleet: never deploy against a stale fallback.
pub fn machines() -> Result<Value, Failure> {
    let v = match hc_local(&["machine", "ls"]) {
        Err(f) if f.code == "hc_not_found" => {
            return crate::herdr_local(&["machine", "list", "--json"])
        }
        v => v?,
    };
    let rows = v["machines"].as_array().ok_or_else(|| {
        Failure::new(
            "hc_bad_output",
            "machine list has no machines array",
            "check hc --version",
        )
    })?;
    if rows.is_empty() {
        return crate::herdr_local(&["machine", "list", "--json"]);
    }
    Ok(Value::Array(rows.iter().map(|m| serde_json::json!({"label":m["name"],"target":m["target"],"host":m["host"],"enabled":true,"via":m["via"],"backend":"hc"})).collect()))
}

/// A corp/inbox-only host must never acquire command access to a private host.
pub fn check_direction(target: &str) -> Result<(), Failure> {
    if crate::state::is_inbox_only(&crate::state::post_host())
        && !crate::state::is_inbox_only(target)
    {
        return Err(Failure::new(
            "machine_direction",
            "a corporate host cannot control a private host",
            "run this operation from the private host",
        ));
    }
    Ok(())
}

/// One JSON-lines RPC over hc's supported stdio transport. Unlike `hc run`,
/// terminal.run accepts an environment separately from argv. EOF closes the
/// one-shot connection after its reply; no core lifecycle command is issued.
pub fn rpc(method: &str, params: Value, timeout: Duration) -> Result<Value, Failure> {
    rpc_with_timeout(method, params, Some(timeout))
}

fn rpc_with_timeout(
    method: &str,
    params: Value,
    timeout: Option<Duration>,
) -> Result<Value, Failure> {
    crate::hc_schema::params(method, &params)
        .map_err(|e| Failure::new("usage", e, "use the parameters in the pinned hc RPC schema"))?;
    let io = |e: std::io::Error| {
        Failure::new(
            if e.kind() == std::io::ErrorKind::NotFound {
                "hc_not_found"
            } else {
                "hc_unavailable"
            },
            e.to_string(),
            "check hc and the target machine connection",
        )
    };
    let mut input = serde_json::to_vec(&serde_json::json!({
        "jsonrpc":"2.0", "id":1, "method":method, "params":params
    }))
    .expect("JSON value serializes");
    input.push(b'\n');
    let mut cmd = Command::new("hc");
    cmd.arg("connect");
    let line = crate::rpc_line_within(&mut cmd, input, timeout)
        .map_err(io)?
        .ok_or_else(|| {
            Failure::new(
                "hc_timeout",
                "hc did not answer before the deadline",
                "inspect vega mux --all before retrying; a launch may have completed",
            )
        })?;
    let bad = || {
        Failure::new(
            "hc_bad_output",
            "hc returned no valid RPC reply",
            "check hc --version and inspect vega mux --all before retrying",
        )
    };
    let reply: Value = serde_json::from_slice(&line).map_err(|_| bad())?;
    if reply["jsonrpc"] != "2.0" || reply["id"] != 1 {
        return Err(bad());
    }
    if let Some(e) = reply.get("error") {
        let kind = e["data"]["kind"].as_str().unwrap_or("error");
        return Err(Failure::new(
            &match kind {
                "not_found" => "pane_not_found".into(),
                "machine_unknown" => "unknown_machine".into(),
                "blocked" => "agent_blocked".into(),
                "invalid_params" | "needs_target" => "usage".into(),
                _ => format!("hc_{kind}"),
            },
            e["message"].as_str().unwrap_or("hc RPC failed"),
            e["data"]["hint"]
                .as_str()
                .unwrap_or("inspect vega mux --all before retrying"),
        ));
    }
    let result = reply.get("result").cloned().ok_or_else(bad)?;
    validate_result(method, &result)?;
    Ok(result)
}

/// Validate a result from a streaming RPC connection against the pinned hc API.
/// One-shot callers are checked by `rpc` automatically.
pub fn validate_result(method: &str, result: &Value) -> Result<(), Failure> {
    crate::hc_schema::result(method, result).map_err(|e| {
        Failure::new(
            "hc_bad_output",
            e,
            "check hc --version against the vendored schema",
        )
    })
}
