//! `vega onboard`: first run and upgrade of a host (docs/design/onboarding.md).
//!
//! The changelog ([`CHANGELOG`], the repo's `CHANGELOG.md`) is embedded at
//! build time; the state file records the newest block this host has seen.
//! [`decide`] keeps the defaults from replacing a file the user already has.

use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;

use serde_json::{json, Value};
use vega_config::{Ctx, Env, Outcome, Unit};

pub mod upgrade;

pub const CHANGELOG: &str = include_str!("../../../CHANGELOG.md");

pub const KINDS: [&str; 5] = ["required", "optional", "unit", "migration", "feature"];

#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub kind: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Block {
    pub n: u32,
    pub date: String,
    pub entries: Vec<Entry>,
}

/// Blocks (`## <n> · <date>`) and their entries (`- <kind>: <text>`, indented
/// continuation lines joined), newest first as in the file. Text before the
/// first block is the preamble.
pub fn parse(text: &str) -> Result<Vec<Block>, String> {
    let mut blocks: Vec<Block> = Vec::new();
    for (i, line) in text.lines().enumerate() {
        let at = || format!("line {}", i + 1);
        if let Some(h) = line.strip_prefix("## ") {
            let (n, date) = h
                .split_once(" · ")
                .ok_or_else(|| format!("{}: heading is not `## <n> · <date>`", at()))?;
            let n = n
                .parse()
                .map_err(|_| format!("{}: block number {n:?}", at()))?;
            blocks.push(Block {
                n,
                date: date.trim().to_string(),
                entries: vec![],
            });
        } else if let Some(e) = line.strip_prefix("- ") {
            let Some(b) = blocks.last_mut() else {
                continue;
            };
            let (kind, text) = e
                .split_once(": ")
                .ok_or_else(|| format!("{}: entry is not `- <kind>: <text>`", at()))?;
            if !KINDS.contains(&kind) {
                return Err(format!("{}: unknown kind {kind:?}", at()));
            }
            b.entries.push(Entry {
                kind: kind.into(),
                text: text.trim().into(),
            });
        } else if let Some(cont) = line.strip_prefix("  ") {
            if let Some(e) = blocks.last_mut().and_then(|b| b.entries.last_mut()) {
                e.text.push(' ');
                e.text.push_str(cont.trim());
            }
        }
    }
    Ok(blocks)
}

/// The changelog's rules: contiguous numbers down to 1, dates non-increasing,
/// no empty block.
pub fn validate(blocks: &[Block]) -> Result<(), String> {
    if blocks.is_empty() {
        return Err("no blocks".into());
    }
    for (i, b) in blocks.iter().enumerate() {
        let want = (blocks.len() - i) as u32;
        if b.n != want {
            return Err(format!("block {} should be {want}", b.n));
        }
        if b.date.len() != 10 || b.date.as_bytes()[4] != b'-' || b.date.as_bytes()[7] != b'-' {
            return Err(format!(
                "block {}: date {:?} is not YYYY-MM-DD",
                b.n, b.date
            ));
        }
        if b.entries.is_empty() {
            return Err(format!("block {} has no entries", b.n));
        }
        if let Some(next) = blocks.get(i + 1) {
            if next.date > b.date {
                return Err(format!("block {} is dated after block {}", next.n, b.n));
            }
        }
    }
    Ok(())
}

pub fn newest(blocks: &[Block]) -> u32 {
    blocks.first().map_or(0, |b| b.n)
}

/// The blocks after `seen`, newest first.
pub fn since(blocks: &[Block], seen: u32) -> Vec<&Block> {
    blocks.iter().filter(|b| b.n > seen).collect()
}

fn state_path(env: &Env) -> PathBuf {
    env.state.join("vega/onboard.json")
}

/// The newest block this host has seen; None before the first `apply`.
pub fn seen(env: &Env) -> Option<u32> {
    let t = fs::read_to_string(state_path(env)).ok()?;
    let v: Value = serde_json::from_str(&t).ok()?;
    v.get("seen")?.as_u64().map(|n| n as u32)
}

pub fn record(env: &Env, block: u32, version: &str) -> Result<(), String> {
    let p = state_path(env);
    if let Some(d) = p.parent() {
        fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
    }
    let text = serde_json::to_string_pretty(&json!({"seen": block, "version": version}))
        .expect("serializes")
        + "\n";
    fs::write(&p, text).map_err(|e| format!("{}: {e}", p.display()))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    FirstRun,
    Upgrade { from: u32, to: u32 },
    Current(u32),
}

