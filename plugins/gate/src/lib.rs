//! vega gate: an LLM review before an agent's push (docs/design/gate.md).
//!
//! The library holds everything the `vega-gate` binary and the `git` shim
//! share: the repo config read from the push target, the segment walk with its
//! diff keys, the review records in beads, the reviewer start and the verdict.
//! The shim links it so a push needs no second process before the decision.

pub mod author;
pub mod config;
pub mod git;
pub mod push;
pub mod record;
pub mod segments;
pub mod start;
pub mod state;
pub mod verdict;

use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

/// The external tools, overridable for tests: `VEGA_GATE_GIT` (the shim sets
/// it to the real git, so the gate's own git calls skip the shim),
/// `VEGA_GATE_BD`, `VEGA_GATE_VEGA`, `VEGA_GATE_HERDR`.
pub fn tool(name: &str) -> String {
    let var = format!("VEGA_GATE_{}", name.to_ascii_uppercase());
    std::env::var(var)
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| name.to_string())
}

/// A finished command.
#[derive(Debug)]
pub struct Out {
    pub ok: bool,
    pub stdout: String,
    pub stderr: String,
}

/// Runs `program args` in `dir`, killed after `timeout`. Err: it did not start
/// or timed out.
pub fn run(
    program: &str,
    args: &[&str],
    dir: Option<&Path>,
    timeout: Option<Duration>,
) -> Result<Out, String> {
    let mut cmd = Command::new(program);
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(d) = dir {
        cmd.current_dir(d);
    }
    let mut child = cmd.spawn().map_err(|e| format!("{program}: {e}"))?;
    let mut so = child.stdout.take().expect("piped");
    let mut se = child.stderr.take().expect("piped");
    let ro = thread::spawn(move || {
        let mut s = Vec::new();
        let _ = so.read_to_end(&mut s);
        s
    });
    let re = thread::spawn(move || {
        let mut s = Vec::new();
        let _ = se.read_to_end(&mut s);
        s
    });
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) => {}
            Err(e) => return Err(format!("{program}: {e}")),
        }
        if timeout.is_some_and(|t| start.elapsed() > t) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!(
                "{program} {} slower than {} s",
                args.first().copied().unwrap_or(""),
                timeout.unwrap_or_default().as_secs()
            ));
        }
        thread::sleep(Duration::from_millis(10));
    };
    let stdout = String::from_utf8_lossy(&ro.join().unwrap_or_default()).into_owned();
    let stderr = String::from_utf8_lossy(&re.join().unwrap_or_default()).into_owned();
    Ok(Out {
        ok: status.success(),
        stdout,
        stderr,
    })
}

/// stdout of a command that must succeed; Err names it with its stderr's first line.
pub fn run_ok(
    program: &str,
    args: &[&str],
    dir: Option<&Path>,
    timeout: Option<Duration>,
) -> Result<String, String> {
    let o = run(program, args, dir, timeout)?;
    if o.ok {
        Ok(o.stdout)
    } else {
        let why = o
            .stderr
            .lines()
            .chain(o.stdout.lines())
            .find(|l| !l.trim().is_empty())
            .unwrap_or("failed")
            .trim()
            .to_string();
        Err(format!(
            "{} {}: {why}",
            Path::new(program)
                .file_name()
                .map_or(program.into(), |f| f.to_string_lossy()),
            args.first().copied().unwrap_or("")
        ))
    }
}

/// `sha256` of `data`, 64 hex.
pub fn sha256(data: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(data)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// `$XDG_CACHE_HOME/vega/gate` (clones, rubric exports, prompts).
pub fn cache_dir() -> std::path::PathBuf {
    xdg("XDG_CACHE_HOME", ".cache").join("vega/gate")
}

/// `$XDG_STATE_HOME/vega/gate` (one state file per reviewer pane).
pub fn state_dir() -> std::path::PathBuf {
    xdg("XDG_STATE_HOME", ".local/state").join("vega/gate")
}

fn xdg(var: &str, fallback: &str) -> std::path::PathBuf {
    std::env::var_os(var)
        .filter(|s| !s.is_empty())
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::path::PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(fallback)
        })
}

/// `6 min`, `2 h`: a coarse age for stderr lines.
pub fn age(secs: i64) -> String {
    match secs.max(0) {
        s if s < 120 => format!("{s} s"),
        s if s < 7200 => format!("{} min", s / 60),
        s => format!("{} h", s / 3600),
    }
}
