//! `vega uber`: the Uber-UAC, the Slayer's one channel to every UAC
//! (docs/design/uber-uac.md). This plugin holds the estate-wide singleton
//! claim (§2, vega-lhc.209.2): `claim`, `release` and `status`; the UAC
//! side `ask` (§3, vega-lhc.209.4, ask.rs); and the Uber host's answer path
//! (§4-5, vega-lhc.209.6): `answer` and `audit` (`answer.rs`). The capture
//! hook is a binary of its own, `vega_uber_capture` (`capture_main.rs`),
//! outside `vega uber`.
//!
//! The engaged window (vega-lhc.209.12, `vega_registry::uber_engaged`):
//! `status` shows it, and `ask` treats an idle Uber as absent.
//!
//! The claim file on its host is the truth; the registry key
//! `<host>.uber.v1` is its copy (`vega_registry::uber_state`), and
//! `View::live_uber` decides which claim is the Uber. This host's herdr
//! panes and claim file replace what the registry holds for it.

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use clap::error::ErrorKind;
use clap::{Parser, Subcommand};
use serde_json::json;
use vega_herdr::{herdr_local, print_failure, state as st, Failure};
use vega_registry::{uber_state, UberClaim, View};

mod answer;

const SKILL: &str = include_str!("../SKILL.md");

/// Connect, read and write within this; the claim needs the broker.
const TIMEOUT: Duration = Duration::from_secs(6);

#[derive(Parser)]
#[command(
    name = "vega uber",
    bin_name = "vega uber",
    disable_version_flag = true,
    about = "The Uber-UAC: the Slayer's one channel to every UAC (estate-wide singleton claim)",
    after_help = "Without a command: status.\n\nExamples:\n  vega uber claim     # this session becomes the Uber-UAC\n  vega uber release   # hand the claim back\n  vega uber status    # live, superseded and gone claims\n  vega uber ask --bead vega-12 --level L3 --deadline no-rush --option \"yes: ship\" --option \"no: wait\" --recommend 0\n  vega uber answer a7 --capture 12   # the Slayer's answer to #a7 from capture 12\n  vega uber audit --last 20"
)]
struct Cli {
    /// Print the agent guide for this plugin, SKILL.md form (read by `vega skills`)
    #[arg(long)]
    skill: bool,
    /// Print the bare version
    #[arg(short = 'V', short_alias = 'v', long)]
    version: bool,
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// This session becomes the Uber-UAC; a live claim elsewhere refuses
    Claim,
    /// The Uber hands back its claim
    Release,
    /// The live claim, superseded and gone claims
    Status,
    /// UAC side: send a slayer-decision bead to the Uber-UAC (uber_absent: decide through the bead)
    Ask(ask::AskArgs),
    /// Send the Slayer's answer to an ask, built from one capture (never from model text)
    Answer {
        /// The ask: its id, or the tag it holds (a7 or '#a7')
        ask: String,
        /// The capture line (1-based) in captures.jsonl; without it: refused, the hint lists the bound ones
        #[arg(long)]
        capture: Option<usize>,
    },
    /// One line per event: asks received, answered, sent, applied, refused
    Audit {
        /// How many events
        #[arg(long, default_value_t = 10)]
        last: usize,
    },
}

fn main() -> ExitCode {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(e) if matches!(e.kind(), ErrorKind::DisplayHelp) => {
            let _ = e.print();
            return ExitCode::SUCCESS;
        }
        Err(e) => {
            let msg = e.to_string();
            let message = msg
                .lines()
                .take_while(|l| !l.trim().is_empty())
                .map(str::trim)
                .collect::<Vec<_>>()
                .join(" ");
            print_failure(&Failure::new(
                "usage",
                message.trim_start_matches("error: "),
                "vega uber --help",
            ));
            return ExitCode::from(2);
        }
    };
    if cli.skill {
        print!("{SKILL}");
        return ExitCode::SUCCESS;
    }
    if cli.version {
        println!("{}", vega_build::version_line!());
        return ExitCode::SUCCESS;
    }
    let result = match cli.cmd {
        Some(Cmd::Claim) => claim(),
        Some(Cmd::Release) => release(),
        Some(Cmd::Status) | None => status(),
        Some(Cmd::Ask(a)) => ask::ask(a),
        Some(Cmd::Answer { ask, capture }) => answer::answer(&ask, capture),
        Some(Cmd::Audit { last }) => answer::audit(last),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(f) => {
            print_failure(&f);
            ExitCode::from(1)
        }
    }
}

/// This session and its host's local truth.
struct Here {
    host: String,
    pane: String,
    /// The panes herdr lists on this host now.
    panes: Vec<String>,
    /// This host's claim file.
    file: PathBuf,
}