pub fn mode(seen: Option<u32>, newest: u32) -> Mode {
    match seen {
        None => Mode::FirstRun,
        Some(s) if s < newest => Mode::Upgrade {
            from: s,
            to: newest,
        },
        Some(s) => Mode::Current(s),
    }
}

/// Whether `vega post relay install` ran here: it writes herdr plugin
/// vega.vegad under `$XDG_DATA_HOME/vega/herdr-plugin` (default
/// `~/.local/share`).
pub fn vegad_installed(env: &Env) -> bool {
    std::env::var_os("XDG_DATA_HOME")
        .filter(|d| !d.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| env.home.join(".local/share"))
        .join("vega/herdr-plugin/herdr-plugin.toml")
        .exists()
}

/// The home view's vegad line (docs/design/vegad.md, Local mode): install it
/// on every host; brokerless local mode requires completed identity migration.
pub fn vegad_line(installed: bool, broker: bool) -> String {
    let local =
        "without a broker it runs in local mode after identity migration, otherwise identity quiesce; sidebar, attention and upgrade proposals in local mode; mail \
                 relay, registry, beads sync and ci verdict mail stay off";
    match (installed, broker) {
        (false, false) => format!(
            "vegad: not installed; run vega post relay install (once per host, herdr starts it; {local})"
        ),
        (false, true) => "vegad: not installed; run vega post relay install (once per host, herdr starts it)".into(),
        (true, false) => format!("vegad: installed; {local}"),
        (true, true) => "vegad: installed".into(),
    }
}

/// What `vega onboard apply` does with a unit.
#[derive(Debug, Clone, PartialEq)]
pub enum Step {
    Apply,
    /// The user has their own version: left alone, with the reason.
    Keep(String),
}

/// Units that replace whole files: applied only where the target is missing.
pub fn whole_file(u: Unit) -> bool {
    matches!(
        u,
        Unit::Herdr
            | Unit::ClaudeMd
            | Unit::CodexMd
            | Unit::PiMd
            | Unit::ClaudeStyles
            | Unit::Agentsview
            | Unit::DcgConfig
            | Unit::DcgPacks
    )
}

pub fn decide(ctx: &Ctx, u: Unit) -> Result<Step, String> {
    let plan = vega_config::plan(ctx, u)?;
    if plan.skip.is_some() {
        return Ok(Step::Apply);
    }
    if whole_file(u) {
        let own = plan
            .files
            .iter()
            .any(|f| (f.live.is_some() || f.link) && f.changed());
        return Ok(if own {
            Step::Keep("your own file differs".into())
        } else {
            Step::Apply
        });
    }
    if u == Unit::ClaudeSettings {
        let seeds = plan.seeds.unwrap_or_default();
        let live: Value = plan
            .files
            .first()
            .and_then(|f| f.live.as_deref())
            .and_then(|t| serde_json::from_str(t).ok())
            .unwrap_or(Value::Null);
        // Keys vega set itself hold the source value: not the user's.
        let set: Vec<&str> = seeds
            .iter()
            .filter(|(k, v)| live.get(k.as_str()).is_some_and(|l| l != *v))
            .map(|(k, _)| k.as_str())
            .collect();
        if !set.is_empty() {
            return Ok(Step::Keep(format!("you set {}", set.join(" "))));
        }
    }
    Ok(Step::Apply)
}

/// One unit's result of `vega onboard apply`.
#[derive(Debug, Clone, PartialEq)]
pub enum Done {
    Kept(String),
    Config(Outcome),
}

pub fn apply(ctx: &Ctx, managed: &BTreeSet<PathBuf>) -> Vec<(Unit, Done)> {
    // Decide on the state before any write: the hooks create settings.json.
    let steps: Vec<(Unit, Result<Step, String>)> =
        Unit::ALL.iter().map(|u| (*u, decide(ctx, *u))).collect();
    steps
        .into_iter()
        .map(|(u, s)| {
            let done = match s {
                Ok(Step::Keep(why)) => Done::Kept(why),
                Ok(Step::Apply) => Done::Config(vega_config::apply(ctx, u, managed)),
                Err(e) => Done::Config(Outcome::Failed(e)),
            };
            (u, done)
        })
        .collect()
}

#[cfg(test)]
mod tests;
