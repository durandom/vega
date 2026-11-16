//! Explicit infra diagnostic; not part of ordinary onboarding/component probes.
use crate::infra::Probe;
use serde::Deserialize;
use std::{process::Command, time::Duration};
use vega_herdr::{cell, Failure};

pub const SCRIPT: &str = include_str!("ssh_auth.py");
pub const HINT: &str = "hc/herdr uses a Background session: check IdentityAgent ~/.ssh/agent.sock and the Aqua local.ssh-agent LaunchAgent; in Terminal.app: SSH_AUTH_SOCK=$HOME/.ssh/agent.sock ssh-add --apple-use-keychain <key>, then SSH_AUTH_SOCK=$HOME/.ssh/agent.sock ssh-add --apple-load-keychain; verify host keys/network if auth still fails (docs/design/ssh-agent.md)";

#[derive(Debug, Deserialize)]
pub struct Row {
    pub host: String,
    pub state: String,
    pub ok: bool,
}

pub fn local(p: &Probe) -> Result<Vec<Row>, Failure> {
    if !p.env.home.join(".ssh/config").exists() {
        return Ok(vec![]);
    }
    let python = p.find("python3").ok_or_else(unavailable)?;
    let mut cmd = Command::new(python);
    cmd.args(["-c", SCRIPT])
        .env("HOME", &p.env.home)
        .env("PATH", std::env::join_paths(&p.path).unwrap_or_default());
    let out = vega_herdr::output_within(&mut cmd, Duration::from_secs(140))
        .map_err(|_| unavailable())?
        .ok_or_else(unavailable)?;
    if !out.status.success() {
        return Err(unavailable());
    }
    parse(&out.stdout)
}

pub fn parse(bytes: &[u8]) -> Result<Vec<Row>, Failure> {
    serde_json::from_slice(bytes).map_err(|_| unavailable())
}

fn unavailable() -> Failure {
    Failure::new(
        "ssh_probe_unavailable",
        "Git SSH diagnostic unavailable",
        "install python3 and ssh, then rerun vega config infra; no config changes were made",
    )
}

pub fn print(machine: &str, rows: &[Row]) -> bool {
    let key = format!("ssh_auth_{}", vega_registry::token(machine));
    if rows.is_empty() {
        println!("{key}: 0 configured Git hosts");
    } else {
        println!("{key}[{}]{{machine,host,state,next}}:", rows.len());
        for row in rows {
            println!(
                "  {},{},{},{}",
                cell(machine),
                cell(&row.host),
                cell(&row.state),
                cell(if row.ok { "" } else { HINT })
            );
        }
    }
    rows.iter().all(|r| r.ok)
}