impl Here {
    fn mine(&self, c: &UberClaim) -> bool {
        c.pane == self.pane && vega_registry::token(&c.host) == vega_registry::token(&self.host)
    }
}

fn here() -> Result<Here, Failure> {
    let me = vega_herdr::me()?;
    let panes = herdr_local(&["pane", "list"])?["result"]["panes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|p| p["pane_id"].as_str().map(String::from))
        .collect();
    let file = uber_state::file()
        .ok_or_else(|| Failure::new("no_home", "no state dir", "set HOME or XDG_STATE_HOME"))?;
    Ok(Here {
        host: st::post_host(),
        pane: me.pane,
        panes,
        file,
    })
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// This host's panes and claim file replace the registry's copy of them:
/// vegad publishes both only every few seconds.
fn with_local(view: &mut View, here: &Here) {
    let own = format!("{}.agent.", vega_registry::token(&here.host));
    view.entries.retain(|k, _| !k.starts_with(&own));
    view.entries
        .insert(vega_registry::beat_key(&here.host), vega_registry::beat());
    for p in &here.panes {
        view.entries.insert(
            vega_registry::agent_key(&here.host, p),
            json!({"pane": p, "status": "local"}),
        );
    }
    let key = vega_registry::uber_key(&here.host);
    match uber_state::published(&here.file, &here.host) {
        Some(v) => view.entries.insert(key, v),
        None => view.entries.remove(&key),
    };
}

async fn read_view(
    client: &async_nats::Client,
    kv: &async_nats::jetstream::kv::Store,
    here: &Here,
) -> Result<View, String> {
    let mut view = vega_registry::read_kinds(client, kv, Some(&["beat", "agent", "uber"])).await?;
    with_local(&mut view, here);
    Ok(view)
}

fn no_broker(e: String) -> Failure {
    Failure::new(
        "no_broker",
        e,
        "the Uber-UAC needs the broker: without it there is no Uber (docs/design/uber-uac.md §2)",
    )
}

/// Runs `f` with a registry connection, within TIMEOUT.
fn with_registry<T>(
    f: impl AsyncFnOnce(&async_nats::Client, &async_nats::jetstream::kv::Store) -> Result<T, Failure>,
) -> Result<T, Failure> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| no_broker(e.to_string()))?;
    rt.block_on(async {
        tokio::time::timeout(TIMEOUT, async {
            let client = vega_registry::connect("uber").await.map_err(no_broker)?;
            let kv = vega_registry::bucket(&client).await.map_err(no_broker)?;
            f(&client, &kv).await
        })
        .await
        .map_err(|_| no_broker("registry timed out".into()))?
    })
}

/// What `claim_on` did.
#[derive(Debug)]
struct Claimed {
    me: UberClaim,
    /// This session already was the live Uber: nothing written.
    already: bool,
    /// The gone claims this one replaces.
    took_over: Vec<UberClaim>,
    /// A newer live claim that won the race (read back after writing).
    lost_to: Option<UberClaim>,
}

/// Read, decide, write the file and the key, read back.
async fn claim_on(
    client: &async_nats::Client,
    kv: &async_nats::jetstream::kv::Store,
    here: &Here,
    now: u64,
) -> Result<Claimed, Failure> {
    let view = read_view(client, kv, here).await.map_err(no_broker)?;
    if let Some(w) = view.live_uber() {
        if here.mine(&w) {
            return Ok(Claimed {
                me: w,
                already: true,
                took_over: vec![],
                lost_to: None,
            });
        }
        return Err(exists(&w));
    }
    let claims = view.uber_claims();
    // Above every known epoch: a returning older claim must lose (§2).
    let epoch = claims
        .iter()
        .map(|c| c.epoch + 1)
        .max()
        .unwrap_or(0)
        .max(now);
    let record = uber_state::value(&here.pane, &here.host, (now / 1000) as i64, epoch);
    let io = |e: std::io::Error| Failure::new("state_io", e.to_string(), "check the state dir");
    uber_state::save(&here.file, &record).map_err(io)?;
    if let Err(e) = uber_state::publish(kv, &here.host, &record).await {
        let _ = uber_state::remove(&here.file);
        return Err(no_broker(e));
    }
    let me = UberClaim {
        pane: here.pane.clone(),
        host: here.host.clone(),
        epoch,
    };
    let view = read_view(client, kv, here).await.map_err(no_broker)?;
    Ok(Claimed {
        lost_to: view.live_uber().filter(|w| *w != me),
        me,
        already: false,
        took_over: claims.into_iter().filter(|c| !here.mine(c)).collect(),
    })
}

fn exists(w: &UberClaim) -> Failure {
    Failure::new(
        "uber_exists",
        format!("{} on {} is the Uber-UAC", w.pane, w.host),
        &format!(
            "one Uber per estate: vega uber release in {} on {}; then claim",
            w.pane, w.host
        ),
    )
}

fn conflict(me: &UberClaim, w: &UberClaim) -> Failure {
    Failure::new(
        "uber_conflict",
        format!(
            "{} on {} (you) is superseded by {} on {} (newer epoch {})",
            me.pane, me.host, w.pane, w.host, w.epoch
        ),
        "vega uber release",
    )
}

/// `w1:p1 on ganymede, epoch 1793019257367, 5m ago`.
fn line(c: &UberClaim, here: &Here) -> String {
    let you = if here.mine(c) { " (you)" } else { "" };
    let age = (now_ms().saturating_sub(c.epoch) / 1000) as i64;
    format!(
        "{} on {}{you}, epoch {}, {} ago",
        c.pane,
        c.host,
        c.epoch,
        ago(age)
    )
}

/// `45s`, `25m`, `2h05m`, `3d`.
fn ago(secs: i64) -> String {
    let secs = secs.max(0);
    match secs {
        0..60 => format!("{secs}s"),
        60..3600 => format!("{}m", secs / 60),
        3600..172800 => format!("{}h{:02}m", secs / 3600, secs % 3600 / 60),
        _ => format!("{}d", secs / 86400),
    }
}

fn claim() -> Result<(), Failure> {
    let here = here()?;
    let c = with_registry(async |client, kv| claim_on(client, kv, &here, now_ms()).await)?;
    if let Some(w) = &c.lost_to {
        return Err(conflict(&c.me, w));
    }
    let note = if c.already { ", already the Uber" } else { "" };
    println!("uber: {} on {} (you{note})", c.me.pane, c.me.host);
    println!("epoch: {}", c.me.epoch);
    for g in &c.took_over {
        println!("took_over: {} on {} (gone)", g.pane, g.host);
    }
    println!("help: vega uber status");
    Ok(())
}

fn release() -> Result<(), Failure> {
    let here = here()?;
    let Some(record) = uber_state::local(&here.file, &here.host) else {
        println!("released: no Uber claim on {} (no-op)", here.host);
        return Ok(());
    };
    let pane = record["pane"].as_str().unwrap_or("?");
    if pane != here.pane {
        return Err(Failure::new(
            "not_uber",
            format!("{pane} on {} holds the Uber claim, not you", here.host),
            "only the Uber releases itself; a gone claim is taken over by vega uber claim",
        ));
    }
    uber_state::remove(&here.file)
        .map_err(|e| Failure::new("state_io", e.to_string(), "check the state dir"))?;
    let key = vega_registry::uber_key(&here.host);
    match with_registry(async |_, kv| {
        uber_state::unpublish(kv, &here.host)
            .await
            .map_err(no_broker)
    }) {
        Ok(()) => println!("key: {key} deleted"),
        Err(f) => println!("key: vegad deletes {key} on reconnect ({})", f.message),
    }
    println!(
        "released: {} on {} is no longer the Uber-UAC",
        here.pane, here.host
    );
    Ok(())
}

/// The claims of `view`, newest first: (live winner, superseded, gone).
fn split(view: &View) -> (Option<UberClaim>, Vec<UberClaim>, Vec<UberClaim>) {
    let winner = view.live_uber();
    let mut claims = view.uber_claims();
    claims.sort_by(|a, b| b.epoch.cmp(&a.epoch));
    let (live, gone): (Vec<_>, Vec<_>) = claims.into_iter().partition(|c| view.uber_live(c));
    let superseded = live
        .into_iter()
        .filter(|c| Some(c) != winner.as_ref())
        .collect();
    (winner, superseded, gone)
}

fn status() -> Result<(), Failure> {
    let here = here()?;
    let view = match with_registry(async |client, kv| {
        read_view(client, kv, &here).await.map_err(no_broker)
    }) {
        Ok(v) => v,
        Err(f) => {
            println!(
                "uber: absent ({}; without a broker there is no Uber)",
                f.message
            );
            return Ok(());
        }
    };
    let (winner, superseded, gone) = split(&view);
    match &winner {
        Some(w) => {
            println!("uber: {}", line(w, &here));
            let now = (now_ms() / 1000) as i64;
            println!("engaged: {}", view.uber_engaged(w, now).line(now));
        }
        None => println!("uber: none (absent; the Uber session runs `vega uber claim`)"),
    }
    for c in &superseded {
        println!("superseded: {}", line(c, &here));
    }
    for c in &gone {
        println!("gone: {}", line(c, &here));
    }
    if let (Some(w), Some(me)) = (&winner, superseded.iter().find(|c| here.mine(c))) {
        return Err(conflict(me, w));
    }
    Ok(())
}

mod ask;

#[cfg(test)]
mod tests;
