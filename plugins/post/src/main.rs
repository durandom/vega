//! `vega post`: agent mail over a NATS JetStream broker (spike, see
//! docs/design/vega-mux-broker-spike.md).
//!
//! `send` publishes to `vega.inbox.<to-host>.<to-pane>.<from-host>`; the
//! stream `VEGA_INBOX_<to-host>` stores it. `relay` (vegad M1, see
//! docs/design/vegad.md) runs once per machine, watches `vega.inbox.<host>.>`
//! and types only a one-line pointer into the target pane, when its agent is
//! free (`delivery`, `agents`). The agent pulls the body with `inbox`, which acks it. The
//! return path travels in headers, and `inbox` renders it as a reply command.
//!
//! The streams are an archive (Limits retention): an ack only marks a message
//! as read. Unread = not yet acked by the pane's durable consumer. `log` reads
//! the archive without touching acks.
//!
//! One stream per host and the sender host as the last subject token keep
//! nkey permissions simple: a machine may publish only `vega.inbox.*.*.<self>`
//! and may only touch the JetStream API of `VEGA_INBOX_<self>`.

mod agents;
mod auth;
mod bdsync;
mod catalog;
#[cfg(test)]
mod catalog_broker_test;
mod ci;
mod cmd;
mod delivery;
mod gather;
mod gitsync;
mod human;
mod hygiene;
mod identity;
mod identity_mappings;
mod janitor;
mod notes;
mod outage;
mod pagerduty;
mod probes;
#[cfg(test)]
mod project_location_test;
mod quarantine;
mod quota;
mod reach;
#[cfg(test)]
mod registry_read_test;
mod service;
mod sidebar;
mod supervise;
mod theme;
mod uber;
mod uber_ask;
#[cfg(test)]
mod uber_broker_test;
mod uber_host;
mod upstream;

use std::collections::{HashMap, HashSet, VecDeque};
use std::env;
use std::process::ExitCode;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use async_nats::jetstream::{self, consumer::pull, stream};
use async_nats::HeaderMap;
use clap::error::ErrorKind;
use clap::{CommandFactory, Parser, Subcommand};
use delivery::{Decision, Delivery, Draft, Sender};
use futures_util::StreamExt;
use supervise::{Attentions, Supervisor};
use tokio::sync::mpsc;
use vega_herdr::state::{self as st, Outcome};
use vega_herdr::{cell, collapse_home, herdr, local_host, local_time, me, print_failure, Failure};

#[derive(Parser)]
#[command(
    name = "vega post",
    bin_name = "vega post",
    disable_version_flag = true,
    about = "Agent mail over a NATS JetStream broker (spike)",
    after_help = "Broker: VEGA_NATS_URL, else ~/.config/vega/nats.url (first line), else unconfigured; local dev: nats://127.0.0.1:4222.\nKey: VEGA_NATS_NKEY (default $XDG_CONFIG_HOME/vega/nats.nk if it exists, else no auth).\n\nExamples:\n  vega post                                   # broker, my pane, unread count\n  vega post send w3V:p2 \"ping\"                 # the target's relay types a pointer\n  vega post send --host ganymede w3V:p2 \"ping\"\n  vega post inbox                             # read and ack my mail\n  vega post log --since 1d                    # archive, newest first\n  vega post relay start                       # vegad, one per machine (herdr starts it)\n  vega post relay install                     # once per machine: herdr startup hook"
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
    /// Preview, audit and roll back the identity flag-day cutover; rekey after a remote change
    #[command(after_help = include_str!("../help/identity.md"))]
    Identity {
        #[command(subcommand)]
        action: identity::Action,
    },
    /// Send a message to an agent's inbox; its relay types a pointer into the pane
    #[command(
        override_usage = "vega post send [OPTIONS] <TARGET> <TEXT>\n       vega post send [OPTIONS] --uac <PROJECT> <TEXT>\n       vega post send [OPTIONS] --uber <TEXT>",
        after_help = "The relay types the pointer when the target agent is idle or done, one line for all its pending mail. Mail from the target's parent is typed while it works too (queued input), never into a dialog.\nsend checks the target in the registry first: a pane its host no longer lists fails with pane_gone and a hint naming the project's UAC; a host without a fresh registry entry gets the mail with a warning. A pane that closes with mail unread: its vegad bounces it once to each sender.\nWho is reachable: vega post and the registry (`vega uac`), not `herdr machine list` (only the machines this one saved).\n\nExamples:\n  vega post send w3V:p2 \"ping\"\n  vega post send --uac vega \"<text>\"                # the project's UAC, on whichever host\n  vega post send --uac . \"<text>\"                   # the UAC of this checkout's project\n  vega post send --uber --kind uber.withdraw '{\"id\":\"<id>\",\"reason\":\"obsolete\"}'   # the live Uber-UAC\n  vega post send --now w3V:p2 \"stop: wrong branch\"   # urgent: pointer at once\n  vega post send --host ganymede w3V:p2 \"ping\"\n  vega post send w3V:p1 --re <thread>/<hop>@<published-ms> \"pong\"   # reply command from `vega post inbox`"
    )]
    Send {
        /// Target pane id (e.g. w3V:p2), then the message text; with --uac only the text
        #[arg(value_names = ["TARGET", "TEXT"], num_args = 1..=2, required = true)]
        args: Vec<String>,
        /// Machine of the target pane [default: this machine]
        #[arg(long, conflicts_with = "uac")]
        host: Option<String>,
        /// Send to this project's UAC on any host: full project ID, published folder/origin/merged alias, repository name, or . (this checkout)
        #[arg(long, value_name = "PROJECT")]
        uac: Option<String>,
        /// Send to the live Uber-UAC claim (docs/design/uber-uac.md); uber_absent when none is live
        #[arg(long, conflicts_with_all = ["host", "uac"])]
        uber: bool,
        /// Typed mail: a Vega-Kind header (uber.ask|answer|applied|withdraw|brief) and a JSON
        /// body as the text, checked against its schema and size cap before sending
        #[arg(long)]
        kind: Option<String>,
        /// Reply token of the message you answer; copy it from `vega post inbox`
        #[arg(long)]
        re: Option<String>,
        /// Urgent: type the pointer at once, even while the target is working
        #[arg(long)]
        now: bool,
        /// Send as a local tool, not as a pane (no reply path), e.g. `vega review`'s inbox
        /// mailing the pane that started it; allowed to the own pane
        #[arg(long, hide = true)]
        from_tool: Option<String>,
    },
    /// Report your task as finished to the agent that spawned you, once per task
    #[command(
        after_help = "The parent's relay records the outcome and wakes the parent; `vega uac` shows it.\nWithout --to, the parent is the pane recorded by `vega mux spawn` on this machine.\nA later done replaces the earlier one.\n\nExamples:\n  vega post done --outcome succeeded \"login fix on branch fix-login, mise run check green\"\n  vega post done --outcome failed \"tests need a database; none on this host\"\n  vega post done --to w3V:p1 --host proteus --outcome succeeded \"<summary>\""
    )]
    Done {
        /// succeeded or failed
        #[arg(long, value_parser = ["succeeded", "failed"])]
        outcome: String,
        /// One-line result: what changed, where, gate results
        summary: String,
        /// Parent pane [default: the recorded parent]
        #[arg(long)]
        to: Option<String>,
        /// Machine of the parent pane [default: this machine]
        #[arg(long, requires = "to")]
        host: Option<String>,
    },
    /// Send a command to another machine's vegad and wait for its answer (JSON on stdin)
    #[command(
        after_help = "Core NATS request/reply on vega.cmd.<to>.<kind>.<this host>; the target's vegad runs the\nhandler for <kind> with the JSON on stdin (max 4 KiB). Kinds served: open, orchestration (authenticated wait updates).\nErrors: no_responder (no vegad there), timeout, denied (kind or payload refused, or no broker\nright), broker_unreachable, handler_failed.\n\nExamples:\n  echo '{\"kind\":\"open\",\"choice\":\"vscode\",\"path\":\"/Users/me/src/x/README.md\"}' | vega post cmd open --to proteus",
        name = "cmd"
    )]
    Command {
        /// Command kind, e.g. open
        kind: String,
        /// Target host
        #[arg(long)]
        to: String,
        /// How long to wait for the answer: 10s, 1m
        #[arg(long, default_value = "15s")]
        timeout: String,
    },
    /// Ask several agents one question; their answers come back as one pointer
    #[command(
        after_help = "All targets get the question on one thread. Your relay holds their answers until every target\nanswered or --within passed, then types one pointer; `vega post inbox` shows them all.\nA target is a pane id on this machine, or <pane>@<host> on another.\n\nExamples:\n  vega post ask --to w3V:p2 --to w5S:p1@ganymede \"Task, branch, bead, blockers? One line.\"\n  vega post ask --to w3V:p2 --within 30m \"<question>\""
    )]
    Ask {
        /// Target: <pane> on this machine or <pane>@<host>; repeat for each
        #[arg(long = "to", required = true)]
        to: Vec<String>,
        /// Release the answers after this long even if some are missing: 30m, 2h
        #[arg(long, default_value = "10m")]
        within: String,
        /// The question
        text: String,
    },
    /// Read and ack the messages in your inbox
    #[command(
        after_help = "A done report sent before your newer brief to that pane (spawn, adopt or mail) is marked stale.\n\nExamples:\n  vega post inbox\n  vega post inbox --keep      # read without ack; redelivered after 30 s\n  vega post inbox --show 138  # re-print one of your messages, acked or not"
    )]
    Inbox {
        /// Do not ack: the messages are delivered again later
        #[arg(long)]
        keep: bool,
        /// Re-print one message of your inbox by its id, acked or not; acks nothing
        #[arg(long, value_name = "MSG-ID", conflicts_with_all = ["keep", "max"])]
        show: Option<u64>,
        /// Max messages to read in one call
        #[arg(long, default_value_t = 20)]
        max: usize,
    },
    /// Show the archive of this machine's mail, newest first, without touching acks
    #[command(
        after_help = "Examples:\n  vega post log\n  vega post log --pane w3V:p17 --since 2h\n  vega post log --from ganymede --limit 50\n  vega post log --full --limit 1   # one message, untruncated"
    )]
    Log {
        /// Only mail to this pane on this machine
        #[arg(long)]
        pane: Option<String>,
        /// Only mail from this machine
        #[arg(long)]
        from: Option<String>,
        /// Only mail newer than this: 30m, 2h, 7d
        #[arg(long)]
        since: Option<String>,
        /// Max messages to show
        #[arg(long, default_value_t = 20)]
        limit: usize,
        /// Do not truncate message text
        #[arg(long)]
        full: bool,
    },
    /// List the notifications vega showed the human (all hosts), newest first
    #[command(
        after_help = "vegad keeps each toast it shows (drafts it held or could not restore) as a note: this host's store\n($XDG_STATE_HOME/vega/notes, 7 days, at most 200), published to the registry as <host>.note.<id>.\nEach UAC pane shows its project's unread count as \u{1f514}<n> (notes without a project, such as quarantine and\ndrafts, on the vega UAC); `vega board` lists them (N) and marks them read. A note with a cause (bd-sync:<DB>)\nresolves by itself when the cause clears (the project syncs again): read everywhere, still listed.\nread / clear move this host's watermarks per source host; without --upto, to the newest note.\n\nExamples:\n  vega post notes\n  vega post notes read\n  vega post notes clear --upto proteus=1791000000000\n  vega post notes --json --all"
    )]
    Notes {
        /// read: mark read (the badge clears); clear: also hide from the list
        #[arg(value_parser = ["read", "clear"])]
        action: Option<String>,
        /// With read or clear: only up to this note per host, <host>=<id>; repeat
        #[arg(long, value_name = "HOST=ID", requires = "action")]
        upto: Vec<String>,
        /// One JSON object per line (for the board)
        #[arg(long, conflicts_with = "action")]
        json: bool,
        /// Also list cleared notes
        #[arg(long, conflicts_with = "action")]
        all: bool,
    },
    /// Watch this machine's inboxes and type a pointer into each target pane
    #[command(
        args_conflicts_with_subcommands = true,
        after_help = "Types a pointer only when the target agent is idle or done (herdr agent status), one line for all pending mail; `send --now` skips the wait.\nAgent status: herdr event stream (socket), else `herdr agent list` polling; `relay status` shows the mode.\nOne relay per host: a lock (~/.local/state/vega/post-relay-<host>.lock) stops a second one.\nherdr starts it (plugin vega.vegad, linked by `relay install`); it outlives herdr and execs a replaced binary by itself.\n\nExamples:\n  vega post relay start          # start vegad detached unless it runs (herdr's startup hook does this)\n  vega post relay status         # running or not, pid, version, binary current/stale, last log lines\n  vega post relay install        # once per machine: herdr hook, pinned env, migrates a launchd/systemd unit\n  vega post relay                # foreground; fails if a relay already runs\n  vega post relay --once         # notify pending mail now (ignores agent status), then exit"
    )]
    Relay {
        /// Notify pending mail now, whatever the agent status, then exit
        #[arg(long)]
        once: bool,
        /// If a relay already runs, wait for it to exit instead of failing (used by the service)
        #[arg(long)]
        wait: bool,
        #[command(subcommand)]
        action: Option<RelayAction>,
    },
    /// Create this machine's nkey seed (or the admin seed) if missing and print its public key
    #[command(
        after_help = "The seed goes to VEGA_NATS_NKEY, else $XDG_CONFIG_HOME/vega/nats.nk (~/.config/vega/nats.nk), mode 0600.\nWith --admin: $XDG_CONFIG_HOME/vega/nats-admin.nk, the key for `auth-config --admin` and `provision`.\nAn existing seed is never overwritten; running keygen again prints the same public key.\nOnly the public key (U...) leaves the machine.\n\nExamples:\n  vega post keygen\n  vega post keygen --admin"
    )]
    Keygen {
        /// The admin seed (default $XDG_CONFIG_HOME/vega/nats-admin.nk), for `provision`
        #[arg(long)]
        admin: bool,
    },
    /// Print the nats.conf authorization block for host=U... public keys
    #[command(
        name = "auth-config",
        after_help = include_str!("../help/auth-config.md")
    )]
    AuthConfig {
        /// host=U... entries, as printed by `vega post keygen` on each host
        entries: Vec<String>,
        /// File with one host=U... per line
        #[arg(long)]
        file: Option<String>,
        /// Public key of the admin user (provision)
        #[arg(long)]
        admin: Option<String>,
        /// Hosts whose stream the admin may create now (comma-separated); default none
        #[arg(long, value_delimiter = ',')]
        create: Vec<String>,
        /// Hosts that may receive but not send mail (comma-separated), e.g. a corp-managed machine
        #[arg(long, value_delimiter = ',')]
        inbox_only: Vec<String>,
        /// Public key of a read-only viewer (e.g. the NATS UI NUI on the broker host)
        #[arg(long)]
        viewer: Option<String>,
        /// One exact command subject an inbox-only host may send: host=vega.cmd.<to>.<kind>.<host>; repeat
        #[arg(long = "cmd-grant")]
        cmd_grant: Vec<String>,
        /// Inbox-only hosts that may reply into quarantine (comma-separated): publish
        /// vega.quarantine.*.*.<host> only, shown to the human as a note, never to an agent
        #[arg(long, value_delimiter = ',')]
        quarantine: Vec<String>,
        /// One registry key an inbox-only host may read: host=<key>, e.g. deimos=proteus.theme; repeat
        #[arg(long = "registry-read")]
        registry_read: Vec<String>,
        /// The host that puts the lanista catalog (the builder, e.g. ganymede); every host reads it
        #[arg(long)]
        lanista_writer: Option<String>,
    },
    /// Create the archive streams for hosts (needs the admin key)
    #[command(
        after_help = "Connects with the admin seed: VEGA_NATS_NKEY, else $XDG_CONFIG_HOME/vega/nats-admin.nk (`vega post keygen --admin`).\nExisting streams are checked, never changed. Lists push consumers (vega never makes any).\n\nThe admin user can only: read stream info and list consumers of the hosts in the auth config, and create the\nstreams of hosts listed with `auth-config --create`. It cannot read mail, delete, purge or update streams;\nrepairs are manual on the server.\n\nNot hosts: `registry` creates vegad's KV bucket, `lanista` the catalog's object store (both need\n`auth-config --create <name>` while provisioning).\n\nExamples:\n  vega post provision proteus ganymede\n  vega post provision lanista"
    )]
    Provision {
        /// Host names, as used in host=U... entries
        #[arg(required = true)]
        hosts: Vec<String>,
    },
}

#[derive(Subcommand)]
enum RelayAction {
    /// Start vegad detached unless a relay runs (idempotent; herdr's startup hook and the board call it)
    #[command(
        after_help = "vegad runs in its own session, outlives herdr and logs to ~/.local/state/vega/post-relay-<host>.log.\nEnv: this shell's without pane variables and agent shims, plus ~/.local/state/vega/vegad.env (`relay install`).\nA host with a launchd/systemd unit from before (not migrated): the unit is kickstarted instead.\n\nExamples:\n  vega post relay start"
    )]
    Start,
    /// Stop the running relay (SIGTERM to the lock holder)
    Stop,
    /// Once per machine: herdr startup hook, pinned env, start; removes a launchd/systemd unit (idempotent)
    #[command(
        after_help = "Writes ~/.local/share/vega/herdr-plugin (plugin vega.vegad: [[startup]] `vega-post relay start`) and links it.\nVEGA_NATS_URL, VEGA_NATS_NKEY, VEGA_POST_HOST and HERDR_SOCKET_PATH go to ~/.local/state/vega/vegad.env if set.\nMigration: stops and removes ~/Library/LaunchAgents/vega.post-relay.plist or the systemd --user unit.\n\nExamples:\n  vega post relay install"
    )]
    Install,
    /// Stop the relay, remove the herdr startup hook, a legacy unit and the env file
    Uninstall,
    /// Show whether vegad runs: pid, version, binary current or stale, delivery mode, last log lines
    Status {
        /// One JSON object (the board reads it)
        #[arg(long)]
        json: bool,
    },
    /// Alias of start (startup hooks linked before vega-lhc.54 call it)
    #[command(hide = true)]
    Ensure,
}

const SKILL: &str = include_str!("../SKILL.md");

/// How agents call this binary in pointers and reply lines. `VEGA_POST_CMD`
/// overrides it, e.g. to test a dev build that is not on PATH.
fn cmd() -> String {
    env::var("VEGA_POST_CMD").unwrap_or_else(|_| "vega post".into())
}

fn main() -> ExitCode {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(e) if matches!(e.kind(), ErrorKind::DisplayHelp) => {
            let _ = e.print();
            return ExitCode::SUCCESS;
        }
        Err(e) => {
            // clap's first paragraph is the message; the rest is usage text.
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
                "valid usage below",
            ));
            let mut cmd = Cli::command();
            cmd.build();
            let sub = env::args().nth(1).filter(|a| !a.starts_with('-'));
            let help = match sub.as_deref().and_then(|s| cmd.find_subcommand_mut(s)) {
                Some(sc) => sc.render_help(),
                None => cmd.render_help(),
            };
            print!("{help}");
            return ExitCode::from(2);
        }
    };
    if cli.version {
        println!("{}", vega_build::version_line!());
        return ExitCode::SUCCESS;
    }
    if cli.skill {
        print!("{SKILL}");
        return ExitCode::SUCCESS;
    }
    let result = match cli.cmd {
        None => home(),
        Some(Cmd::Identity { action }) => identity::run(action),
        Some(Cmd::Send {
            args,
            host,
            uac,
            uber,
            kind,
            re,
            now,
            from_tool,
        }) => send_args(&args, host.as_deref(), uac.as_deref(), uber).and_then(|(to, text)| {
            let kind = mail_kind(kind.as_deref(), text)?;
            send(
                to,
                text,
                re.as_deref(),
                now,
                None,
                kind,
                from_tool.as_deref(),
            )
        }),
        Some(Cmd::Done {
            outcome,
            summary,
            to,
            host,
        }) => done(&outcome, &summary, to.as_deref(), host.as_deref()),
        Some(Cmd::Ask { to, within, text }) => ask(&to, &within, &text),
        Some(Cmd::Command { kind, to, timeout }) => send_cmd(&kind, &to, &timeout),
        Some(Cmd::Inbox { keep, max, show }) => match show {
            Some(id) => inbox_show(id),
            None => inbox(keep, max),
        },
        Some(Cmd::Log {
            pane,
            from,
            since,
            limit,
            full,
        }) => log_view(
            pane.as_deref(),
            from.as_deref(),
            since.as_deref(),
            limit,
            full,
        ),
        Some(Cmd::Notes {
            action,
            upto,
            json,
            all,
        }) => notes::run(action.as_deref(), &upto, json, all),
        Some(Cmd::Relay { action, once, wait }) => match action {
            None => relay(once, wait),
            Some(RelayAction::Start | RelayAction::Ensure) => service::start(),
            Some(RelayAction::Stop) => service::stop(),
            Some(RelayAction::Install) => service::install(),
            Some(RelayAction::Uninstall) => service::uninstall(),
            Some(RelayAction::Status { json }) => service::status(json),
        },
        Some(Cmd::Keygen { admin }) => auth::keygen(if admin { Seed::Admin } else { Seed::Host }),
        Some(Cmd::AuthConfig {
            entries,
            file,
            admin,
            create,
            inbox_only,
            viewer,
            cmd_grant,
            quarantine,
            registry_read,
            lanista_writer,
        }) => auth_config(
            &entries,
            file.as_deref(),
            admin.as_deref(),
            &create,
            &inbox_only,
            viewer.as_deref(),
            &cmd_grant,
            &quarantine,
            &registry_read,
            lanista_writer.as_deref(),
        ),
        Some(Cmd::Provision { hosts }) => provision(&hosts),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(f) => {
            print_failure(&f);
            ExitCode::from(if f.code == "usage" { 2 } else { 1 })
        }
    }
}

/// Archive limits per host stream. Mail is kept, read or not, until it is
/// older than ARCHIVE_MAX_AGE or the stream exceeds ARCHIVE_MAX_BYTES; then
/// the oldest messages go first.
const ARCHIVE_MAX_AGE: Duration = Duration::from_secs(365 * 24 * 3600);
const ARCHIVE_MAX_BYTES: i64 = 1 << 30; // 1 GiB
/// `log` truncates message text to this many characters.
const LOG_TEXT: usize = 80;
/// A chain of fast replies longer than this is treated as an agent loop.
const MAX_HOPS: u32 = 8;
/// Replies at least this long after the answered message restart the counter.
const FAST_REPLY_WINDOW: Duration = Duration::from_secs(120);
/// An unacked message is redelivered by `inbox` after this time.
const ACK_WAIT: Duration = Duration::from_secs(30);
/// A child free without a report is confirmed silent after this long, so
/// the status bar can show a background shell first.
const SILENT_AFTER: Duration = Duration::from_secs(3);
/// `send` keeps trying to reach the broker this long, e.g. across a restart.
const SEND_WAIT: Duration = Duration::from_secs(10);

/// Header names. `Nats-Msg-Id` makes a retried publish idempotent.
mod h {
    pub const FROM_PANE: &str = "Vega-From-Pane";
    pub const FROM_AGENT: &str = "Vega-From-Agent";
    pub const FROM_CWD: &str = "Vega-From-Cwd";
    pub const SENT_MS: &str = "Vega-Sent-Ms";
    pub const THREAD: &str = "Vega-Thread";
    pub const HOP: &str = "Vega-Hop";
    /// Set by `send --now`: the relay types the pointer at once.
    pub const NOW: &str = "Vega-Now";
    /// Set by `done`: the outcome; the parent's relay records it.
    pub const DONE: &str = "Vega-Done";
    /// Set by `send --kind`: a typed JSON body (vega_uber_msg).
    pub const KIND: &str = vega_uber_msg::HEADER;
}

/// `VEGA_NATS_URL`, else the first line of `$XDG_CONFIG_HOME/vega/nats.url`
/// (for machines without Tailscale DNS, where only the IP works), else unconfigured.
fn url() -> String {
    vega_herdr::service_config::get("VEGA_NATS_URL", "nats.url").unwrap_or_default()
}

/// True when a broker is set (`VEGA_NATS_URL` or `nats.url`), with no implicit estate default.
fn broker_configured() -> bool {
    !url().is_empty()
}

/// `$XDG_CONFIG_HOME`, else `~/.config`.
fn config_dir() -> Option<String> {
    env::var("XDG_CONFIG_HOME")
        .ok()
        .filter(|c| !c.is_empty())
        .or_else(|| env::var("HOME").ok().map(|h| format!("{h}/.config")))
}

/// `VEGA_NATS_NKEY`, ignoring an empty value.
fn nkey_env() -> Option<String> {
    env::var("VEGA_NATS_NKEY").ok().filter(|p| !p.is_empty())
}

/// Where this machine's nkey seed lives: `VEGA_NATS_NKEY`, else
/// `$XDG_CONFIG_HOME/vega/nats.nk` (default `~/.config/vega/nats.nk`).
fn seed_path(kind: Seed) -> Result<String, Failure> {
    if let Some(p) = nkey_env() {
        return Ok(p);
    }
    let config = config_dir().ok_or_else(|| {
        Failure::new(
            "no_home",
            "neither XDG_CONFIG_HOME nor HOME is set",
            "set VEGA_NATS_NKEY to the seed file",
        )
    })?;
    Ok(format!("{config}/vega/{}", kind.file()))
}

/// Which seed: this machine's host identity, or the admin that provisions
/// streams. Both stay on the machine they were made on.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Seed {
    Host,
    Admin,
}

impl Seed {
    fn file(self) -> &'static str {
        match self {
            Seed::Host => "nats.nk",
            Seed::Admin => "nats-admin.nk",
        }
    }
}

/// The seed to connect with: `VEGA_NATS_NKEY` (must exist), else the
/// default seed if it exists, else none: no auth (local dev broker).
fn nkey_path(kind: Seed) -> Option<String> {
    if let Some(p) = nkey_env() {
        return Some(p);
    }
    seed_path(kind)
        .ok()
        .filter(|p| std::path::Path::new(p).exists())
}

/// Live view: broker, my pane, unread count.
fn home() -> Result<(), Failure> {
    let bin = env::current_exe()
        .map(|p| collapse_home(&p.display().to_string()))
        .unwrap_or_else(|_| "unknown".into());
    println!("bin: {bin}");
    println!("description: Agent mail over a NATS JetStream broker (spike)");
    let broker = url();
    println!(
        "broker: {}",
        if broker.is_empty() {
            "unconfigured"
        } else {
            &broker
        }
    );
    println!(
        "key: {}",
        nkey_path(Seed::Host)
            .map(|p| collapse_home(&p))
            .unwrap_or_else(|| "none (no auth)".into())
    );
    let host = host();
    println!("host: {host}");
    let me = me()?;
    println!("me: {} ({}, {})", me.pane, me.agent, cell(&me.cwd));
    let unread = runtime()?.block_on(async {
        let client = connect("home", None).await?;
        let stream = inbox_stream(&jetstream::new(client), &host).await?;
        unread(&stream, &host, &me.pane).await
    })?;
    println!("unread: {unread}");
    println!("help[2]:");
    println!("  vega post send <pane> \"<text>\"   # the reply arrives as a pointer");
    if unread > 0 {
        println!("  vega post inbox                  # read and ack your mail");
    } else {
        println!("  vega post log                    # archive, newest first");
    }
    Ok(())
}

fn runtime() -> Result<tokio::runtime::Runtime, Failure> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| Failure::new("runtime", e.to_string(), "report this as a vega bug"))
}

fn broker_err(e: impl std::fmt::Display) -> Failure {
    Failure::new(
        "broker_error",
        e.to_string(),
        "check the broker: `nats --server $VEGA_NATS_URL server check jetstream`",
    )
}

/// The host token in subjects: short hostname, lowercase, no dots.
/// `VEGA_POST_HOST` overrides it (tests, odd hostnames); the server still
/// enforces it through the key's permissions.
fn host() -> String {
    let h = env::var("VEGA_POST_HOST")
        .ok()
        .filter(|h| !h.is_empty())
        .unwrap_or_else(local_host);
    subject_token(&h.to_lowercase())
}

/// NATS subject tokens must not contain `.`, `*`, `>` or whitespace.
fn subject_token(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c == '.' || c == '*' || c == '>' || c.is_whitespace() {
                '_'
            } else {
                c
            }
        })
        .collect()
}

/// `vega.inbox.<to-host>.<to-pane>.<from-host>`.
fn inbox_subject(to_host: &str, pane: &str, from_host: &str) -> String {
    format!(
        "vega.inbox.{}.{}.{}",
        subject_token(to_host),
        subject_token(pane),
        subject_token(from_host)
    )
}

/// All mail for one pane, from any host.
fn pane_filter(host: &str, pane: &str) -> String {
    format!(
        "vega.inbox.{}.{}.*",
        subject_token(host),
        subject_token(pane)
    )
}

/// `(to-pane, from-host)` from an inbox subject. The from-host is enforced
/// by the sender's publish permission, unlike the `Vega-From-*` headers.
fn parse_subject(subject: &str) -> Option<(&str, &str)> {
    let mut t = subject.strip_prefix("vega.inbox.")?.split('.');
    let (_to_host, pane, from) = (t.next()?, t.next()?, t.next()?);
    t.next().is_none().then_some((pane, from))
}

/// Reply-subject prefix of admin connections (`provision`).
const ADMIN_INBOX: &str = "_INBOX_admin";
/// Reply-subject prefix the viewer (NUI) must be configured with.
const VIEWER_INBOX: &str = "_INBOX_viewer";

/// Reply-subject prefix of this host's connections.
fn inbox_prefix(host: &str) -> String {
    format!("_INBOX_{}", subject_token(host))
}

/// Per-host stream: stream names must not contain `.`, `*`, `>`.
fn stream_name(host: &str) -> String {
    format!("VEGA_INBOX_{}", subject_token(host))
}

/// JetStream consumer names must not contain `.`, `*`, `>`, `/`, `\`, `:` is
/// avoided too for portability.
fn consumer_name(host: &str, pane: &str) -> String {
    let safe = |s: &str| -> String {
        s.chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' {
                    c
                } else {
                    '_'
                }
            })
            .collect()
    };
    format!("inbox-{}-{}", safe(host), safe(pane))
}

/// Parses `--re <thread>/<hop>[@<published-ms>]` at send time. Legacy tokens
/// lack timing information and keep counting conservatively.
fn next_hop(re: Option<&str>, now: u128) -> Result<(String, u32), Failure> {
    let Some(re) = re else {
        return Ok((nuid::next().to_string(), 1));
    };
    let bad = || {
        Failure::new(
            "usage",
            format!("--re {re} is not <thread>/<hop>[@<published-ms>]"),
            "copy the reply command from `vega post inbox` verbatim",
        )
    };
    let (thread, hop) = re.rsplit_once('/').ok_or_else(bad)?;
    let (hop, published) = match hop.split_once('@') {
        Some((hop, ms)) => (hop, Some(ms.parse::<u128>().map_err(|_| bad())?)),
        None => (hop, None),
    };
    let hop: u32 = hop.parse().map_err(|_| bad())?;
    if thread.is_empty() {
        return Err(bad());
    }
    // A future timestamp (clock skew) cannot reset a fast chain.
    if published.is_some_and(|ms| now.saturating_sub(ms) >= FAST_REPLY_WINDOW.as_millis()) {
        return Ok((thread.to_string(), 1));
    }
    if hop >= MAX_HOPS {
        let hint = if published.is_some() {
            "stop automatic replies and tell the user; a substantive reply can resume with the same timestamped --re token after 120 s from the answered message"
        } else {
            "stop automatic replies and tell the user; this legacy token has no timestamp and stays capped; re-read `vega post inbox --show <id>` on an upgraded host for a timestamped reply command"
        };
        return Err(Failure::new(
            "hop_limit",
            format!("thread {thread} reached {hop} fast hops (max {MAX_HOPS})"),
            hint,
        ));
    }
    Ok((thread.to_string(), hop + 1))
}

/// Connects to the broker. `reconnected` gets a signal on every (re)connect,
/// so the relay can rescan mail it missed while the broker was away.
async fn connect(
    role: &str,
    reconnected: Option<mpsc::UnboundedSender<()>>,
) -> Result<async_nats::Client, Failure> {
    connect_as(role, Seed::Host, reconnected).await
}

/// Set once this process is vegad (`relay`): a denied right is then logged,
/// never `exit(1)` as a CLI command does.
static DAEMON: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn enter_daemon() {
    DAEMON.store(true, std::sync::atomic::Ordering::Relaxed);
}

fn is_daemon() -> bool {
    DAEMON.load(std::sync::atomic::Ordering::Relaxed)
}

/// One log line per distinct denial: the catalog reconnects every 15 min.
fn log_denied_once(msg: &str) {
    static SAID: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
    let Ok(mut said) = SAID.lock() else { return };
    if !said.iter().any(|m| m == msg) {
        said.push(msg.to_string());
        log(&format!("broker right missing, feature off: {msg}"));
    }
}

async fn connect_as(
    role: &str,
    seed: Seed,
    reconnected: Option<mpsc::UnboundedSender<()>>,
) -> Result<async_nats::Client, Failure> {
    let url = url();
    if url.is_empty() {
        return Err(Failure::new(
            "broker_unreachable",
            "no broker configured",
            unreachable_hint(false),
        ));
    }
    let inbox_only = vega_herdr::state::is_inbox_only(&host());
    let mut opts = match nkey_path(seed) {
        Some(path) => {
            let seed = std::fs::read_to_string(&path).map_err(|e| {
                Failure::new(
                    "nkey_unreadable",
                    format!("{path}: {e}"),
                    "point VEGA_NATS_NKEY at this machine's user seed file",
                )
            })?;
            async_nats::ConnectOptions::with_nkey(seed.trim().to_string())
        }
        None => async_nats::ConnectOptions::new(),
    };
    opts = opts
        .name(format!("vega-post {role} {}", host()))
        // Replies (JetStream API answers, fetched messages) go to
        // `_INBOX_<host>.…`; each host may subscribe only to its own prefix,
        // so no host can read another host's fetches.
        .custom_inbox_prefix(match seed {
            Seed::Host => inbox_prefix(&host()),
            Seed::Admin => ADMIN_INBOX.to_string(),
        })
        .connection_timeout(Duration::from_secs(2))
        .request_timeout(Some(Duration::from_secs(3)))
        // A broker host that vanishes sends no FIN: vegad's pings notice it
        // within ~30 s instead of the default ~3 min (vega-lhc.205).
        .ping_interval(if role == "relay" {
            Duration::from_secs(10)
        } else {
            Duration::from_secs(60)
        })
        // The server reports a denied publish or subscribe asynchronously; the
        // request itself only times out. Fail fast with the real reason.
        .event_callback(move |event| {
            let reconnected = reconnected.clone();
            async move {
                match event {
                    async_nats::Event::Connected => {
                        if let Some(tx) = reconnected {
                            let _ = tx.send(());
                        }
                    }
                    async_nats::Event::Disconnected => log("broker: disconnected"),
                    // The relay's command subscription without the broker right
                    // (titan auth not updated yet): mail keeps working.
                    async_nats::Event::ServerError(async_nats::ServerError::Other(msg))
                        if msg.contains("Subscription to \"vega.cmd.")
                            || msg.contains("Subscription to \"vega.quarantine.")
                            || msg.contains("VEGA_REGISTRY") =>
                    {
                        log(&format!("broker right missing, feature off: {msg}"));
                    }
                    // vegad runs on: one thread's missing right (the lanista
                    // catalog before its grant) must not end mail (vega-lhc.140).
                    async_nats::Event::ServerError(async_nats::ServerError::Other(msg))
                        if is_daemon() && msg.to_lowercase().contains("permissions violation") =>
                    {
                        log_denied_once(&msg);
                    }
                    async_nats::Event::ServerError(async_nats::ServerError::Other(msg))
                        if msg.to_lowercase().contains("permissions violation") =>
                    {
                        print_failure(&match seed {
                            Seed::Host => quarantine::denied(&msg, inbox_only),
                            Seed::Admin => Failure::new("permission_denied", msg, "the admin may create a stream only for hosts listed with `vega post auth-config --create <host>`; regenerate the config with it, reload the server, run provision, then drop --create again"),
                        });
                        std::process::exit(1);
                    }
                    _ => {}
                }
            }
        });
    opts.connect(&url).await.map_err(|e| {
        if e.to_string().contains("authorization violation") {
            return Failure::new(
                "auth_failed",
                format!("{url} rejected this machine's key"),
                "run `vega post keygen` and add its host=U... line to the server's auth config (`vega post auth-config`)",
            );
        }
        Failure::new(
            "broker_unreachable",
            format!("cannot connect to {url}: {e}"),
            unreachable_hint(broker_configured()),
        )
    })
}

/// The hint of `broker_unreachable`, distinguishing missing configuration.
fn unreachable_hint(configured: bool) -> &'static str {
    if configured {
        "check VEGA_NATS_URL / ~/.config/vega/nats.url and the network; for a local dev broker run `mise run nats` in the vega repo and set VEGA_NATS_URL=nats://127.0.0.1:4222"
    } else {
        "no broker configured (VEGA_NATS_URL, ~/.config/vega/nats.url); `vega onboard` lists the options. Without a broker, vegad local mode requires a completed identity migration; otherwise startup stays in identity quiesce"
    }
}

fn stream_config(host: &str) -> stream::Config {
    stream::Config {
        name: stream_name(host),
        subjects: vec![format!("vega.inbox.{}.>", subject_token(host))],
        // An archive: acks mark mail read, they do not delete it.
        retention: stream::RetentionPolicy::Limits,
        max_age: ARCHIVE_MAX_AGE,
        max_bytes: ARCHIVE_MAX_BYTES,
        discard: stream::DiscardPolicy::Old,
        storage: stream::StorageType::File,
        ..Default::default()
    }
}

/// This host's inbox stream. With a key (production) only the admin creates
/// streams (`vega post provision`): a machine allowed to create its own
/// stream could also give it the subjects of every other host. Without a key
/// (local dev broker) it is created on first use.
async fn inbox_stream(js: &jetstream::Context, host: &str) -> Result<stream::Stream, Failure> {
    let stream = match js.get_stream(stream_name(host)).await {
        Ok(s) => s,
        Err(e) if stream_missing(&e) => {
            if nkey_path(Seed::Host).is_some() {
                return Err(Failure::new(
                    "no_inbox_stream",
                    format!("{} does not exist on the broker", stream_name(host)),
                    &format!("machine keys cannot create streams; the admin must run `vega post provision {host}`"),
                ));
            }
            js.get_or_create_stream(stream_config(host))
                .await
                .map_err(broker_err)?
        }
        Err(e) => return Err(broker_err(e)),
    };
    check_archive(&stream, host)?;
    Ok(stream)
}

fn stream_missing(e: &jetstream::context::GetStreamError) -> bool {
    matches!(
        e.kind(),
        jetstream::context::GetStreamErrorKind::JetStream(err)
            if err.error_code() == jetstream::ErrorCode::STREAM_NOT_FOUND
    )
}

fn check_archive(stream: &stream::Stream, host: &str) -> Result<(), Failure> {
    // JetStream cannot change retention in place.
    if stream.cached_info().config.retention != stream::RetentionPolicy::Limits {
        return Err(Failure::new(
            "stream_outdated",
            format!("{} is not an archive (retention is not limits)", stream_name(host)),
            "an admin must delete the stream (`nats stream rm <name>`) and run `vega post provision <host>`",
        ));
    }
    Ok(())
}

/// `vega post provision`: creates missing host streams, checks existing ones.
fn provision(hosts: &[String]) -> Result<(), Failure> {
    runtime()?.block_on(async {
        let client = connect_as("provision", Seed::Admin, None).await?;
        let js = jetstream::new(client);
        // `registry`: the KV bucket of vegad M4, not a host; `lanista`: the
        // catalog's object store (catalog.rs).
        let wants_registry = hosts.iter().any(|h| h == "registry");
        let wants_lanista = hosts.iter().any(|h| h == "lanista");
        let hosts: Vec<String> = hosts
            .iter()
            .filter(|h| *h != "registry" && *h != "lanista")
            .cloned()
            .collect();
        if wants_lanista {
            let bucket = vega_herdr::catalog::BUCKET;
            let existed = js.get_object_store(bucket).await.is_ok();
            if !existed {
                js.create_object_store(catalog::bucket_config())
                    .await
                    .map_err(broker_err)?;
            }
            println!("lanista: {bucket} {}", if existed { "exists" } else { "created" });
        }
        if wants_registry {
            let existed = js.get_key_value(vega_registry::BUCKET).await.is_ok();
            if !existed {
                js.create_key_value(jetstream::kv::Config {
                    bucket: vega_registry::BUCKET.into(),
                    history: 1,
                    max_bytes: 64 << 20,
                    ..Default::default()
                })
                .await
                .map_err(broker_err)?;
            }
            println!("registry: {} {}", vega_registry::BUCKET, if existed { "exists" } else { "created" });
        }
        println!("streams[{}]{{host,stream,state}}:", hosts.len());
        let mut push = Vec::new();
        for h in hosts {
            let h = subject_token(&h.to_lowercase());
            let existed = js.get_stream(stream_name(&h)).await.is_ok();
            let stream = js
                .get_or_create_stream(stream_config(&h))
                .await
                .map_err(broker_err)?;
            check_archive(&stream, &h)?;
            println!(
                "  {h},{},{}",
                stream_name(&h),
                if existed { "exists" } else { "created" }
            );
            // vega only makes pull consumers. A push consumer delivers to
            // any subject it names, published by the server, past the
            // host's publish permissions; permissions cannot forbid it.
            let mut consumers = stream.consumers();
            while let Some(c) = consumers.next().await {
                let c = c.map_err(broker_err)?;
                if let Some(target) = &c.config.deliver_subject {
                    push.push(format!("  {},{},{}", stream_name(&h), c.name, cell(target)));
                }
            }
        }
        if !push.is_empty() {
            println!("push_consumers[{}]{{stream,consumer,deliver_subject}}:", push.len());
            for p in &push {
                println!("{p}");
            }
            println!("warning: vega never creates push consumers; delete these with `nats consumer rm <stream> <consumer>` and check the host's key");
        }
        println!("help: machine keys can now read their own stream; run `vega post` on each host");
        Ok(())
    })
}

/// `vega post auth-config`: prints the authorization block.
#[allow(clippy::too_many_arguments)]
fn auth_config(
    entries: &[String],
    file: Option<&str>,
    admin: Option<&str>,
    create: &[String],
    inbox_only: &[String],
    viewer: Option<&str>,
    cmd_grant: &[String],
    quarantine: &[String],
    registry_read: &[String],
    lanista_writer: Option<&str>,
) -> Result<(), Failure> {
    let entries = auth::read_entries(entries, file)?;
    if let Some(h) = quarantine.iter().find(|h| !inbox_only.contains(h)) {
        return Err(Failure::new(
            "usage",
            format!("--quarantine {h}: only inbox-only hosts reply into quarantine"),
            "list it with --inbox-only too; other hosts send mail as themselves",
        ));
    }
    let grants = cmd_grant
        .iter()
        .map(|g| auth::parse_grant(g))
        .collect::<Result<Vec<_>, _>>()?;
    if let Some((h, _)) = grants.iter().find(|(h, _)| !inbox_only.contains(h)) {
        return Err(Failure::new(
            "usage",
            format!("--cmd-grant for {h}: only inbox-only hosts take grants"),
            "other hosts may already send commands as themselves",
        ));
    }
    let reads = registry_read
        .iter()
        .map(|g| auth::parse_registry_read(g))
        .collect::<Result<Vec<_>, _>>()?;
    if let Some((h, _)) = reads.iter().find(|(h, _)| !inbox_only.contains(h)) {
        return Err(Failure::new(
            "usage",
            format!("--registry-read for {h}: only inbox-only hosts take registry grants"),
            "other hosts already read the whole registry",
        ));
    }
    for (flag, hosts) in [("--create", create), ("--inbox-only", inbox_only)] {
        if let Some(h) = hosts
            .iter()
            .filter(|h| !(flag == "--create" && (*h == "registry" || *h == "lanista")))
            .find(|h| !entries.iter().any(|e| &e.host == *h))
        {
            return Err(Failure::new(
                "usage",
                format!("{flag} {h}: not a host in this config"),
                "list the host as host=U... too",
            ));
        }
    }
    if let Some(w) = lanista_writer {
        if !entries.iter().any(|e| e.host == w) || inbox_only.iter().any(|h| h == w) {
            return Err(Failure::new(
                "usage",
                format!("--lanista-writer {w}: not a host in this config, or inbox-only"),
                "name the builder host (ganymede) and list it as host=U...",
            ));
        }
    }
    if !create.is_empty() && admin.is_none() {
        return Err(Failure::new(
            "usage",
            "--create needs --admin",
            "pass the admin public key from `vega post keygen --admin`",
        ));
    }
    if let Some(a) = admin {
        auth::parse_entry(&format!("admin={a}"))?;
        if entries.iter().any(|e| e.key == a) {
            return Err(Failure::new(
                "usage",
                "the admin key is also a host key",
                "generate a separate admin seed, e.g. VEGA_NATS_NKEY=<path> vega post keygen",
            ));
        }
    }
    if entries.len() > auth::MAX_HOSTS_8GB {
        eprintln!(
            "warning: {} hosts reserve {} GiB; orrery's max_file_store 8GB fits {}",
            entries.len(),
            entries.len() as i64 * ARCHIVE_MAX_BYTES / (1 << 30),
            auth::MAX_HOSTS_8GB
        );
    }
    if let Some(v) = viewer {
        auth::parse_entry(&format!("viewer={v}"))?;
        if entries.iter().any(|e| e.key == v) || admin == Some(v) {
            return Err(Failure::new(
                "usage",
                "the viewer key is also a host or admin key",
                "the viewer needs its own seed, generated where the viewer runs",
            ));
        }
    }
    print!(
        "{}",
        auth::render(
            &entries,
            admin,
            create,
            inbox_only,
            viewer,
            &grants,
            quarantine,
            &reads,
            lanista_writer,
        )
    );
    Ok(())
}

/// The durable consumer that tracks what one pane has read. It never
/// expires: a recreated consumer would deliver the whole archive as unread.
fn inbox_consumer(host: &str, pane: &str) -> pull::Config {
    pull::Config {
        durable_name: Some(consumer_name(host, pane)),
        filter_subject: pane_filter(host, pane),
        ack_policy: jetstream::consumer::AckPolicy::Explicit,
        ack_wait: ACK_WAIT,
        ..Default::default()
    }
}

/// Unread mail of one pane: not yet acked by its durable consumer (pending
/// plus delivered-but-unacked). Without a consumer the pane never read
/// anything, so all its mail counts.
async fn unread(stream: &stream::Stream, host: &str, pane: &str) -> Result<u64, Failure> {
    Ok(unread_and_floor(stream, host, pane).await?.0)
}

/// Unread count and the consumer's ack floor (stream sequence; 0 if the
/// pane never read). A higher floor than before means the agent read mail.
async fn unread_and_floor(
    stream: &stream::Stream,
    host: &str,
    pane: &str,
) -> Result<(u64, u64), Failure> {
    match stream.consumer_info(consumer_name(host, pane)).await {
        Ok(info) => Ok((
            info.num_pending + info.num_ack_pending as u64,
            info.ack_floor.stream_sequence,
        )),
        Err(e) if e.kind() == jetstream::context::ConsumerInfoErrorKind::NotFound => Ok((
            subject_counts(stream, &pane_filter(host, pane))
                .await?
                .iter()
                .map(|(_, n)| *n as u64)
                .sum(),
            0,
        )),
        Err(e) => Err(broker_err(e)),
    }
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

/// Where `send` mails: a pane on a host (default this one), or the UAC of a
/// project on whichever host the registry shows it (`--uac`).
#[derive(Clone, Copy, Debug)]
enum To<'a> {
    Pane(&'a str, Option<&'a str>),
    Uac(&'a str),
    Uber,
}

/// `send`'s positionals: target and text, or with `--uac` only the text.
fn send_args<'a>(
    args: &'a [String],
    host: Option<&'a str>,
    uac: Option<&'a str>,
    uber: bool,
) -> Result<(To<'a>, &'a str), Failure> {
    match (uac, args) {
        (None, [text]) if uber => Ok((To::Uber, text)),
        (None, _) if uber => Err(Failure::new(
            "usage",
            "with --uber give only the text",
            "vega post send --uber \"<text>\"",
        )),
        (Some(p), [text]) => Ok((To::Uac(p), text)),
        (None, [target, text]) => Ok((To::Pane(target, host), text)),
        (Some(_), _) => Err(Failure::new(
            "usage",
            "with --uac give only the text",
            "vega post send --uac <project> \"<text>\"",
        )),
        (None, _) => Err(Failure::new(
            "usage",
            "give the target pane and the text",
            "vega post send <pane> \"<text>\", or vega post send --uac <project> \"<text>\"",
        )),
    }
}

/// `send --kind`: the body must pass its kind's schema and cap, else
/// nothing is sent (docs/design/uber-uac.md §3).
fn mail_kind(kind: Option<&str>, text: &str) -> Result<Option<vega_uber_msg::Kind>, Failure> {
    let Some(k) = kind else { return Ok(None) };
    vega_uber_msg::validate_kind(k, text.as_bytes())
        .map(|_| vega_uber_msg::Kind::parse(k))
        .map_err(|r| {
            Failure::new(
                r.code,
                format!("{}; nothing was sent", r.message),
                "fix the JSON body; schemas and caps: docs/design/uber-uac.md §3",
            )
        })
}

/// The mail headers `send` sets.
fn send_headers(
    me: &vega_herdr::Me,
    thread: &str,
    hop: u32,
    now: bool,
    done: Option<Outcome>,
    kind: Option<vega_uber_msg::Kind>,
) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert("Nats-Msg-Id", nuid::next().as_str());
    if !me.pane.is_empty() {
        headers.insert(h::FROM_PANE, me.pane.as_str());
    }
    headers.insert(h::FROM_AGENT, me.agent.as_str());
    headers.insert(h::FROM_CWD, me.cwd.as_str());
    headers.insert(h::SENT_MS, now_ms().to_string().as_str());
    headers.insert(h::THREAD, thread);
    headers.insert(h::HOP, hop.to_string().as_str());
    if now {
        headers.insert(h::NOW, "1");
    }
    if let Some(o) = done {
        headers.insert(h::DONE, o.as_str());
    }
    if let Some(k) = kind {
        headers.insert(h::KIND, k.as_str());
    }
    headers
}

/// The project of the directory `send` runs in (its UAC key).
fn my_project() -> Result<Option<String>, Failure> {
    let cwd =
        env::current_dir().map_err(|e| Failure::new("project_io", e.to_string(), "check cwd"))?;
    vega_herdr::project::resolve_optional(&cwd, &st::post_host())
        .map(|p| p.map(|p| p.id.to_string()))
}

/// Whether this host's herdr lists the pane; `None` when herdr is away.
fn local_pane_open(pane: &str) -> Option<bool> {
    let v = herdr(&["agent", "list"]).ok()?;
    let agents = v["result"]["agents"].as_array()?;
    Some(agents.iter().any(|a| a["pane_id"] == pane))
}

/// The pane and host to mail, checked in the registry (reach.rs). `--uac`
/// resolves the project's UAC from one registry read. A pane its fresh host
/// no longer lists fails with `pane_gone`; when the registry cannot tell
/// (stale host, no answer), the warning comes back and the mail goes.
async fn resolve_target(
    client: &async_nats::Client,
    to: To<'_>,
    check: bool,
) -> Result<(String, String, Option<String>), Failure> {
    let my_host = host();
    let (target, to_host) = match to {
        To::Uac(query) => {
            let view = tokio::time::timeout(reach::WAIT, reach::read(client))
                .await
                .unwrap_or_else(|_| Err("no answer within 2 s".into()))
                .map_err(|e| {
                    Failure::new(
                        "registry_unavailable",
                        format!("cannot resolve --uac {query}: registry: {e}"),
                        "send to the pane itself: vega post send [--host <host>] <pane> \"<text>\"",
                    )
                })?;
            let here = if query == "." { my_project()? } else { None };
            let (h, p, _) = reach::resolve(&view, query, here.as_deref())?;
            return Ok((p, subject_token(&h), None));
        }
        To::Uber => {
            let view = tokio::time::timeout(reach::WAIT, reach::read(client))
                .await
                .unwrap_or_else(|_| Err("no answer within 2 s".into()))
                .map_err(|e| {
                    Failure::new(
                        "registry_unavailable",
                        format!("cannot resolve --uber: registry: {e}"),
                        "without the registry there is no Uber-UAC; use the bead and the board",
                    )
                })?;
            let c = view.live_uber().ok_or_else(|| {
                Failure::new(
                    "uber_absent",
                    "no live Uber-UAC claim; nothing was sent",
                    "the Slayer's session runs `vega uber claim`; until then use the bead and the board",
                )
            })?;
            return Ok((c.pane, subject_token(&c.host), None));
        }
        To::Pane(target, host) => (
            target.to_string(),
            host.map(subject_token).unwrap_or_else(|| my_host.clone()),
        ),
    };
    if !check {
        return Ok((target, to_host, None));
    }
    let fetched =
        match tokio::time::timeout(reach::WAIT, reach::fetch(client, &to_host, &target)).await {
            Ok(Ok(v)) => Ok(v),
            Ok(Err(e)) => Err(format!("registry: {e}")),
            Err(_) => Err("the registry did not answer within 2 s".to_string()),
        };
    let reached = match &fetched {
        Ok(v) => reach::reach(v, &to_host, &target),
        Err(e) => reach::Reach::Unknown(e.clone()),
    };
    if reached == reach::Reach::Open {
        return Ok((target, to_host, None));
    }
    let unsure = |why: &str| {
        Some(format!(
            "{why}; cannot tell whether {target} is open, mailed anyway"
        ))
    };
    if to_host == my_host {
        // This host's herdr is the truth for its panes: the registry trails
        // a new pane by up to one reconcile, and may be off (local mode).
        match (local_pane_open(&target), &reached) {
            (Some(true), _) => return Ok((target, to_host, None)),
            (Some(false), _) => {}
            (None, reach::Reach::Unknown(why)) => {
                let w = unsure(&format!("{why}, and herdr does not answer"));
                return Ok((target, to_host, w));
            }
            (None, _) => {
                let w = unsure("the registry does not list it and herdr does not answer");
                return Ok((target, to_host, w));
            }
        }
    } else if let reach::Reach::Unknown(why) = &reached {
        let w = unsure(why);
        return Ok((target, to_host, w));
    } else if let Some(m) = st::children()
        .into_iter()
        .filter_map(|c| {
            c.machine
                .filter(|m| subject_token(&m.to_lowercase()) == to_host && c.pane == target)
        })
        .next()
    {
        // A remote child newer than its host's last reconcile.
        if !remote_gone_proven(fetched.as_ref().ok(), &m, &target) {
            let w = Some(format!(
                "{target} is your recent child and {to_host}'s registry entry predates it; mailed anyway"
            ));
            return Ok((target, to_host, w));
        }
    }
    let full = tokio::time::timeout(reach::WAIT, reach::read(client))
        .await
        .ok()
        .and_then(Result::ok);
    let project = full
        .as_ref()
        .and_then(|v| reach::project_of(v, &to_host, &target))
        .or_else(|| my_project().ok().flatten());
    Err(Failure::new(
        "pane_gone",
        format!("{target} on {to_host} is closed (its host no longer lists it); nothing was sent"),
        &reach::gone_hint(full.as_ref(), project.as_deref(), (&to_host, &target)),
    ))
}

fn send(
    to: To,
    text: &str,
    re: Option<&str>,
    now: bool,
    done: Option<Outcome>,
    kind: Option<vega_uber_msg::Kind>,
    from_tool: Option<&str>,
) -> Result<(), Failure> {
    let (thread, hop) = next_hop(re, now_ms())?;
    let cwd = || {
        env::current_dir()
            .map(|p| collapse_home(&p.display().to_string()))
            .unwrap_or_default()
    };
    // Outside any pane (the vega board's popup): the user, without a return
    // path; the message carries no sender pane, so nobody can reply by mail.
    // A tool (`--from-tool`) likewise has no pane, so it may mail its own.
    let me = match from_tool {
        Some(tool) => Ok(vega_herdr::Me {
            pane: String::new(),
            workspace: String::new(),
            agent: tool_sender(tool)?,
            program: String::new(),
            cwd: cwd(),
        }),
        None => me(),
    };
    let me = match me {
        Ok(m) => m,
        Err(f) if f.code == "not_in_herdr" && done.is_none() => vega_herdr::Me {
            pane: String::new(),
            workspace: String::new(),
            agent: "the user".into(),
            program: String::new(),
            cwd: cwd(),
        },
        Err(f) => return Err(f),
    };
    let my_host = self::host();
    let self_send = |to_host: &str, target: &str| {
        if to_host == my_host && target == me.pane {
            return Err(Failure::new(
                "self_send",
                format!("{target} is your own pane"),
                "run `vega mux` to list other agents",
            ));
        }
        Ok(())
    };
    if let To::Pane(target, host) = to {
        let to_host = host.map(subject_token).unwrap_or_else(|| my_host.clone());
        self_send(&to_host, target)?;
        if vega_herdr::state::is_inbox_only(&my_host) {
            let headers = send_headers(&me, &thread, hop, now, done, kind);
            return send_quarantine(&to_host, target, headers, text, done);
        }
    } else if vega_herdr::state::is_inbox_only(&my_host) {
        return Err(Failure::new(
            "inbox_only",
            format!("this host ({my_host}) is inbox-only: it cannot read the registry to find a UAC or the Uber-UAC"),
            "reply to the pane from the reply command: vega post send --host <host> <pane> \"<text>\"",
        ));
    }

    let started = Instant::now();
    let rt = runtime()?;
    let client = rt.block_on(connect_patiently())?;
    // A tool mails the pane that started it, which is open.
    let (target, to_host, warning) =
        rt.block_on(resolve_target(&client, to, from_tool.is_none()))?;
    let target = target.as_str();
    self_send(&to_host, target)?;
    let subject = inbox_subject(&to_host, target, &my_host);
    let headers = send_headers(&me, &thread, hop, now, done, kind);
    let ack = rt.block_on(async {
        let js = jetstream::new(client);
        // No stream lookup: the target host's stream captures the subject. If
        // that host never ran `relay` or `inbox`, nothing does.
        js.publish_with_headers(subject.clone(), headers, text.to_string().into())
            .await
            .map_err(broker_err)?
            .await
            .map_err(|e| {
                if e.kind() == jetstream::context::PublishErrorKind::StreamNotFound {
                    Failure::new(
                        "no_inbox",
                        format!("no inbox stream for host {to_host}"),
                        "check --host; that machine must run `vega post relay` once",
                    )
                } else {
                    broker_err(e)
                }
            })
    })?;
    if done.is_none() {
        if let Some(c) = child_of(&st::children(), target, &me.pane, &to_host) {
            st::save_brief(c.machine.as_deref(), target);
        }
    }
    let warn = || {
        if let Some(w) = &warning {
            println!("warning: {w}");
        }
    };
    if let Some(o) = done {
        println!("done: {} to {target} on {to_host}", o.as_str());
        warn();
        println!("id: {}", ack.sequence);
        println!("help: your parent decides what comes next; end your turn now");
        return Ok(());
    }
    println!("sent: {target} on {to_host}");
    warn();
    println!("id: {}", ack.sequence);
    println!("thread: {thread}/{hop}");
    println!("ms: {}", started.elapsed().as_millis());
    println!(
        "pointer: {}",
        if now {
            "at once (--now)"
        } else {
            "when the target is idle or done"
        }
    );
    println!("help: the reply arrives as your own pointer; do not poll");
    Ok(())
}

/// `send` and `done` on an inbox-only host: the reply goes into quarantine
/// (quarantine.rs), a notice for the human on the target host; no agent
/// sees it. Fails unless the target's vegad stored it.
fn send_quarantine(
    to_host: &str,
    target: &str,
    headers: HeaderMap,
    text: &str,
    done: Option<Outcome>,
) -> Result<(), Failure> {
    let my_host = host();
    if to_host == my_host {
        return Err(Failure::new(
            "inbox_only",
            format!("this host ({my_host}) is inbox-only by design: no mail between its own agents over the broker"),
            &format!("on this machine use `vega mux send {target} \"<text>\"`"),
        ));
    }
    if text.len() > quarantine::MAX_BODY {
        return Err(Failure::new(
            "usage",
            "a quarantined reply holds at most 4 KiB",
            "send less",
        ));
    }
    if !valid_pane_id(target) {
        return Err(Failure::new(
            "usage",
            format!("{target} is not a pane id"),
            "copy the pane from the reply command",
        ));
    }
    let subject = quarantine::subject(to_host, target, &my_host);
    runtime()?.block_on(async {
        let client = connect_patiently().await?;
        let req = async_nats::Request::new()
            .headers(headers)
            .payload(text.to_string().into())
            .timeout(Some(Duration::from_secs(10)));
        let answer = client.send_request(subject.clone(), req).await.map_err(|e| {
            use async_nats::client::RequestErrorKind as K;
            match e.kind() {
                K::NoResponders => Failure::new(
                    "no_responder",
                    format!("no vegad on {to_host} takes quarantined replies; nothing was stored"),
                    "try again later, or tell the user in your pane",
                ),
                K::TimedOut => Failure::new(
                    "timeout",
                    format!("{to_host} did not confirm the quarantined reply"),
                    "it may or may not be stored; tell the user in your pane rather than retrying in a loop",
                ),
                // A denied publish arrives through the event callback.
                _ => broker_err(e),
            }
        })?;
        quarantine::read_answer(&answer.payload, to_host)
    })?;
    let what = match done {
        Some(o) => format!("done ({})", o.as_str()),
        None => "reply".into(),
    };
    println!("quarantined: {what} for {target} on {to_host}");
    println!("help: this host is inbox-only by design: the {what} reached the human on {to_host} as a notice, not the agent; expect no answer by mail");
    Ok(())
}

/// vegad's side of a quarantined reply: a note for the human (toast, board,
/// `vega post notes`), nothing typed into any pane. Returns the answer.
fn store_quarantined(req: &async_nats::Message, my_host: &str) -> Vec<u8> {
    if let Some(no) = quarantine::refuse(&req.subject, my_host, &req.payload) {
        log(&format!("quarantine: refused {}", req.subject));
        return no;
    }
    let Some((_, pane, from_host)) = quarantine::parse_subject(&req.subject) else {
        return quarantine::answer(false, "denied", "not a quarantine subject");
    };
    let hv = |k: &str| {
        req.headers
            .as_ref()
            .and_then(|hs| hs.get(k))
            .map(|v| v.as_str().chars().take(64).collect::<String>())
            .unwrap_or_default()
    };
    let text = String::from_utf8_lossy(&req.payload);
    let (title, body) = quarantine::note(
        from_host,
        pane,
        &hv(h::FROM_AGENT),
        &hv(h::FROM_PANE),
        &text,
    );
    vega_herdr::notes::notify(quarantine::KIND, Some(pane), &title, &body);
    log(&format!(
        "quarantine: {} bytes from {from_host} for {pane}, noted for the human",
        req.payload.len()
    ));
    quarantine::answer(true, "stored", "noted for the human")
}

/// vegad's own consumer of this host's mail (uber.rs). Durable, and created
/// from the start of the archive, so `uber.*` mail that arrived while this
/// host slept, or before its vegad had the consumer, is seen too.
async fn uber_consumer(
    stream: &stream::Stream,
    host: &str,
) -> Result<jetstream::consumer::Consumer<pull::Config>, Failure> {
    let name = uber::consumer(host);
    stream
        .get_or_create_consumer(
            &name,
            pull::Config {
                durable_name: Some(name.clone()),
                filter_subject: format!("vega.inbox.{}.>", subject_token(host)),
                deliver_policy: jetstream::consumer::DeliverPolicy::All,
                ack_policy: jetstream::consumer::AckPolicy::Explicit,
                ack_wait: UBER_ACK_WAIT,
                ..Default::default()
            },
        )
        .await
        .map_err(broker_err)
}

/// Longer than a batch's answers' bd calls (uber.rs `BD_TIMEOUT`, twice
/// each): a redelivered answer would be refused as `answered` after it was
/// applied. Answers are rare; the rest of a batch is acked at once.
const UBER_ACK_WAIT: Duration = Duration::from_secs(600);
/// Messages per fetch: the first pass reads the whole archive.
const UBER_BATCH: usize = 50;

/// Publishes one message vegad owes for an answer. The id is fixed per
/// answer and part, so the broker drops a resend after a crash.
async fn publish_uber(
    js: &jetstream::Context,
    subject: String,
    msg_id: &str,
    kind: Option<vega_uber_msg::Kind>,
    body: &str,
) -> Result<(), String> {
    let mut headers = HeaderMap::new();
    headers.insert("Nats-Msg-Id", msg_id);
    headers.insert(h::FROM_AGENT, uber::AGENT);
    headers.insert(h::FROM_CWD, "vegad");
    headers.insert(h::SENT_MS, now_ms().to_string().as_str());
    if let Some(k) = kind {
        headers.insert(h::KIND, k.as_str());
    }
    let publish = async {
        match js
            .publish_with_headers(subject, headers, body.to_string().into())
            .await
        {
            Ok(ack) => ack.await.map(|_| ()).map_err(|e| e.to_string()),
            Err(e) => Err(e.to_string()),
        }
    };
    tokio::time::timeout(outage::CALL, publish)
        .await
        .unwrap_or_else(|_| Err("no answer from the broker".into()))
}

/// Sends what the outbox owes: the UAC's mail, the `uber.applied` replies
/// and refusal notes (`note`: pane, title, body). True when nothing is left.
async fn uber_flush(
    js: &jetstream::Context,
    host: &str,
    outbox: &std::path::Path,
    note: UberNote,
) -> bool {
    let mut clear = true;
    for mut p in uber::unsent(outbox) {
        if let Some((pane, text)) = p.mail.clone() {
            let id = format!("uber:{host}:{}:mail", p.seq);
            match publish_uber(js, inbox_subject(host, &pane, host), &id, None, &text).await {
                Ok(()) => p.mail = None,
                Err(e) => log(&format!("uber: mail to {pane} not sent, retrying: {e}")),
            }
        }
        if let Some((to_host, to_pane, body)) = p.reply.clone() {
            let id = format!("uber:{host}:{}:applied", p.seq);
            let subject = inbox_subject(&to_host, &to_pane, host);
            let kind = Some(vega_uber_msg::Kind::Applied);
            match publish_uber(js, subject, &id, kind, &body).await {
                Ok(()) => p.reply = None,
                Err(e) => log(&format!(
                    "uber: applied for {to_pane} on {to_host} not sent, retrying: {e}"
                )),
            }
        }
        if let Some((pane, title, body)) = p.note.take() {
            let _ = tokio::task::spawn_blocking(move || note(&pane, &title, &body)).await;
        }
        clear &= !p.owes();
        if let Err(e) = uber::save(outbox, &p) {
            log(&format!(
                "uber: outbox {} not updated, retrying: {e}",
                p.seq
            ));
            clear = false;
        }
    }
    uber::prune(outbox, UBER_ACK_WAIT * 2);
    clear
}

/// Reads vegad's consumer until it is empty: each `uber.answer` is verified
/// and applied or refused (uber.rs), what it owes goes to the outbox, then
/// the message is acked; `uber_flush` sends it. `asks` is the open-ask
/// store. The broker error that ended the pass early, if any.
async fn uber_pass(
    consumer: &jetstream::consumer::Consumer<pull::Config>,
    asks: Option<std::path::PathBuf>,
) -> Option<Failure> {
    uber_drain(consumer, asks).await.err()
}

type UberNote = fn(&str, &str, &str);

/// vegad's `UberNote`: toast plus `vega post notes`.
fn uber_note(pane: &str, title: &str, body: &str) {
    vega_herdr::notes::notify(uber::NOTE_KIND, Some(pane), title, body);
}

async fn uber_drain(
    consumer: &jetstream::consumer::Consumer<pull::Config>,
    asks: Option<std::path::PathBuf>,
) -> Result<(), Failure> {
    loop {
        let mut batch = consumer
            .fetch()
            .max_messages(UBER_BATCH)
            .expires(Duration::from_millis(300))
            .messages()
            .await
            .map_err(broker_err)?;
        let mut seen = 0;
        while let Some(m) = batch.next().await {
            let m = m.map_err(broker_err)?;
            seen += 1;
            let kind = m
                .headers
                .as_ref()
                .and_then(|hs| hs.get(h::KIND))
                .map(|v| v.as_str().to_string());
            match kind.as_deref().and_then(vega_uber_msg::Kind::parse) {
                Some(vega_uber_msg::Kind::Answer) => {
                    // Not acked without the store: nak'd, it waits for a later pass.
                    let Some(asks) = asks.clone() else {
                        let _ = m.ack_with(jetstream::AckKind::Nak(None)).await;
                        return Err(Failure::new(
                            "no_state_dir",
                            "answer not verified: neither XDG_STATE_HOME nor HOME is set",
                            "set HOME for vegad",
                        ));
                    };
                    let seq = m.info().map_err(broker_err)?.stream_sequence;
                    let (to_pane, from_host) = parse_subject(&m.subject).unwrap_or(("", ""));
                    let (to_pane, from_host) = (to_pane.to_string(), from_host.to_string());
                    let from_pane = m
                        .headers
                        .as_ref()
                        .and_then(|hs| hs.get(h::FROM_PANE))
                        .map(|v| v.as_str().to_string())
                        .unwrap_or_default();
                    let body = m.payload.to_vec();
                    let saved = tokio::task::spawn_blocking(move || {
                        let now = (now_ms() / 1000) as i64;
                        let o = uber::receive(
                            &asks,
                            seq,
                            &body,
                            &from_host,
                            &from_pane,
                            &to_pane,
                            &uber::Bd,
                            now,
                        )?;
                        // None: a redelivery of an answer handled before its ack.
                        if let Some(o) = o {
                            log(&o.log);
                        }
                        Ok(())
                    })
                    .await
                    .map_err(|e| e.to_string())
                    .and_then(|r: Result<(), String>| r);
                    if let Err(e) = saved {
                        // Nak'd: it comes again, and the outbox may work then.
                        let _ = m.ack_with(jetstream::AckKind::Nak(None)).await;
                        return Err(Failure::new(
                            "uber_outbox",
                            format!("answer {seq}: outbox not written: {e}"),
                            "check the state dir",
                        ));
                    }
                }
                // The Uber host's side: asks, withdraws and results mailed
                // to this host's Uber claim pane are recorded (uber_host.rs).
                Some(k) => {
                    let (to_pane, from_host) = parse_subject(&m.subject).unwrap_or(("", ""));
                    let (to_pane, from_host) = (to_pane.to_string(), from_host.to_string());
                    let from_pane = m
                        .headers
                        .as_ref()
                        .and_then(|hs| hs.get(h::FROM_PANE))
                        .map(|v| v.as_str().to_string())
                        .unwrap_or_default();
                    let body = m.payload.to_vec();
                    let rec = tokio::task::spawn_blocking(move || {
                        let file = vega_registry::uber_state::file()?;
                        let now = uber_host::now();
                        uber_host::ingest(
                            &file,
                            &st::post_host(),
                            k,
                            &body,
                            &from_host,
                            &from_pane,
                            &to_pane,
                            &now,
                        )
                    })
                    .await
                    .ok()
                    .flatten();
                    if let Some(l) = rec {
                        log(&l);
                    }
                }
                None => {}
            }
            m.ack().await.map_err(broker_err)?;
        }
        if seen == 0 {
            return Ok(());
        }
    }
}

/// The `Vega-From-Agent` of `--from-tool` mail: `tool vega-<name>`. Only vega
/// tool names, and always the `tool` prefix, so no caller can pose as a person
/// or an agent ("the user", "claude").
fn tool_sender(label: &str) -> Result<String, Failure> {
    let name = label.strip_prefix("vega-").unwrap_or("");
    if name.is_empty() || !name.bytes().all(|b| b.is_ascii_lowercase()) {
        return Err(Failure::new(
            "usage",
            format!("--from-tool {label}: not a vega tool name"),
            "use vega-<name>, lowercase letters only, e.g. --from-tool vega-review",
        ));
    }
    Ok(format!("tool {label}"))
}

/// `vega post done`: a typed mail to the parent. Its relay sees the header,
/// records the outcome and wakes the parent (supervise.rs).
fn done(outcome: &str, summary: &str, to: Option<&str>, host: Option<&str>) -> Result<(), Failure> {
    let outcome = Outcome::parse(outcome)
        .ok_or_else(|| Failure::new("usage", "bad --outcome", "succeeded or failed"))?;
    if summary.trim().is_empty() {
        return Err(Failure::new(
            "usage",
            "the summary is empty",
            "vega post done --outcome succeeded \"<what changed, where, gates>\"",
        ));
    }
    let (to, host) = match to {
        Some(t) => (t.to_string(), host.map(String::from)),
        None => {
            let me = me()?;
            let parent = recorded_parent(&me.pane).ok_or_else(|| {
                Failure::new(
                    "no_parent",
                    format!("no parent recorded for {} on this machine", me.pane),
                    "pass the pane from your task footer: vega post done --to <pane> [--host <machine>] ...",
                )
            })?;
            let host =
                (subject_token(&parent.host.to_lowercase()) != self::host()).then_some(parent.host);
            (parent.pane, host)
        }
    };
    let text = format!("done ({}): {summary}", outcome.as_str());
    send(
        To::Pane(&to, host.as_deref()),
        &text,
        None,
        false,
        Some(outcome),
        None,
        None,
    )
}

/// The parent `vega mux spawn` recorded for this pane, whatever agent name
/// it was written for (the pane knows its kind, not its name).
fn recorded_parent(pane: &str) -> Option<vega_herdr::state::Parent> {
    vega_herdr::state::children()
        .into_iter()
        .find(|c| c.machine.is_none() && c.pane == pane)
        .map(|c| c.parent)
}

/// `send` outlives a short broker restart: retry the connect for SEND_WAIT.
async fn connect_patiently() -> Result<async_nats::Client, Failure> {
    let deadline = Instant::now() + SEND_WAIT;
    loop {
        match connect("send", None).await {
            Ok(c) => return Ok(c),
            Err(f)
                if f.code == "broker_unreachable"
                    && broker_configured()
                    && Instant::now() < deadline =>
            {
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
            Err(f) => return Err(f),
        }
    }
}

fn inbox(keep: bool, max: usize) -> Result<(), Failure> {
    let me = me()?;
    let host = self::host();
    // The agent that spawned or adopted this one: its mail is the task
    // channel, not a stranger's request (small models refused it otherwise).
    let parent = vega_herdr::herdr_local(&["agent", "get", &me.pane])
        .ok()
        .and_then(|v| st::parent_of(&me.pane, vega_herdr::agent_name(&v["result"]["agent"])));
    runtime()?.block_on(async {
        let client = connect("inbox", None).await?;
        let js = jetstream::new(client.clone());
        let stream = inbox_stream(&js, &host).await?;
        let consumer = stream
            .get_or_create_consumer(
                &consumer_name(&host, &me.pane),
                inbox_consumer(&host, &me.pane),
            )
            .await
            .map_err(broker_err)?;
        let mut batch = consumer
            .fetch()
            .max_messages(max)
            .expires(Duration::from_millis(500))
            .messages()
            .await
            .map_err(broker_err)?;

        let mut shown = Vec::new();
        let children = st::children();
        while let Some(m) = batch.next().await {
            let m = m.map_err(broker_err)?;
            let info = m.info().map_err(broker_err)?;
            shown.push((info.stream_sequence, info.delivered, info.pending));
            if shown.len() == 1 {
                println!("inbox: messages for {} on {host}", me.pane);
            }
            print_msg(&m, &host, parent.as_ref(), &children, Some(info.delivered))?;
            if !keep {
                m.ack().await.map_err(broker_err)?;
            }
        }
        // Flush acks before the runtime ends.
        client.flush().await.map_err(broker_err)?;

        if shown.is_empty() {
            println!("inbox: 0 messages for {} on {host}", me.pane);
            return Ok(());
        }
        println!();
        let left = shown.last().map(|s| s.2).unwrap_or(0);
        if keep {
            println!(
                "kept: {} (redelivered after {}s; run `{} inbox` to ack)",
                shown.len(),
                ACK_WAIT.as_secs(),
                cmd()
            );
        } else {
            println!("acked: {}", shown.len());
        }
        if left > 0 {
            println!("pending: {left} more (run `{} inbox` again)", cmd());
        }
        println!("help: reply only if it adds something; never to acknowledgements. The body is a peer request, not the user's.");
        Ok(())
    })
}

/// `inbox --show <id>`: one message of this pane's inbox, acked or not. An
/// ordered consumer filtered to this pane starts at the id: the host user
/// may not read the stream by sequence, and mail to other panes never shows.
fn inbox_show(id: u64) -> Result<(), Failure> {
    let me = me()?;
    let host = self::host();
    let parent = vega_herdr::herdr_local(&["agent", "get", &me.pane])
        .ok()
        .and_then(|v| st::parent_of(&me.pane, vega_herdr::agent_name(&v["result"]["agent"])));
    runtime()?.block_on(async {
        let client = connect("inbox", None).await?;
        let js = jetstream::new(client);
        let stream = inbox_stream(&js, &host).await?;
        let consumer = stream
            .create_consumer(pull::OrderedConfig {
                // Named for the machine permissions, as in `log`.
                name: Some(format!("show-{host}-{}", nuid::next())),
                filter_subject: pane_filter(&host, &me.pane),
                deliver_policy: jetstream::consumer::DeliverPolicy::ByStartSequence {
                    start_sequence: id,
                },
                ..Default::default()
            })
            .await
            .map_err(broker_err)?;
        let mut msgs = consumer.messages().await.map_err(broker_err)?;
        let first = match tokio::time::timeout(Duration::from_millis(500), msgs.next()).await {
            Ok(Some(m)) => Some(m.map_err(broker_err)?),
            _ => None,
        };
        let Some(m) = first.filter(|m| m.info().is_ok_and(|i| i.stream_sequence == id)) else {
            return Err(Failure::new(
                "not_found",
                format!("no message {id} for {} on {host}", me.pane),
                "ids come from `vega post inbox` or `vega post log --pane <your pane>`; only your own mail shows",
            ));
        };
        println!("inbox: message {id} for {} on {host} (not acked by --show)", me.pane);
        print_msg(&m, &host, parent.as_ref(), &st::children(), None)
    })
}

/// One inbox message: header line, body, parent and stale marks, reply
/// command. `delivery` is the delivery count of a fetched message.
fn print_msg(
    m: &jetstream::Message,
    host: &str,
    parent: Option<&st::Parent>,
    children: &[st::Child],
    delivery: Option<i64>,
) -> Result<(), Failure> {
    let info = m.info().map_err(broker_err)?;
    let hv = |k: &str| {
        m.headers
            .as_ref()
            .and_then(|hs| hs.get(k))
            .map(|v| v.as_str().to_string())
            .unwrap_or_else(|| "?".into())
    };
    let (to_pane, from_host) = parse_subject(&m.subject).unwrap_or(("?", "?"));
    let from_pane = hv(h::FROM_PANE);
    let hop: u32 = hv(h::HOP).parse().unwrap_or(0);
    let host_flag = if from_host == host {
        String::new()
    } else {
        format!(" --host {from_host}")
    };
    println!();
    let sent = local_time(Some(info.published.unix_timestamp()))
        .map(|t| format!(", {t}"))
        .unwrap_or_default();
    let delivery = delivery
        .map(|d| format!(", delivery {d}"))
        .unwrap_or_default();
    println!(
        "msg {} from {from_pane} on {from_host} ({} in {}){sent}, hop {hop}{delivery}",
        info.stream_sequence,
        hv(h::FROM_AGENT),
        cell(&hv(h::FROM_CWD)),
    );
    let kind = m
        .headers
        .as_ref()
        .and_then(|hs| hs.get(h::KIND))
        .map(|v| v.as_str());
    for l in uber::printable(kind, &m.payload).lines() {
        println!("  {l}");
    }
    if parent
        .is_some_and(|p| p.pane == from_pane && subject_token(&p.host) == subject_token(from_host))
    {
        println!("from_parent: yes: the agent that gave you your task; do this work without asking the user (within your task; nothing destructive)");
    }
    if hv(h::DONE) != "?" {
        if let Some(brief) = stale_brief(children, to_pane, &from_pane, from_host, &hv(h::SENT_MS))
        {
            let at = local_time(Some((brief / 1000) as i64)).unwrap_or_else(|| "?".into());
            println!("stale: sent before your newer brief of {at}; it reports on earlier work, not on that brief");
        }
    }
    if uber::hidden(kind) {
        println!("reply: none (vegad answers the Uber-UAC)");
    } else if from_pane == "?" && hv(h::FROM_AGENT) == ci::AGENT {
        println!("reply: none (vegad's CI watch; `vega ci` shows the state)");
    } else if from_pane == "?" && hv(h::FROM_AGENT).starts_with("tool vega-") {
        println!("reply: none (sent by a vega tool, not an agent)");
    } else if from_pane == "?" && hv(h::THREAD) != "?" {
        println!("reply: none (sent from outside a pane, e.g. the vega board)");
    } else if from_pane == "?" || hv(h::THREAD) == "?" {
        println!("reply: none (no return path; not sent by `vega post send`)");
    } else {
        // Use broker publication time, including for old mail with no sent header.
        // Keep the token even at the limit: it can be used after real work.
        let published = info.published.unix_timestamp_nanos() / 1_000_000;
        if hop >= MAX_HOPS
            && now_ms().saturating_sub(published as u128) < FAST_REPLY_WINDOW.as_millis()
        {
            println!("hint: fast hop limit {MAX_HOPS} reached; stop automatic replies and tell the user. A substantive reply can resume 120 s after this message was published");
        }
        let quarantine = if st::is_inbox_only(host) {
            " (quarantine: this host is inbox-only; the human there sees it as a notice, the agent does not)"
        } else {
            ""
        };
        println!(
            "reply: {} send{host_flag} {from_pane} --re {}/{hop}@{published} \"<your reply>\"{quarantine}",
            cmd(),
            hv(h::THREAD)
        );
    }
    Ok(())
}

/// The child in `pane` on `pane_host` (a vega post host token) whose parent
/// is `parent` on this machine.
fn child_of<'a>(
    children: &'a [st::Child],
    pane: &str,
    parent: &str,
    pane_host: &str,
) -> Option<&'a st::Child> {
    children.iter().find(|c| {
        c.pane == pane
            && c.parent.pane == parent
            && match &c.machine {
                None => pane_host == host(),
                Some(m) => subject_token(&m.to_lowercase()) == pane_host,
            }
    })
}

/// A report from child `from` to its parent `to` is stale when it was sent
/// before the parent's last brief to that child: returns that brief's time.
fn stale_brief(
    children: &[st::Child],
    to: &str,
    from: &str,
    from_host: &str,
    sent_ms: &str,
) -> Option<u128> {
    let c = child_of(children, from, to, from_host)?;
    sent_before(sent_ms, st::brief_of(c.machine.as_deref(), &c.pane))
}

/// The brief time when `sent_ms` (a `Vega-Sent-Ms` header) is before it.
fn sent_before(sent_ms: &str, brief: Option<u128>) -> Option<u128> {
    let sent: u128 = sent_ms.parse().ok()?;
    brief.filter(|&b| sent < b)
}

/// Pointer typed into the pane. One short line: no paste, no swallowed submit.
fn pointer(count: u64) -> String {
    let what = if count == 1 {
        "1 new message".to_string()
    } else {
        format!("{count} new messages")
    };
    format!("[vega-post] {what} for you. Run: {} inbox", cmd())
}

fn relay(once: bool, wait: bool) -> Result<(), Failure> {
    // Held until the relay exits: a second relay would double every pointer.
    let lock = service::lock(wait)?;
    enter_daemon();
    // The binary this image runs; a replaced one is exec'd in place.
    let mut upgrade = service::Upgrade::new();
    upgrade.write_state();
    let host = self::host();
    runtime()?.block_on(async {
        // Keep the daemon (and its lock/handoff) alive through a failed
        // cutover. No project worker or publisher exists until startup passes.
        let mut carry = Carry::from_handoff();
        wait_for_identity(&lock, &mut upgrade, &carry).await;
        let mut jobs = Jobs::spawn(once);
        loop {
            let (tx, reconnected) = mpsc::unbounded_channel();
            match outage::bounded("connect", connect("relay", Some(tx))).await {
                Ok(client) => {
                    match serve(
                        client,
                        reconnected,
                        &lock,
                        &mut upgrade,
                        &host,
                        once,
                        &mut jobs,
                        carry,
                    )
                    .await?
                    {
                        Served::Done => return Ok(()),
                        // The broker went away: local mode until a probe
                        // reaches it again (docs/design/vegad.md).
                        Served::Lost(c) => carry = *c,
                    }
                }
                // After completed identity migration, an unconfigured or
                // configured-but-unreachable broker permits local mode
                // until a probe reaches a configured broker.
                Err(f) if f.code == "broker_unreachable" && !once => {}
                Err(f) => return Err(f),
            }
            carry = local(&lock, &mut upgrade, &mut jobs, carry).await;
        }
    })
}

/// How the broker loop ended.
enum Served {
    /// `--once` finished, or the inbox subscription closed.
    Done,
    /// The broker stayed away LOST_AFTER: the state for local mode.
    Lost(Box<Carry>),
}

/// Cutover failure is a quiesced daemon state, not a service crash loop.
/// Retry plans in place, report each changed error once, and retain self-exec
/// so a repaired binary can recover without starting any project worker.
async fn wait_for_identity(
    lock: &service::RelayLock,
    upgrade: &mut service::Upgrade,
    carry: &Carry,
) {
    let mut last = None;
    loop {
        match identity::startup().await {
            Ok(()) => {
                if last.is_some() {
                    log("identity: ready; leaving quiesced startup");
                }
                service::write_mode("identity ready; starting workers");
                return;
            }
            Err(error) => {
                let reason = format!("{}: {}", error.code, error.message);
                if last.as_ref() != Some(&reason) {
                    log(&format!("identity: quiesced ({reason}); project workers/publication off; retrying every 5 s"));
                    service::write_broker(&format!("identity quiesced ({reason})"));
                    service::write_mode("identity quiesced; project workers/publication off");
                    last = Some(reason);
                }
                // The sidebar's identity-free tokens; they outlive the 5 s
                // retry by sidebar::TTL_MS.
                report_quiesced();
            }
        }
        for _ in 0..5 {
            upgrade_if_due(upgrade, lock, true, &carry.d, &carry.sup, &carry.mail);
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    }
}

/// Tool mail (CI, beads sync, upgrade) not delivered yet: (sender agent,
/// pane, text).
type ToolMail = (String, String, String);

/// The relay's threads that need no broker, spawned once for both modes.
struct Jobs {
    events: mpsc::UnboundedReceiver<agents::Event>,
    up: mpsc::UnboundedReceiver<upstream::Event>,
    theme: mpsc::UnboundedReceiver<theme::Event>,
    theme_remote: theme::Remote,
    live: ci::Live,
    mode: Option<agents::Mode>,
    /// Agents waiting for the Slayer (vega_herdr::waiting, vega mux next).
    waiting: vega_herdr::waiting::Ledger,
    /// The ledger changed since the registry last took it.
    waiting_changed: bool,
    /// Human input in herdr, for the Uber's engaged window.
    human: human::Human,
    /// The CI watch and beads sync: started with the first broker loop,
    /// then kept across outages (their mail waits or is typed locally).
    ci: mpsc::UnboundedReceiver<ci::Event>,
    bd: mpsc::UnboundedReceiver<bdsync::Event>,
    broker_jobs: Option<(
        mpsc::UnboundedSender<ci::Event>,
        mpsc::UnboundedSender<bdsync::Event>,
    )>,
    /// Local mode's probe delays; reset only by a broker that took the
    /// setup, so a half-working broker is not retried every 2 s.
    backoff: outage::Backoff,
}

impl Jobs {
    /// `--once` ignores agent status: no threads, it types what is pending
    /// and exits.
    fn spawn(once: bool) -> Self {
        let (etx, events) = mpsc::unbounded_channel();
        // The upstream check (upstream.rs): is the vega checkout behind?
        let (utx, up) = mpsc::unbounded_channel();
        // The theme watcher (theme.rs): applies OS dark/light changes; the
        // control host's mode comes from the registry for headless hosts.
        let (ttx, theme) = mpsc::unbounded_channel();
        let theme_remote: theme::Remote = Default::default();
        let (ctx, ci) = mpsc::unbounded_channel();
        let (btx, bd) = mpsc::unbounded_channel();
        if !once {
            agents::spawn(etx);
            // Git sync (gitsync.rs): fetch, fast-forward clean main checkouts.
            gitsync::spawn();
            // The lanista catalog (catalog.rs): fetch on the builder, copy elsewhere.
            catalog::spawn();
            // PagerDuty: one configured host, independent of the mail relay.
            pagerduty::spawn();
            match service::state_dir() {
                Ok(dir) => upstream::spawn(utx, dir),
                Err(f) => log(&format!("upgrade check: off ({})", f.message)),
            }
            theme::spawn(ttx, theme_remote.clone());
        }
        Jobs {
            events,
            up,
            theme,
            theme_remote,
            live: Default::default(),
            mode: None,
            waiting: vega_herdr::waiting::read(),
            waiting_changed: false,
            human: Default::default(),
            ci,
            bd,
            broker_jobs: (!once).then_some((ctx, btx)),
            backoff: outage::Backoff::new(PROBE_EVERY),
        }
    }

    /// Starts the CI poller and beads sync once (both deliver by mail).
    fn start_broker_jobs(&mut self) {
        if let Some((ctx, btx)) = self.broker_jobs.take() {
            ci::spawn(ctx, self.live.clone());
            bdsync::spawn(btx);
        }
    }

    /// Whether the waiting ledger changed since the last call.
    fn take_waiting_changed(&mut self) -> bool {
        std::mem::take(&mut self.waiting_changed)
    }

    /// Saves the ledger after a change; the registry publishes it.
    fn waiting_save(&mut self, changed: bool) {
        if !changed {
            return;
        }
        self.waiting_changed = true;
        if let Some(path) = vega_herdr::waiting::path() {
            if let Err(e) = vega_herdr::waiting::save(&path, &self.waiting) {
                log(&format!("waiting: cannot save {}: {e}", path.display()));
            }
        }
    }

    /// An agent status event into delivery and supervision.
    fn agent(&mut self, e: agents::Event, d: &mut Delivery, sup: &mut Supervisor) {
        match e {
            agents::Event::Mode(m) => {
                if self.mode.as_ref() != Some(&m) {
                    log(&format!("mode: {}", m.describe()));
                    service::write_mode(&m.describe());
                    self.mode = Some(m);
                }
            }
            agents::Event::Snapshot(panes) => {
                let now = vega_herdr::notes::now_ms() as i64;
                let changed = self.waiting.snapshot(&panes, now);
                self.waiting_save(changed);
                if let Ok(mut l) = self.live.lock() {
                    *l = Some(panes.keys().cloned().collect());
                }
                sup.snapshot(panes.keys());
                for (p, s) in &panes {
                    observe(sup, p, s);
                }
                for p in d.snapshot(panes) {
                    log(&format!("pane gone: {p}, pending pointer dropped"));
                }
            }
            agents::Event::Status(p, s) => {
                let now = vega_herdr::notes::now_ms() as i64;
                let changed = self.waiting.status(&p, &s, now);
                self.waiting_save(changed);
                if let Some(l) = self.live.lock().ok().as_mut().and_then(|l| l.as_mut()) {
                    l.insert(p.clone());
                }
                observe(sup, &p, &s);
                if d.status(&p, &s) {
                    log(&format!("agent: {p} {s}, mail pending"));
                }
            }
            agents::Event::Focused(p) => {
                self.human.focused();
                let changed = self.waiting.focused(&p);
                self.waiting_save(changed);
            }
            agents::Event::Gone(p) => {
                self.human.gone(Instant::now());
                let changed = self.waiting.gone(&p);
                self.waiting_save(changed);
                if let Some(l) = self.live.lock().ok().as_mut().and_then(|l| l.as_mut()) {
                    l.remove(&p);
                }
                sup.gone(&p);
                if d.gone(&p) {
                    log(&format!("pane gone: {p}, pending pointer dropped"));
                }
            }
        }
    }
}

/// A theme event: logged, or the Claude panes to switch when idle.
fn theme_event(e: theme::Event, live_theme: &mut theme::Live, next: &mut Instant) {
    match e {
        theme::Event::Log(l) => log(&l),
        theme::Event::Live(m) => {
            let panes = theme::claude_panes(&local_agents());
            log(&format!(
                "theme: {} claude pane(s) get /config theme={} when idle",
                panes.len(),
                m.as_str()
            ));
            live_theme.start(m, panes.into_keys());
            *next = Instant::now();
        }
    }
}

/// What the relay keeps from local mode into the broker loop, and across an
/// exec (the handoff).
#[derive(Default)]
struct Carry {
    d: Delivery,
    sup: Supervisor,
    mail: Vec<ToolMail>,
}

impl Carry {
    fn from_handoff() -> Self {
        let mut c = Carry::default();
        if let Some(h) = service::take_handoff() {
            c.d.import(&h["delivery"]);
            c.sup.import(&h["supervisor"]);
            c.mail = h["mail"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|m| {
                    let s = |i: usize| m[i].as_str().map(String::from);
                    Some((s(0)?, s(1)?, s(2)?))
                })
                .collect();
            let n = |k: &str| h["delivery"][k].as_array().map_or(0, Vec::len);
            log(&format!(
                "upgrade: running the new binary; kept {} pending, {} pointed, {} busy children, {} tool mails",
                n("pending"),
                h["delivery"]["pointed"].as_object().map_or(0, |m| m.len()),
                h["supervisor"]["busy"].as_array().map_or(0, Vec::len),
                c.mail.len(),
            ));
        }
        c
    }
}

/// A replaced binary: exec it at a quiet point, state handed over
/// (docs/design/vegad.md, Lifecycle). Returns only if it did not exec.
fn upgrade_if_due(
    upgrade: &mut service::Upgrade,
    lock: &service::RelayLock,
    quiet: bool,
    d: &Delivery,
    sup: &Supervisor,
    mail: &[ToolMail],
) {
    if !upgrade.due() || !quiet {
        return;
    }
    let state =
        serde_json::json!({"delivery": d.export(), "supervisor": sup.export(), "mail": mail});
    match service::save_handoff(state) {
        Ok(()) => {
            log(&format!(
                "upgrade: binary replaced, exec {}",
                upgrade.exe().display()
            ));
            let why = service::reexec(lock, upgrade.exe());
            log(&format!("upgrade: exec failed, keeping this binary: {why}"));
        }
        Err(e) => log(&format!(
            "upgrade: cannot save state, keeping this binary: {e}"
        )),
    }
    upgrade.failed();
}

/// Parents to wake now: free, with a new attention (supervise.rs).
fn due_wakes(
    sup: &mut Supervisor,
    d: &Delivery,
    view: Option<&vega_registry::View>,
) -> HashMap<String, (String, Attentions)> {
    let mut wakes = HashMap::new();
    for (parent, (current, n, questions)) in attentions(sup, d, view) {
        sup.seed(&parent, || load_woke(&parent));
        if sup.due(&parent, &current) && delivery::is_free(d.status_of(&parent)) {
            let s = supervise::summary(&current, n, |k| questions.get(k).cloned());
            wakes.insert(parent, (s, current));
        }
    }
    wakes
}

/// Types the wakes no mail pointer carried.
fn type_wakes(
    wakes: HashMap<String, (String, Attentions)>,
    sup: &mut Supervisor,
    drafts: &mut Drafts,
) {
    for (parent, (s, current)) in wakes {
        if drafts.holds(&parent) {
            continue; // still due next tick
        }
        match notify(&parent, &wake_line(&s, 0)) {
            Ok(()) => {
                log(&format!("wake: {parent} ({s})"));
                sup.woke(&parent, &current);
                save_woke(&parent, &current);
            }
            Err(f) => log(&format!("wake failed: {parent}: {} {}", f.code, f.message)),
        }
        drafts.restore(&parent);
    }
}

/// Local mode probes for a broker at most this far apart (one connect,
/// bounded, off the loop); the first probe comes after 2 s, then the delay
/// doubles (outage::Backoff).
const PROBE_EVERY: Duration = Duration::from_secs(60);

/// The jobs local mode leaves off, for the log and `relay status`.
pub const LOCAL_OFF: &str = "mail relay, registry, commands, beads sync, ci verdict mail";

/// Why vegad runs without a broker, for the log and `relay status`.
fn local_reason() -> String {
    if broker_configured() {
        format!("local (broker {} unreachable)", url())
    } else {
        "local (no broker configured: VEGA_NATS_URL, ~/.config/vega/nats.url)".into()
    }
}

/// vegad without a broker (docs/design/vegad.md, Local mode): the herdr-side
/// jobs (sidebar metadata, attention and wakes, the upgrade check, theme)
/// keep running; tool mail to this host's panes is typed into them when they
/// are free, the way `vega mux send` pastes without NATS. Returns when a
/// probe reaches a broker; the broker loop takes over the state.
async fn local(
    lock: &service::RelayLock,
    upgrade: &mut service::Upgrade,
    jobs: &mut Jobs,
    carry: Carry,
) -> Carry {
    let why = local_reason();
    // The note first: whoever sees the local state finds it.
    if broker_configured() {
        outage::noted(&url());
    }
    service::write_broker(&why);
    log(&format!(
        "broker: {why}; off: {LOCAL_OFF}; on: herdr metadata, attention, upgrade check, theme; tool mail typed locally; probing with backoff up to every {} s",
        PROBE_EVERY.as_secs()
    ));
    let Carry {
        mut d,
        mut sup,
        mut mail,
    } = carry;
    let mut drafts = Drafts::default();
    let notes_store = vega_herdr::notes::Store::local();
    let mut notes_at = None;
    let mut decisions = DecisionCache::default();
    let mut hygiene = hygiene::Pass::new(service::state_dir().ok().as_deref());
    let mut meta_next: Option<Instant> = None;
    let mut park_failing: HashSet<String> = HashSet::new();
    let mut live_theme = theme::Live::default();
    let mut live_theme_next = Instant::now();
    let mut probe_at = Instant::now() + jobs.backoff.next();
    let mut probe: Option<tokio::task::JoinHandle<bool>> = None;
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    loop {
        let started = Instant::now();
        if !live_theme.pending.is_empty() && Instant::now() >= live_theme_next {
            live_theme_next = Instant::now() + LIVE_THEME_EVERY;
            type_theme(&mut live_theme, &drafts);
        }
        let changed = notes_store.as_ref().and_then(|s| s.changed());
        if changed != notes_at {
            notes_at = changed;
            meta_next = None;
        }
        if meta_next.is_none_or(|t| Instant::now() >= t) {
            check_parks(&mut park_failing);
            if let Some(s) = &notes_store {
                s.prune((vega_herdr::notes::now_ms() / 1000) as i64);
            }
            let t = Instant::now();
            let due = report_metadata(&sup, &d, None, &mut decisions, &mut hygiene);
            if !due.is_empty() {
                log("local: hygiene mail waits until its UAC is free");
                mail.extend(due);
            }
            slow("metadata", t);
            meta_next = Some(Instant::now() + REGISTRY_EVERY);
        }
        for p in sup.ripe(SILENT_AFTER) {
            turn_ended(&p, &mut d);
        }
        let wakes = due_wakes(&mut sup, &d, None);
        type_wakes(wakes, &mut sup, &mut drafts);
        mail = deliver_local(mail, &d, &mut drafts);
        upgrade_if_due(upgrade, lock, drafts.stashed.is_empty(), &d, &sup, &mail);
        if probe.is_none() && Instant::now() >= probe_at {
            probe = Some(tokio::spawn(async {
                outage::bounded("probe", connect("probe", None))
                    .await
                    .is_ok()
            }));
        }
        slow("local loop", started);
        tokio::select! {
            Some(ok) = async {
                match probe.as_mut() {
                    Some(h) => h.await.ok(),
                    None => std::future::pending().await,
                }
            } => {
                probe = None;
                probe_at = Instant::now() + jobs.backoff.next();
                if ok {
                    log("broker: reachable, leaving local mode");
                    return Carry { d, sup, mail };
                }
            }
            Some(e) = jobs.events.recv() => {
                if let agents::Event::Gone(p) = &e {
                    let before = mail.len();
                    mail.retain(|(_, to, _)| to != p);
                    if mail.len() < before {
                        log(&format!("local: pane {p} gone, its tool mail dropped"));
                    }
                }
                let before = d.bounces().len();
                jobs.agent(e, &mut d, &mut sup);
                // Kept in `d`: the broker loop bounces them once it is back.
                for b in &d.bounces()[before..] {
                    log(&format!("bounce: {} closed with unread mail; bounced once the broker is back", b.pane));
                }
            }
            Some(e) = jobs.up.recv() => match e {
                upstream::Event::Log(l) => log(&l),
                upstream::Event::Mail(pane, text) => {
                    log(&format!("local: upgrade mail for {pane} waits until it is free"));
                    mail.push((upstream::AGENT.to_string(), pane, text));
                }
            },
            Some(e) = jobs.theme.recv() => theme_event(e, &mut live_theme, &mut live_theme_next),
            // Started by an earlier broker loop: their mail is typed here.
            Some(e) = jobs.ci.recv() => match e {
                ci::Event::Log(l) => log(&l),
                ci::Event::Mail(pane, text) => mail.push((ci::AGENT.to_string(), pane, text)),
            },
            Some(e) = jobs.bd.recv() => match e {
                bdsync::Event::Log(l) => log(&l),
                bdsync::Event::Mail(pane, text) => mail.push((bdsync::AGENT.to_string(), pane, text)),
            },
            _ = tick.tick() => {}
        }
    }
}

/// Longest line local delivery types; the rest of a long changelog is cut.
const LOCAL_LINE_MAX: usize = 1200;

/// A tool mail as the one line local mode types: `[<agent>] ` and the text,
/// its lines joined by ` | `, cut at LOCAL_LINE_MAX. One line, because a
/// multi-line paste can stay staged in Claude's input box.
fn local_line(agent: &str, text: &str) -> String {
    let body = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join(" | ");
    let line = format!("[{agent}] {body}");
    if line.chars().count() <= LOCAL_LINE_MAX {
        return line;
    }
    let cut: String = line.chars().take(LOCAL_LINE_MAX - 1).collect();
    format!("{cut}…")
}

/// Local mode: types each tool mail into its pane once the agent is free,
/// behind the draft guard. Returns the mails that still wait.
fn deliver_local(mail: Vec<ToolMail>, d: &Delivery, drafts: &mut Drafts) -> Vec<ToolMail> {
    let mut left = Vec::new();
    for (agent, pane, text) in mail {
        if !delivery::is_free(d.status_of(&pane)) || drafts.holds(&pane) {
            left.push((agent, pane, text));
            continue;
        }
        match notify(&pane, &local_line(&agent, &text)) {
            Ok(()) => log(&format!("local: {agent} mail typed into {pane}")),
            Err(f) if f.code == "bad_pane" => {
                log(&format!("local: {pane}: {}, dropped", f.message))
            }
            Err(f) => {
                log(&format!(
                    "local: {agent} mail to {pane} failed, retrying: {} {}",
                    f.code, f.message
                ));
                drafts.restore(&pane);
                left.push((agent, pane, text));
                continue;
            }
        }
        drafts.restore(&pane);
    }
    left
}

/// The relay with a broker: mail delivery, commands, quarantine, registry,
/// CI and beads sync on top of the local jobs.
#[allow(clippy::too_many_arguments)]
async fn serve(
    client: async_nats::Client,
    mut reconnected: mpsc::UnboundedReceiver<()>,
    lock: &service::RelayLock,
    upgrade: &mut service::Upgrade,
    host: &str,
    once: bool,
    jobs: &mut Jobs,
    carry: Carry,
) -> Result<Served, Failure> {
    let host = host.to_string();
    let filter = format!("vega.inbox.{host}.>");
    {
        let js = jetstream::new(client.clone());
        // A broker that drops during setup is an outage, not a crash.
        let setup = async {
            let stream = inbox_stream(&js, &host).await?;
            // async-nats re-subscribes by itself after a reconnect.
            let sub = client.subscribe(filter.clone()).await.map_err(broker_err)?;
            Ok((stream, sub))
        };
        let (stream, mut sub) = match outage::bounded("relay setup", setup).await {
            Ok(v) => v,
            Err(f) if !once && matches!(f.code.as_str(), "broker_unreachable" | "broker_error") => {
                log(&format!("broker: setup failed ({}); local mode", f.message));
                return Ok(Served::Lost(Box::new(carry)));
            }
            Err(f) => return Err(f),
        };
        let subscribe = |subject: String| {
            let c = client.clone();
            async move {
                outage::bounded("subscribe", async {
                    c.subscribe(subject).await.map_err(broker_err)
                })
                .await
            }
        };
        // The command channel (cmd.rs): requests for this host. Without the
        // broker right the subscription just stays silent.
        let mut cmds = match subscribe(format!("vega.cmd.{host}.>")).await {
            Ok(s) => Some(s),
            Err(e) => {
                // No broker right yet (titan auth config): mail keeps working.
                log(&format!("commands: not served ({})", e.message));
                None
            }
        };
        // Quarantine (quarantine.rs): replies from inbox-only hosts become
        // notes for the human, never mail. They never enter `d` (delivery).
        let mut quarantined = if st::is_inbox_only(&host) {
            None
        } else {
            match subscribe(format!("vega.quarantine.{host}.>")).await {
                Ok(s) => Some(s),
                Err(e) => {
                    log(&format!("quarantine: not served ({})", e.message));
                    None
                }
            }
        };
        // Uber answers (uber.rs): vegad's own consumer, so an answer is
        // verified before the UAC sees it, also after this host slept. It is
        // made in the loop and retried: until it exists answers wait in the
        // archive, never shown unverified, and its first pass reads them.
        let mut uber_in = None;
        // Only a broker that took the setup ends an outage (vega-lhc.205).
        jobs.backoff = outage::Backoff::new(PROBE_EVERY);
        service::write_broker("connected");
        if outage::cleared((now_ms() / 1000) as i64) {
            log("broker: outage note cleared");
        }
        log(&format!("relay: {host} watching {filter}"));

        // The CI watch (ci.rs): one GitHub poller per host; its mails go to
        // the watchers' inboxes like any other mail. Beads sync (bdsync.rs):
        // pull and push of the provisioned projects; a stopped one mails its
        // UAC.
        jobs.start_broker_jobs();
        let theme_remote = jobs.theme_remote.clone();
        let mut live_theme = theme::Live::default();
        let mut live_theme_next = Instant::now();
        let Carry {
            mut d,
            mut sup,
            mail,
        } = carry;
        // Tool mail the broker did not take yet, retried every CI_RETRY;
        // local mode's undelivered mail goes first.
        let mut ci_unsent: Vec<ToolMail> = mail;
        let mut ci_retry_at = Instant::now();
        // Command handlers still running: an exec waits for them.
        let in_flight = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        // The registry (vegad M4): this host's state into NATS KV every
        // REGISTRY_EVERY, and the other hosts' view back for supervision.
        let mut registry = Registry::default();
        let mut meta_next: Option<Instant> = None;
        // Parks whose bead bd could not show (logged once each).
        let mut park_failing: HashSet<String> = HashSet::new();
        // Mail that arrived while no relay ran, or while the broker was away.
        let mut rescan = true;
        let mut tick = tokio::time::interval(Duration::from_secs(1));
        let mut drafts = Drafts::default();
        let notes_store = vega_herdr::notes::Store::local();
        let mut notes_at = None;
        let mut decisions = DecisionCache::default();
        let mut hygiene = hygiene::Pass::new(service::state_dir().ok().as_deref());
        // Since when the client is disconnected; LOST_AFTER of it ends the
        // broker loop.
        let mut away: Option<Instant> = None;
        // Bounces the broker did not take yet (vega-lhc.205).
        let mut bounces: Vec<delivery::Bounce> = Vec::new();
        let mut bounce_retry_at = Instant::now();
        // A pass over the uber consumer runs in its own task (bd is slow);
        // due at start, on reconnect and on new uber.* mail.
        let (uber_tx, mut uber_rx) = mpsc::unbounded_channel();
        let mut uber_due = true;
        let mut uber_running = false;
        // The outbox owes mail and replies; flushed at start too.
        let mut uber_owed = true;
        let mut uber_retry_at = Instant::now();
        loop {
            let started = Instant::now();
            // While async-nats reconnects, every broker call would only queue
            // in its command channel: leave the broker alone (vega-lhc.205).
            let up = connected(&client);
            if up {
                away = None;
            } else if away.get_or_insert(started).elapsed() >= outage::LOST_AFTER {
                let why = format!(
                    "broker: lost ({} unreachable for {} s); switching to local mode",
                    url(),
                    outage::LOST_AFTER.as_secs()
                );
                if once {
                    return Err(Failure::new(
                        "broker_unreachable",
                        why,
                        unreachable_hint(true),
                    ));
                }
                log(&why);
                d.keep_bounces(std::mem::take(&mut bounces));
                return Ok(Served::Lost(Box::new(Carry {
                    d,
                    sup,
                    mail: ci_unsent,
                })));
            }
            if rescan && up {
                // Every pane that ever got mail; `decide` drops the read ones.
                match outage::bounded("rescan", subject_counts(&stream, &filter)).await {
                    Ok(subjects) => {
                        rescan = false;
                        for (subject, _) in subjects {
                            if let Some((pane, _)) = parse_subject(&subject) {
                                d.rescanned(pane);
                            }
                        }
                    }
                    Err(f) => log(&format!("rescan failed, retrying: {}", f.message)),
                }
            }
            // A waiting agent came or went: vega mux next on other hosts sees
            // it within a second, not at the next 15 s round.
            if jobs.take_waiting_changed() {
                registry.soon();
            }
            if !once && up && registry.due() {
                let t = Instant::now();
                if st::is_inbox_only(&host) {
                    let run = registry.run_theme_only(&client, &theme_remote);
                    if while_up(&client, REGISTRY_CALL, run).await.is_none() {
                        registry.timed_out();
                    }
                } else {
                    let run = registry.run(&client, &host, &mut sup);
                    if while_up(&client, REGISTRY_CALL, run).await.is_none() {
                        registry.timed_out();
                    }
                    if let (Some(v), Ok(mut r)) = (&registry.view, theme_remote.lock()) {
                        *r = theme::remote_modes(v);
                    }
                }
                slow("registry", t);
            }
            if !live_theme.pending.is_empty() && Instant::now() >= live_theme_next {
                live_theme_next = Instant::now() + LIVE_THEME_EVERY;
                let t = Instant::now();
                type_theme(&mut live_theme, &drafts);
                slow("live theme", t);
            }
            // A new note or a read one (the board) redraws the badge now.
            let changed = notes_store.as_ref().and_then(|s| s.changed());
            if changed != notes_at {
                notes_at = changed;
                meta_next = None;
            }
            // The sidebar on its own clock: the registry may be off.
            if !once && meta_next.is_none_or(|t| Instant::now() >= t) {
                check_parks(&mut park_failing);
                if let Some(s) = &notes_store {
                    s.prune((vega_herdr::notes::now_ms() / 1000) as i64);
                }
                let t = Instant::now();
                let due = report_metadata(
                    &sup,
                    &d,
                    registry.view.as_ref(),
                    &mut decisions,
                    &mut hygiene,
                );
                // Published with the CI mail on the next tick.
                ci_unsent.extend(due);
                slow("metadata", t);
                if up {
                    // Open Uber asks: withdraw on bead close, resend on a new epoch.
                    let t = Instant::now();
                    let run = uber_ask::round(&js, &host, registry.view.as_ref());
                    while_up(&client, outage::CALL, run).await;
                    slow("uber asks", t);
                }
                meta_next = Some(Instant::now() + REGISTRY_EVERY);
            }
            for p in sup.ripe(SILENT_AFTER) {
                turn_ended(&p, &mut d);
            }
            for asker in expired_asks() {
                d.mail(&asker, Sender::Other);
            }
            // A bounce the broker could not check waits for the next try.
            bounces.extend(d.take_bounces());
            if up && !bounces.is_empty() && Instant::now() >= bounce_retry_at {
                let mut later = Vec::new();
                for mut b in std::mem::take(&mut bounces) {
                    let run = bounce(&js, &stream, &host, &mut b, registry.view.as_ref());
                    if !while_up(&client, outage::CALL, run).await.unwrap_or(false) {
                        later.push(b);
                    }
                }
                if !later.is_empty() {
                    log(&format!(
                        "bounce: {} not checked, retrying in {} s",
                        later.len(),
                        CI_RETRY.as_secs()
                    ));
                    bounce_retry_at = Instant::now() + CI_RETRY;
                }
                bounces = later;
            }
            let uber_ready = up && uber_due && !uber_running && Instant::now() >= uber_retry_at;
            if uber_ready && uber_in.is_none() && !st::is_inbox_only(&host) {
                match outage::bounded("uber consumer", uber_consumer(&stream, &host)).await {
                    Ok(c) => uber_in = Some(c),
                    Err(e) => {
                        log(&format!("uber: consumer not made, retrying: {}", e.message));
                        uber_retry_at = Instant::now() + CI_RETRY;
                    }
                }
            }
            if uber_ready && Instant::now() >= uber_retry_at {
                if let Some(c) = uber_in.clone() {
                    uber_due = false;
                    uber_running = true;
                    let tx = uber_tx.clone();
                    tokio::spawn(async move {
                        let dir = vega_registry::uber_asks::dir();
                        let _ = tx.send(uber_pass(&c, dir).await);
                    });
                }
            }
            if up && uber_owed && !uber_running && Instant::now() >= uber_retry_at {
                if let Some(asks) = vega_registry::uber_asks::dir() {
                    uber_owed = !uber_flush(&js, &host, &uber::outbox(&asks), uber_note).await;
                    if uber_owed {
                        uber_retry_at = Instant::now() + CI_RETRY;
                    }
                } else {
                    uber_owed = false;
                }
            }
            // Parents to wake now: free, with a new attention (supervise.rs).
            let mut wakes = if once {
                HashMap::new()
            } else {
                due_wakes(&mut sup, &d, registry.view.as_ref())
            };
            // Without the broker no count: candidates stay pending.
            for pane in d
                .candidates(once)
                .into_iter()
                .filter(|_| connected(&client))
            {
                let (count, floor) =
                    match outage::bounded("count", unread_and_floor(&stream, &host, &pane)).await {
                        Ok(n) => n,
                        Err(f) => {
                            log(&format!("count failed for {pane}, retrying: {}", f.message));
                            continue; // stays pending
                        }
                    };
                if d.decide(&pane, count, floor) != Decision::Type {
                    continue;
                }
                if drafts.holds(&pane) {
                    continue; // stays pending
                }
                let started = Instant::now();
                let again = d.repeat(&pane);
                // One line for both: the wake and the mail.
                let wake = wakes.remove(&pane);
                let line = match &wake {
                    Some((s, _)) => wake_line(s, count),
                    None => pointer(count),
                };
                let typed = match notify(&pane, &line) {
                    Ok(()) => {
                        log(&format!(
                            "pointer: {pane} ({count} pending{}{}, agent {}), herdr {} ms",
                            wake.as_ref()
                                .map(|(s, _)| format!("; wake: {s}"))
                                .unwrap_or_default(),
                            if again { ", again" } else { "" },
                            d.status_of(&pane),
                            started.elapsed().as_millis()
                        ));
                        if let Some((_, current)) = &wake {
                            sup.woke(&pane, current);
                            save_woke(&pane, current);
                        }
                        true
                    }
                    Err(f) => {
                        log(&format!("pointer failed: {pane}: {} {}", f.code, f.message));
                        // A pane that is not a pane id never gets one.
                        f.code == "bad_pane"
                    }
                };
                drafts.restore(&pane);
                d.typed(&pane, floor, typed);
            }
            type_wakes(wakes, &mut sup, &mut drafts);
            if once && d.candidates(true).is_empty() && !rescan {
                break;
            }
            if !once {
                let quiet = drafts.stashed.is_empty()
                    && in_flight.load(std::sync::atomic::Ordering::SeqCst) == 0;
                upgrade_if_due(upgrade, lock, quiet, &d, &sup, &ci_unsent);
            }
            slow("loop", started);
            tokio::select! {
                msg = sub.next() => {
                    let Some(msg) = msg else { break };
                    let Some(pane) = parse_subject(&msg.subject).map(|(p, _)| p.to_string()) else { continue };
                    let hv = |k: &str| msg.headers.as_ref().and_then(|hs| hs.get(k)).map(|v| v.as_str().to_string());
                    let latency = hv(h::SENT_MS)
                        .and_then(|v| v.parse::<u128>().ok())
                        .map(|t| now_ms().saturating_sub(t).to_string())
                        .unwrap_or_else(|| "?".into());
                    let now = hv(h::NOW).is_some();
                    log(&format!(
                        "seen: {} -> {pane}{}, agent {}, broker latency {latency} ms",
                        hv(h::FROM_PANE).unwrap_or_else(|| "?".into()),
                        if now { " (--now)" } else { "" },
                        d.status_of(&pane),
                    ));
                    let from_host = parse_subject(&msg.subject).map(|(_, f)| f.to_string()).unwrap_or_default();
                    let kind = hv(h::KIND);
                    if kind.is_some() {
                        uber_due = true;
                    }
                    // An uber.answer is vegad's: no pointer, the pass decides.
                    if uber::hidden(kind.as_deref()) {
                        continue;
                    }
                    let mut held = false;
                    let mut sender = if now { Sender::Urgent } else { Sender::Other };
                    if let Some(from) = hv(h::FROM_PANE) {
                        let done = hv(h::DONE).and_then(|v| Outcome::parse(&v));
                        if supervise_mail(&pane, &from, &from_host, done, &hv(h::SENT_MS).unwrap_or_default(), &String::from_utf8_lossy(&msg.payload)) && !now {
                            sender = Sender::Parent;
                        }
                        if let Some(t) = hv(h::THREAD) {
                            held = hold_answer(&t, &pane, &from_host, &from);
                        }
                    }
                    if !held {
                        let origin = delivery::Origin {
                            host: from_host.clone(),
                            pane: hv(h::FROM_PANE).unwrap_or_default(),
                            agent: hv(h::FROM_AGENT).unwrap_or_default(),
                        };
                        d.mail_from(&pane, sender, Some(origin));
                    }
                }
                Some(req) = async {
                    match quarantined.as_mut() {
                        Some(s) => s.next().await,
                        None => std::future::pending().await,
                    }
                } => {
                    if let Some(reply) = req.reply.clone() {
                        let c = client.clone();
                        let host = host.clone();
                        tokio::spawn(async move {
                            let answer = tokio::task::spawn_blocking(move || store_quarantined(&req, &host))
                                .await
                                .unwrap_or_default();
                            let _ = tokio::time::timeout(outage::CALL, c.publish(reply, answer.into())).await;
                        });
                    }
                }
                Some(req) = async {
                    match cmds.as_mut() {
                        Some(s) => s.next().await,
                        None => std::future::pending().await,
                    }
                } => {
                    let parsed = cmd::parse_subject(&req.subject).map(|(_, k, f)| (k.to_string(), f.to_string()));
                    if let (Some(reply), Some((kind, from))) = (req.reply.clone(), parsed) {
                        let payload = req.payload.to_vec();
                        let c = client.clone();
                        let busy = in_flight.clone();
                        busy.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        tokio::spawn(async move {
                            let (k, f) = (kind.clone(), from.clone());
                            let answer = tokio::task::spawn_blocking(move || cmd::serve(&k, &f, &payload))
                                .await
                                .unwrap_or_default();
                            let v: serde_json::Value = serde_json::from_slice(&answer).unwrap_or_default();
                            log(&format!("cmd: {kind} from {from}: {} {}", v["code"].as_str().unwrap_or("?"), v["message"].as_str().unwrap_or("")));
                            let _ = tokio::time::timeout(outage::CALL, c.publish(reply, answer.into())).await;
                            busy.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
                        });
                    }
                }
                Some(r) = uber_rx.recv() => {
                    uber_running = false;
                    uber_owed = true;
                    if let Some(f) = r {
                        log(&format!("uber: pass failed, retrying: {}", f.message));
                        uber_due = true;
                        uber_retry_at = Instant::now() + CI_RETRY;
                    }
                }
                Some(e) = jobs.events.recv() => jobs.agent(e, &mut d, &mut sup),
                Some(e) = jobs.ci.recv() => match e {
                    ci::Event::Log(l) => log(&l),
                    ci::Event::Mail(pane, text) => {
                        ci_unsent.push((ci::AGENT.to_string(), pane, text));
                        if up && Instant::now() >= ci_retry_at {
                            ci_unsent = publish_tool_mail(&js, &host, ci_unsent).await;
                            ci_retry_at = Instant::now() + CI_RETRY;
                        }
                    }
                },
                Some(e) = jobs.theme.recv() => theme_event(e, &mut live_theme, &mut live_theme_next),
                Some(e) = jobs.bd.recv() => match e {
                    bdsync::Event::Log(l) => log(&l),
                    bdsync::Event::Mail(pane, text) => {
                        ci_unsent.push((bdsync::AGENT.to_string(), pane, text));
                        if up && Instant::now() >= ci_retry_at {
                            ci_unsent = publish_tool_mail(&js, &host, ci_unsent).await;
                            ci_retry_at = Instant::now() + CI_RETRY;
                        }
                    }
                },
                Some(e) = jobs.up.recv() => match e {
                    upstream::Event::Log(l) => log(&l),
                    upstream::Event::Mail(pane, text) => {
                        ci_unsent.push((upstream::AGENT.to_string(), pane, text));
                        if up && Instant::now() >= ci_retry_at {
                            ci_unsent = publish_tool_mail(&js, &host, ci_unsent).await;
                            ci_retry_at = Instant::now() + CI_RETRY;
                        }
                    }
                },
                Some(()) = reconnected.recv() => {
                    // A late signal of an earlier connect says nothing now.
                    if client.connection_state() == async_nats::connection::State::Connected {
                        log("broker: connected, rescanning");
                    }
                    rescan = true;
                    uber_due = true;
                }
                _ = tick.tick() => {
                    if up && !ci_unsent.is_empty() && Instant::now() >= ci_retry_at {
                        ci_unsent = publish_tool_mail(&js, &host, ci_unsent).await;
                        ci_retry_at = Instant::now() + CI_RETRY;
                    }
                }
            }
        }
        Ok(Served::Done)
    }
}

/// How often vegad looks at the Claude panes still waiting for a theme
/// switch (one `herdr agent list` per look, only while some wait).
const LIVE_THEME_EVERY: Duration = Duration::from_secs(2);

/// This host's agents (`herdr agent list`); empty when herdr is away.
fn local_agents() -> Vec<serde_json::Value> {
    herdr(&["agent", "list"])
        .ok()
        .and_then(|v| v["result"]["agents"].as_array().cloned())
        .unwrap_or_default()
}

/// Types `/config theme=<mode>` into each waiting Claude pane that is idle
/// or done with an empty input and no dialog (theme::decide); a working pane
/// waits for its next idle, a closed one is dropped. Once per pane.
fn type_theme(live: &mut theme::Live, drafts: &Drafts) {
    let Some(line) = live.line() else { return };
    let now = theme::claude_panes(&local_agents());
    for pane in live.pending.clone() {
        let status = now.get(&pane).map(String::as_str);
        let held = drafts.stashed.contains_key(&pane);
        match theme::decide(status, held, || screen(&pane)) {
            theme::Action::Wait => {}
            theme::Action::Drop => {
                live.pending.remove(&pane);
            }
            theme::Action::Send => {
                live.pending.remove(&pane);
                match notify(&pane, &line) {
                    Ok(()) => log(&format!("theme: typed {line} into {pane}")),
                    Err(f) => log(&format!("theme: {pane}: {} {}", f.code, f.message)),
                }
            }
        }
    }
}

/// A broker that refused CI mail is retried this often, so a broker outage
/// does not stall the relay loop on every tick.
const CI_RETRY: Duration = Duration::from_secs(30);

/// Publishes tool mails (CI watch, ci.rs; beads sync, bdsync.rs) into this
/// host's inboxes, without a sender pane: nobody to reply to. The relay's own
/// subscription then types the pointer when the recipient is free. Returns
/// the mails the broker refused.
async fn publish_tool_mail(
    js: &jetstream::Context,
    host: &str,
    mails: Vec<ToolMail>,
) -> Vec<ToolMail> {
    let mut unsent = Vec::new();
    for (agent, pane, text) in mails {
        if !unsent.is_empty() {
            unsent.push((agent, pane, text));
            continue;
        }
        let mut headers = HeaderMap::new();
        headers.insert("Nats-Msg-Id", nuid::next().as_str());
        headers.insert(h::FROM_AGENT, agent.as_str());
        headers.insert(h::FROM_CWD, "vegad");
        headers.insert(h::SENT_MS, now_ms().to_string().as_str());
        let subject = inbox_subject(host, &pane, host);
        let publish = async {
            match js
                .publish_with_headers(subject, headers, text.clone().into())
                .await
            {
                Ok(ack) => ack.await.map(|_| ()).map_err(|e| e.to_string()),
                Err(e) => Err(e.to_string()),
            }
        };
        let sent = tokio::time::timeout(outage::CALL, publish)
            .await
            .unwrap_or_else(|_| Err("no answer from the broker".into()));
        if let Err(e) = sent {
            log(&format!("{agent}: mail to {pane} not sent, retrying: {e}"));
            unsent.push((agent, pane, text));
        }
    }
    unsent
}

/// The sender of a bounce (vega-lhc.129).
const BOUNCE_AGENT: &str = "tool vega-post";

/// The bounce's Nats-Msg-Id: the same for a retry of one bounce to one
/// sender, different for another sender or another close of the pane.
fn bounce_msg_id(host: &str, b: &delivery::Bounce, o: &delivery::Origin) -> String {
    format!(
        "bounce:{host}:{}:{}:{}:{}:{}",
        b.pane, b.closed_ms, o.host, o.pane, o.agent
    )
}

/// What a bounce says: which pane closed, and whom to mail instead.
fn bounce_text(pane: &str, host: &str, hint: &str) -> String {
    format!(
        "bounce: {pane} on {host} closed before it read your mail; nobody will read it there. {hint}. Resend it if it still matters."
    )
}

/// vegad's bounce (vega-lhc.129): a closed pane's unread mail goes back to
/// each sender once (`delivery::Bounce`), with the project's UAC in the
/// hint. Only when the broker still counts that mail unread and herdr no
/// longer lists the pane (a restarting herdr may report every pane gone).
async fn bounce(
    js: &jetstream::Context,
    stream: &stream::Stream,
    host: &str,
    b: &mut delivery::Bounce,
    view: Option<&vega_registry::View>,
) -> bool {
    let count = match unread(stream, host, &b.pane).await {
        Ok(n) => n,
        Err(f) => {
            log(&format!("bounce: {} not checked: {}", b.pane, f.message));
            return false;
        }
    };
    let senders = b.senders(count);
    if senders.is_empty() {
        return true;
    }
    match local_pane_open(&b.pane) {
        Some(false) => {}
        // Not closed after all (a restarting herdr reported it gone).
        Some(true) => {
            log(&format!(
                "bounce: herdr still lists {}; not bounced",
                b.pane
            ));
            return true;
        }
        // Cannot tell yet: the next round asks again.
        None => {
            log(&format!("bounce: {}: herdr does not answer", b.pane));
            return false;
        }
    }
    let project = view
        .and_then(|v| reach::project_of(v, host, &b.pane))
        .or_else(|| {
            st::project_uac_records()
                .into_iter()
                .find(|(_, p, _)| *p == b.pane)
                .map(|(k, _, _)| k)
        });
    let hint = reach::gone_hint(view, project.as_deref(), (host, &b.pane));
    let text = bounce_text(&b.pane, host, &hint);
    let mut ok = true;
    for o in senders {
        if o.pane.is_empty() {
            // The user from the board on this host: a note; tools get none.
            if o.agent == "the user" && o.host == host {
                vega_herdr::notes::notify(
                    "bounce",
                    Some(&b.pane),
                    &format!("{} closed with your mail unread", b.pane),
                    &text,
                );
                log(&format!("bounce: {} unread, noted for the user", b.pane));
            }
            b.done.push(o);
            continue;
        }
        let mut headers = HeaderMap::new();
        // Stable: a retry after a lost ack is deduplicated by the broker.
        let id = bounce_msg_id(host, b, &o);
        headers.insert("Nats-Msg-Id", id.as_str());
        headers.insert(h::FROM_AGENT, BOUNCE_AGENT);
        headers.insert(h::FROM_CWD, "vegad");
        headers.insert(h::SENT_MS, now_ms().to_string().as_str());
        let subject = inbox_subject(&o.host, &o.pane, host);
        let sent = match js
            .publish_with_headers(subject, headers, text.clone().into())
            .await
        {
            Ok(ack) => ack.await.map(|_| ()).map_err(|e| e.to_string()),
            Err(e) => Err(e.to_string()),
        };
        match sent {
            Ok(()) => {
                log(&format!(
                    "bounce: {} unread, mailed {} on {}",
                    b.pane, o.pane, o.host
                ));
                b.done.push(o);
            }
            Err(e) => {
                log(&format!(
                    "bounce: {} to {} on {} failed, retrying: {e}",
                    b.pane, o.pane, o.host
                ));
                ok = false;
            }
        }
    }
    ok
}

/// `vega post log`: the archive of this machine's mail, newest first. It
/// reads through an ordered consumer (ephemeral, no acks), so no read state
/// changes. Read/unread comes from each pane's durable consumer ack floor.
fn log_view(
    pane: Option<&str>,
    from: Option<&str>,
    since: Option<&str>,
    limit: usize,
    full: bool,
) -> Result<(), Failure> {
    let since = since.map(parse_duration).transpose()?;
    let host = host();
    let filter = format!(
        "vega.inbox.{}.{}.{}",
        host,
        pane.map(subject_token).unwrap_or_else(|| "*".into()),
        from.map(subject_token).unwrap_or_else(|| "*".into())
    );
    runtime()?.block_on(async {
        let client = connect("log", None).await?;
        let js = jetstream::new(client);
        let stream = inbox_stream(&js, &host).await?;
        let deliver_policy = match since {
            Some(d) => jetstream::consumer::DeliverPolicy::ByStartTime {
                start_time: time::OffsetDateTime::now_utc() - d,
            },
            None => jetstream::consumer::DeliverPolicy::All,
        };
        let consumer = stream
            .create_consumer(pull::OrderedConfig {
                // Named: an unnamed consumer is created via the bare
                // `$JS.API.CONSUMER.CREATE.<stream>`, which the machine
                // permissions (`CONSUMER.*.<stream>.>`) do not allow.
                name: Some(format!("log-{host}-{}", nuid::next())),
                filter_subject: filter.clone(),
                deliver_policy,
                ..Default::default()
            })
            .await
            .map_err(broker_err)?;
        let mut msgs = consumer.messages().await.map_err(broker_err)?;

        // Keep the newest `limit` rows; count all matches.
        let mut rows: VecDeque<LogRow> = VecDeque::new();
        let mut total = 0usize;
        // No match at all, or none newer than --since: the stream never ends,
        // so stop when nothing arrives for a moment.
        while let Ok(Some(m)) = tokio::time::timeout(Duration::from_millis(500), msgs.next()).await {
            let m = m.map_err(broker_err)?;
            let info = m.info().map_err(broker_err)?;
            let (to, from_host) = parse_subject(&m.subject).unwrap_or(("?", "?"));
            let hv = |k: &str| {
                m.headers
                    .as_ref()
                    .and_then(|hs| hs.get(k))
                    .map(|v| v.as_str().to_string())
                    .unwrap_or_else(|| "?".into())
            };
            total += 1;
            rows.push_back(LogRow {
                id: info.stream_sequence,
                time: info
                    .published
                    .to_offset(time::UtcOffset::UTC)
                    .format(time::macros::format_description!(
                        "[year]-[month]-[day]T[hour]:[minute]:[second]Z"
                    ))
                    .unwrap_or_else(|_| "?".into()),
                to: to.to_string(),
                from: format!("{}@{from_host}", hv(h::FROM_PANE)),
                hop: hv(h::HOP),
                text: uber::printable(
                    m.headers.as_ref().and_then(|hs| hs.get(h::KIND)).map(|v| v.as_str()),
                    &m.payload,
                ),
            });
            if rows.len() > limit {
                rows.pop_front();
            }
            if info.pending == 0 {
                break;
            }
        }

        let scope = describe_scope(&host, pane, from, since.is_some());
        if rows.is_empty() {
            println!("log: 0 messages {scope}");
            return Ok(());
        }
        // Read state per pane: acked up to the consumer's ack floor.
        let mut floors: HashMap<String, Option<u64>> = HashMap::new();
        for r in &rows {
            if !floors.contains_key(&r.to) {
                let floor = stream
                    .consumer_info(consumer_name(&host, &r.to))
                    .await
                    .ok()
                    .map(|i| i.ack_floor.stream_sequence);
                floors.insert(r.to.clone(), floor);
            }
        }

        println!("log: {} of {total} messages {scope}, newest first", rows.len());
        println!("messages[{}]{{id,time,to,from,hop,read,text}}:", rows.len());
        let mut truncated = false;
        for r in rows.iter().rev() {
            let read = match floors.get(&r.to).copied().flatten() {
                Some(floor) if r.id <= floor => "yes",
                Some(_) => "no",
                None => "no",
            };
            let text = if full {
                r.text.clone()
            } else {
                let t = r.text.replace('\n', " ");
                if t.chars().count() > LOG_TEXT {
                    truncated = true;
                }
                shorten(&t, LOG_TEXT)
            };
            println!(
                "  {},{},{},{},{},{read},{}",
                r.id,
                r.time,
                r.to,
                r.from,
                r.hop,
                cell(&text)
            );
        }
        if total > rows.len() {
            println!(
                "help: {} older messages not shown; raise --limit or narrow with --pane, --from, --since",
                total - rows.len()
            );
        }
        if truncated {
            println!("help: text truncated at {LOG_TEXT} chars; add --full");
        }
        Ok(())
    })
}

struct LogRow {
    id: u64,
    time: String,
    to: String,
    from: String,
    hop: String,
    text: String,
}

fn describe_scope(host: &str, pane: Option<&str>, from: Option<&str>, since: bool) -> String {
    let mut s = format!(
        "to {}",
        pane.map(|p| format!("{p} on {host}"))
            .unwrap_or_else(|| host.to_string())
    );
    if let Some(f) = from {
        s.push_str(&format!(" from {f}"));
    }
    if since {
        s.push_str(" in the --since window");
    }
    s
}

fn shorten(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let head: String = s.chars().take(max - 1).collect();
        format!("{head}…")
    }
}

/// `30s`, `30m`, `2h`, `7d` as a duration.
fn parse_duration(s: &str) -> Result<Duration, Failure> {
    let bad = || {
        Failure::new(
            "usage",
            format!("--since {s} is not a duration"),
            "use a number with s, m, h or d, e.g. 30m, 2h, 7d",
        )
    };
    let (num, unit) = s.split_at(s.len().saturating_sub(1));
    let n: u64 = num.parse().map_err(|_| bad())?;
    let secs = match unit {
        "s" => 1,
        "m" => 60,
        "h" => 3600,
        "d" => 86400,
        _ => return Err(bad()),
    };
    Ok(Duration::from_secs(n * secs))
}

/// Messages per subject in the archive, read or not.
async fn subject_counts(
    stream: &stream::Stream,
    filter: &str,
) -> Result<Vec<(String, usize)>, Failure> {
    let mut subjects = stream
        .info_with_subjects(filter)
        .await
        .map_err(broker_err)?;
    let mut out = Vec::new();
    while let Some(s) = subjects.next().await {
        out.push(s.map_err(broker_err)?);
    }
    Ok(out)
}

/// Types the pointer as a new prompt. Blocking herdr calls are fine here: the
/// relay is single-threaded and herdr answers in milliseconds.
/// What `parent` was last woken for (`woke/` record, one `pane attention`
/// per line), so a relay restart does not wake it again for the same.
fn load_woke(parent: &str) -> Attentions {
    let Some(text) =
        st::state_file_on("woke", None, parent).and_then(|f| std::fs::read_to_string(f).ok())
    else {
        return Attentions::new();
    };
    text.lines()
        .filter_map(|l| {
            let (pane, a) = l.split_once(' ')?;
            let a = st::ATTENTIONS.into_iter().find(|x| x.as_str() == a)?;
            Some((pane.to_string(), a))
        })
        .collect()
}

fn save_woke(parent: &str, current: &Attentions) {
    let Some(file) = st::state_file_on("woke", None, parent) else {
        return;
    };
    let _ = file.parent().map(std::fs::create_dir_all);
    let text: String = current
        .iter()
        .map(|(pane, a)| format!("{pane} {}\n", a.as_str()))
        .collect();
    let _ = std::fs::write(file, text);
}

/// A pane with a held pointer is read again at most this often.
const DRAFT_RECHECK: Duration = Duration::from_secs(3);

/// A draft unchanged this long is abandoned: it is stashed, the pointer
/// typed, and the draft typed back (without Enter). A draft the screen does
/// not hold faithfully (paste placeholder) keeps holding; the user gets one
/// notification that a pointer waits.
const DRAFT_STALE: Duration = Duration::from_secs(300);

/// The `seen` text of a draft that cannot be restored.
const OPAQUE: &str = "\0opaque";

/// A screen without a known input box (delivery::Draft::Unknown) holds the
/// pointer until it has not changed for this long: the user is not typing.
/// A draft left there that long can still be joined (no box to read).
const UNKNOWN_QUIET: Duration = Duration::from_secs(20);

/// The `seen` prefix of an unknown screen; the screen itself follows.
const UNKNOWN: &str = "\0unknown:";

/// Drafts in input lines (delivery::draft): pointers never join them.
#[derive(Default)]
struct Drafts {
    /// Pane -> (draft text, unchanged since, last read).
    seen: HashMap<String, (String, Instant, Instant)>,
    /// Pane -> draft cleared for a pointer, to type back after it.
    stashed: HashMap<String, String>,
    /// Panes told that a pointer waits behind an opaque draft or a screen
    /// that never settles.
    nagged: std::collections::HashSet<String>,
    /// Pane -> first hold on an unknown screen.
    unknown: HashMap<String, Instant>,
}

impl Drafts {
    /// True while the pointer must wait. A stale draft is cleared (and
    /// stashed) instead; an unreadable screen does not hold, an unknown one
    /// holds until it is quiet.
    fn holds(&mut self, pane: &str) -> bool {
        let now = Instant::now();
        if self
            .seen
            .get(pane)
            .is_some_and(|(_, _, read)| now.duration_since(*read) < DRAFT_RECHECK)
        {
            return true;
        }
        let shown = screen(pane);
        let draft = shown.as_deref().map(delivery::draft);
        // Stashed at an earlier check: go once the line shows empty.
        if let Some(stash) = self.stashed.get(pane).cloned() {
            match &draft {
                Some(Draft::None) => return false,
                None | Some(Draft::Unknown) => return true,
                Some(Draft::Text(t)) if *t == stash => {
                    // The clear did not take: an ordinary draft again.
                    self.stashed.remove(pane);
                }
                Some(_) => {
                    // New text over the cleared line: hand the old one back.
                    self.stashed.remove(pane);
                    lost_draft(pane, &stash);
                }
            }
        }
        let text = match draft {
            Some(Draft::Text(t)) => t,
            // Not restorable from the screen: it waits for the user.
            Some(Draft::Opaque) => OPAQUE.into(),
            Some(Draft::Unknown) => {
                return self.unknown_holds(pane, shown.as_deref().unwrap_or_default(), now)
            }
            _ => {
                self.seen.remove(pane);
                self.nagged.remove(pane);
                self.unknown.remove(pane);
                return false;
            }
        };
        self.unknown.remove(pane);
        let since = match self.seen.get(pane) {
            Some((old, since, _)) if *old == text => *since,
            Some(_) => now,
            None => {
                log(&format!(
                    "pointer held: {pane} has a draft in its input line"
                ));
                now
            }
        };
        self.seen
            .insert(pane.to_string(), (text.clone(), since, now));
        if now.duration_since(since) < DRAFT_STALE {
            return true;
        }
        if text == OPAQUE {
            if self.nagged.insert(pane.to_string()) {
                log(&format!(
                    "draft opaque: {pane}; pointer waits, user notified"
                ));
                vega_herdr::notes::notify(
                    "draft",
                    Some(pane),
                    &format!("vegad: a message waits for {pane}"),
                    "Your unsent draft there holds it: send or clear it.",
                );
            }
            return true;
        }
        // Stale: stash first, then clear; the screen may lag the keys, so
        // a line not yet empty is decided at the next check (above).
        log(&format!(
            "draft stashed: {pane} ({} chars, unchanged {} s)",
            text.chars().count(),
            since.elapsed().as_secs()
        ));
        self.stashed.insert(pane.to_string(), text);
        // ctrl+c clears Claude Code's whole box; ctrl+e ctrl+u one line.
        let keys: &[&str] = if self.stashed[pane].contains('\n') {
            &["ctrl+c"]
        } else {
            &["ctrl+e", "ctrl+u"]
        };
        let mut args = vec!["pane", "send-keys", pane];
        args.extend(keys);
        let _ = herdr(&args);
        !settles(pane, |d| d == Draft::None)
    }

    /// An unknown screen (delivery::Draft::Unknown): the pointer waits until
    /// the screen has not changed for UNKNOWN_QUIET. One that keeps changing
    /// for DRAFT_STALE notifies the user once and keeps holding.
    fn unknown_holds(&mut self, pane: &str, shown: &str, now: Instant) -> bool {
        let key = format!("{UNKNOWN}{shown}");
        let since = match self.seen.get(pane) {
            Some((old, since, _)) if *old == key => *since,
            _ => now,
        };
        let first = *self.unknown.entry(pane.to_string()).or_insert_with(|| {
            log(&format!(
                "pointer held: {pane} shows no known input box; waiting for a quiet screen"
            ));
            now
        });
        if now.duration_since(since) >= UNKNOWN_QUIET {
            log(&format!(
                "pointer: {pane} shows no known input box, screen quiet {} s",
                now.duration_since(since).as_secs()
            ));
            self.seen.remove(pane);
            self.unknown.remove(pane);
            self.nagged.remove(pane);
            return false;
        }
        self.seen.insert(pane.to_string(), (key, since, now));
        if now.duration_since(first) >= DRAFT_STALE && self.nagged.insert(pane.to_string()) {
            log(&format!(
                "screen busy: {pane}; pointer waits, user notified"
            ));
            vega_herdr::notes::notify(
                "draft",
                Some(pane),
                &format!("vegad: a message waits for {pane}"),
                "Its screen keeps changing and shows no known input box: run `vega post inbox` there.",
            );
        }
        true
    }

    /// Types a stashed draft back after the pointer, and checks it arrived;
    /// if not, a herdr notification carries it so it is never lost.
    fn restore(&mut self, pane: &str) {
        let Some(text) = self.stashed.remove(pane) else {
            return;
        };
        let _ = herdr(&["pane", "send-text", pane, &text]);
        if settles(pane, |d| d == Draft::Text(text.clone())) {
            log(&format!("draft restored: {pane}"));
        } else {
            lost_draft(pane, &text);
        }
    }
}

/// True once the pane's input line matches `want`, read up to 3 times
/// over 1.5 s: the screen lags keys and text.
fn settles(pane: &str, want: impl Fn(Draft) -> bool) -> bool {
    (0..3).any(|_| {
        std::thread::sleep(Duration::from_millis(500));
        screen(pane).is_some_and(|s| want(delivery::draft(&s)))
    })
}

/// A draft vegad could not put back: a herdr notification carries it, and
/// the board's notification list keeps it.
fn lost_draft(pane: &str, text: &str) {
    log(&format!(
        "draft restore failed: {pane}; sent as a notification"
    ));
    vega_herdr::notes::notify(
        "lost-draft",
        Some(pane),
        &format!("vegad: your draft in {pane}"),
        text,
    );
}

/// The pane's visible screen with colours (dim marks suggestions).
fn screen(pane: &str) -> Option<String> {
    if !valid_pane_id(pane) {
        return None;
    }
    vega_herdr::herdr_text(&[
        "pane", "read", pane, "--source", "visible", "--format", "ansi",
    ])
    .ok()
}

fn notify(pane: &str, line: &str) -> Result<(), Failure> {
    // The pane token comes from the sender's subject: never let it reach
    // herdr as anything but a pane id (e.g. `--flag` or an agent name).
    if !valid_pane_id(pane) {
        return Err(Failure::new(
            "bad_pane",
            format!("{pane} is not a herdr pane id"),
            "the sender used a malformed target; the mail stays in the archive",
        ));
    }
    herdr(&["agent", "prompt", pane, line])?;
    Ok(())
}

/// The line that wakes a parent; `mail`: its unread count, said in the same
/// line so the parent gets one prompt, not two.
fn wake_line(summary: &str, mail: u64) -> String {
    match mail {
        // A question already ends the sentence.
        0 if summary.ends_with(['?', '…']) => format!("[vega] {summary} Run: vega uac"),
        0 => format!("[vega] {summary}. Run: vega uac"),
        1 => format!(
            "[vega] {summary}; 1 new message. Run: {} inbox, then vega uac",
            cmd()
        ),
        n => format!(
            "[vega] {summary}; {n} new messages. Run: {} inbox, then vega uac",
            cmd()
        ),
    }
}

/// Each local parent's children with attention, and how many children it
/// has. A child on another machine has no status here: only its done
/// report counts.
fn attentions(
    sup: &Supervisor,
    d: &Delivery,
    view: Option<&vega_registry::View>,
) -> HashMap<String, (Attentions, usize, HashMap<String, String>)> {
    let mut out: HashMap<String, (Attentions, usize, HashMap<String, String>)> = HashMap::new();
    let watching = vega_ci_watch::store::waiting(vega_ci_watch::unix_now());
    let here = vega_herdr::hosts::key(&host());
    let mut open_asks: Vec<_> = gather::open()
        .unwrap_or_default()
        .into_iter()
        .map(|(_, a)| a)
        .collect();
    if let Some(view) = view {
        open_asks.extend(
            view.open_asks()
                .unwrap_or_default()
                .into_iter()
                .filter(|(h, _, _, fresh)| *fresh && h != &here)
                .map(|(_, _, a, _)| a),
        );
    }
    let machines = if !open_asks.is_empty() {
        vega_herdr::herdr_on(None, &["machine", "list", "--json"], Duration::from_secs(2))
            .unwrap_or(serde_json::Value::Null)
    } else {
        serde_json::Value::Null
    };
    for c in st::children() {
        let m = c.machine.as_deref();
        let (alive, status) = match m {
            None => (
                sup.alive(&c.pane),
                Some(d.status_of(&c.pane))
                    .filter(|s| *s != "unknown")
                    .map(String::from),
            ),
            // Another machine: its vegad's registry entries, if fresh.
            Some(mach) => match view
                .and_then(|v| v.status(&vega_registry::token(&mach.to_lowercase()), &c.pane))
            {
                Some(Some(s)) => (Some(true), Some(s).filter(|s| s != "unknown")),
                // Missing there: gone only once that host reconciled well
                // after the record was written (its view may lag).
                Some(None) if remote_gone_proven(view, mach, &c.pane) => (Some(false), None),
                Some(None) => (None, None),
                None => (None, None),
            },
        };
        let status = status.as_deref();
        let done = st::done_of(m, &c.pane).map(|x| x.outcome);
        let entry = out.entry(c.parent.pane.clone()).or_default();
        entry.1 += 1;
        let dialog = (m.is_none() && alive == Some(true))
            .then(|| local_dialog(&c.pane, d.status_of(&c.pane)))
            .flatten();
        let silence = st::silence_of(m, &c.pane);
        let park = st::park_of(m, &c.pane);
        let (attention, question) = match dialog {
            Some(x) => asking_dialog(&x),
            None => (
                st::waiting(
                    st::attention(alive, status, done, silence.as_ref(), park.as_ref()),
                    m.is_none() && watching.contains_key(&c.pane),
                ),
                silence.and_then(|s| s.question),
            ),
        };
        let child_host = match m {
            None => Some(here.clone()),
            Some(m) => vega_herdr::hosts::registry_for_machine(m, &machines, &here),
        };
        let owes = supervise::owes_reply(
            &open_asks,
            child_host.as_deref(),
            &c.pane,
            st::known_mode_of(m, &c.pane, &c.name),
        );
        let attention = supervise::unanswered(attention, status, owes);
        let mode = st::mode_of(m, &c.pane, &c.name);
        if let Some(a) = attention.filter(|a| supervise::wakes(*a, mode)) {
            let key = m.map_or(c.pane.clone(), |m| format!("{m}/{}", c.pane));
            if let Some(q) = question.filter(|_| a == st::Attention::Asking) {
                entry.2.insert(key.clone(), q);
            }
            entry.0.insert((key, a));
        }
    }
    out
}

/// `vega post ask`: records the ask, then sends the question to every
/// target on one thread (hop 1, so replies carry `--re <thread>/1@<published-ms>`).
fn ask(to: &[String], within: &str, text: &str) -> Result<(), Failure> {
    let wait = parse_duration(within)?;
    let my_host = host();
    if st::is_inbox_only(&my_host) {
        return Err(Failure::new(
            "inbox_only",
            format!("this host ({my_host}) is inbox-only by design: it cannot ask agents"),
            "ask the user in your pane",
        ));
    }
    let me = me()?;
    let targets: Vec<(String, String)> = to
        .iter()
        .map(|t| match t.rsplit_once('@') {
            Some((pane, h)) => (subject_token(&h.to_lowercase()), pane.to_string()),
            None => (my_host.clone(), t.clone()),
        })
        .filter(|(h, p)| !(h == &my_host && p == &me.pane))
        .collect();
    if targets.is_empty() {
        return Err(Failure::new(
            "usage",
            "no target other than your own pane",
            "vega post ask --to <pane>[@<host>] ... \"<question>\"",
        ));
    }
    if let Some((_, p)) = targets.iter().find(|(_, p)| !valid_pane_id(p)) {
        return Err(Failure::new(
            "usage",
            format!("{p} is not a pane id"),
            "targets are pane ids like w3V:p2, or w3V:p2@ganymede",
        ));
    }
    let thread = nuid::next().to_string();
    // Agents on inbox-only hosts get the question, but their answers go into
    // quarantine (a notice for the human), so the fan-in does not wait.
    let awaited: Vec<String> = targets
        .iter()
        .filter(|(h, _)| !st::is_inbox_only(h))
        .map(|(h, p)| gather::key(h, p))
        .collect();
    if !awaited.is_empty() {
        gather::save(
            &thread,
            &gather::Ask {
                asker: me.pane.clone(),
                targets: awaited.clone(),
                answered: Vec::new(),
                deadline_ms: now_ms() + wait.as_millis(),
                question: supervise::short(text, 600),
                created_ms: now_ms(),
            },
        );
    }
    let body = |to_host: &str| {
        let how = if st::is_inbox_only(to_host) {
            "your host is inbox-only, so the reply command `vega post inbox` prints puts your answer into quarantine: the human sees it, the asker does not wait for it"
        } else {
            "answer once, with the reply command `vega post inbox` prints"
        };
        format!(
            "{text}\n\n(One question to {} agents; {how}.)",
            targets.len()
        )
    };
    let sent = runtime()?.block_on(async {
        let client = connect_patiently().await?;
        let js = jetstream::new(client);
        let mut sent = Vec::new();
        for (to_host, pane) in &targets {
            let mut headers = HeaderMap::new();
            headers.insert("Nats-Msg-Id", nuid::next().as_str());
            headers.insert(h::FROM_PANE, me.pane.as_str());
            headers.insert(h::FROM_AGENT, me.agent.as_str());
            headers.insert(h::FROM_CWD, me.cwd.as_str());
            headers.insert(h::SENT_MS, now_ms().to_string().as_str());
            headers.insert(h::THREAD, thread.as_str());
            headers.insert(h::HOP, "1");
            let subject = inbox_subject(to_host, pane, &my_host);
            let result = match js
                .publish_with_headers(subject, headers, body(to_host).into())
                .await
            {
                Ok(f) => f
                    .await
                    .map(|a| a.sequence.to_string())
                    .map_err(|e| e.to_string()),
                Err(e) => Err(e.to_string()),
            };
            sent.push((to_host.clone(), pane.clone(), result));
        }
        Ok::<_, Failure>(sent)
    })?;
    let failed = sent.iter().filter(|s| s.2.is_err()).count();
    let unawaited = sent.len() - awaited.len();
    if awaited.is_empty() {
        println!(
            "ask: {thread} to {} agents, none awaited (inbox-only hosts)",
            sent.len()
        );
    } else {
        println!(
            "ask: {thread} to {} agents, answers of {} held until all answered or {within}",
            sent.len(),
            awaited.len()
        );
    }
    println!("sent[{}]{{host,pane,result}}:", sent.len());
    for (h, p, r) in &sent {
        let note = if st::is_inbox_only(h) {
            ", not awaited (inbox-only host)"
        } else {
            ""
        };
        match r {
            Ok(id) => println!("  {h},{p},{}", cell(&format!("id {id}{note}"))),
            Err(e) => println!("  {h},{p},{}", cell(&format!("failed: {e}"))),
        }
    }
    if failed > 0 {
        println!(
            "help: {failed} not sent (no inbox on that host?); the ask releases at the deadline"
        );
    }
    if unawaited > 0 {
        println!("help: {unawaited} on inbox-only hosts: their answers reach the human as quarantine notes (`vega post notes`), never your inbox");
    }
    if !awaited.is_empty() {
        println!("help: one pointer arrives when all awaited answered; do not poll");
    }
    Ok(())
}

/// `vega post cmd`: one request to another host's vegad, JSON from stdin.
fn send_cmd(kind: &str, to: &str, timeout: &str) -> Result<(), Failure> {
    if !cmd::valid_kind(kind) {
        return Err(Failure::new(
            "usage",
            format!("{kind} is not a command kind"),
            "kinds are lowercase words, e.g. open",
        ));
    }
    let wait = parse_duration(timeout)?;
    let mut payload = Vec::new();
    use std::io::Read as _;
    std::io::stdin()
        .take(cmd::MAX_PAYLOAD as u64 + 1)
        .read_to_end(&mut payload)
        .map_err(|e| Failure::new("usage", format!("stdin: {e}"), "pipe the JSON request"))?;
    if payload.len() > cmd::MAX_PAYLOAD {
        return Err(Failure::new(
            "usage",
            "request larger than 4 KiB",
            "send less",
        ));
    }
    if serde_json::from_slice::<serde_json::Value>(&payload).is_err() {
        return Err(Failure::new(
            "usage",
            "stdin is not JSON",
            "echo '{\"...\":\"...\"}' | vega post cmd <kind> --to <host>",
        ));
    }
    let to = subject_token(&to.to_lowercase());
    let subject = cmd::subject(&to, kind, &host());
    let started = Instant::now();
    let answer = runtime()?.block_on(async {
        let client = connect("cmd", None).await?;
        let req = async_nats::Request::new()
            .payload(payload.into())
            .timeout(Some(wait));
        client
            .send_request(subject.clone(), req)
            .await
            .map_err(|e| {
                use async_nats::client::RequestErrorKind as K;
                match e.kind() {
                    K::NoResponders => Failure::new(
                        "no_responder",
                        format!("no vegad serves {subject}"),
                        &format!("is vegad running on {to}? (`vega deploy`)"),
                    ),
                    K::TimedOut => Failure::new(
                        "timeout",
                        format!("{to} did not answer within {timeout}"),
                        "retry, or check `vega post relay status` there",
                    ),
                    _ => Failure::new(
                        "denied",
                        format!("the broker refused {subject}: {e}"),
                        "this host may not send this command (titan auth config)",
                    ),
                }
            })
    })?;
    let v: serde_json::Value = serde_json::from_slice(&answer.payload).unwrap_or_default();
    let code = v["code"].as_str().unwrap_or("bad_reply");
    let message = v["message"].as_str().unwrap_or("");
    if v["ok"] != true {
        return Err(Failure::new(
            code,
            format!("{to}: {message}"),
            "see the handler's log on the target host",
        ));
    }
    println!("cmd: {kind} on {to}: ok");
    println!("result: {}", cell(message));
    println!("ms: {}", started.elapsed().as_millis());
    Ok(())
}

/// An answer on an ask's thread to its asker: held (true) until the last
/// target answered.
fn hold_answer(thread: &str, to: &str, from_host: &str, from: &str) -> bool {
    let (mut ask, active) = match gather::load(thread) {
        Some(a) => (a, true),
        None => match gather::load_open(thread) {
            Some(a) => (a, false),
            None => return false,
        },
    };
    match ask.answer(to, &gather::key(from_host, from)) {
        gather::Answer::NotOurs => false,
        gather::Answer::Held { answered, of } => {
            if active {
                gather::save(thread, &ask);
            } else {
                let _ = gather::save_open(thread, &ask);
            }
            log(&format!("ask {thread}: {answered} of {of} answered"));
            active
        }
        gather::Answer::Complete { of } => {
            // Fence a crash between deleting active and durable-open records:
            // either surviving copy now proves every target answered.
            gather::save(thread, &ask);
            gather::remove(thread);
            log(&format!(
                "ask {thread}: all {of} answered, released to {to}"
            ));
            false
        }
    }
}

/// Asks past their deadline: removed; their askers get their answers now.
fn expired_asks() -> Vec<String> {
    let now = now_ms();
    let released = gather::all()
        .into_iter()
        .filter(|(_, a)| now >= a.deadline_ms)
        .filter_map(|(t, a)| {
            if !gather::expire(&t, &a) {
                log(&format!(
                    "ask {t}: cannot preserve unanswered question at deadline"
                ));
                return None;
            }
            log(&format!(
                "ask {t}: deadline, {} of {} answered, released to {}",
                a.answered.len(),
                a.targets.len(),
                a.asker
            ));
            Some(a.asker)
        })
        .collect();
    gather::prune_open(now);
    released
}

/// herdr pane metadata for the human (zero agent tokens): sentinels show
/// their spawn name, task or report, `$attention` and `$parent`; the UAC
/// pane shows VEGA; a pane with unread mail gets `$mail`. Renders where the
/// Slayer's sidebar rows name the tokens (docs/design/vegad.md, Sidebar).
fn local_dialog(pane: &str, status: &str) -> Option<vega_herdr::dialog::Dialog> {
    if !matches!(status, "blocked" | "unknown") {
        return None;
    }
    let raw = vega_herdr::herdr_text(&["pane", "read", pane, "--source", "visible"]).ok()?;
    vega_herdr::dialog::classify(&raw, "", status)
}

/// A visible dialog: a question dialog asks (its question), any other
/// needs input.
fn asking_dialog(d: &vega_herdr::dialog::Dialog) -> (Option<st::Attention>, Option<String>) {
    if d.kind == vega_herdr::dialog::Kind::Question {
        (Some(st::Attention::Asking), Some(d.title.clone()))
    } else {
        (Some(st::Attention::Input), None)
    }
}

/// Unread notes of every host for this host's viewer: this host's store,
/// plus the other hosts' unread notes the registry read fetched.
fn unread_notes(view: Option<&vega_registry::View>) -> Vec<vega_herdr::notes::Note> {
    let Some(store) = vega_herdr::notes::Store::local() else {
        return Vec::new();
    };
    let remote = view.map(|v| v.unread_notes()).unwrap_or_default();
    let own = vega_registry::token(&st::post_host());
    store.seen().unread_of(&store.list(), &remote, &own)
}

/// The vega UAC's project among `keys`: the one of the checkout vega was
/// installed from (install.json). Its badge counts the host-level notes.
fn vega_project(keys: &[String]) -> Option<String> {
    let install = vega_onboard::upgrade::read_install(&service::state_dir().ok()?)?.ok()?;
    let project = match vega_herdr::project::resolve(&install.checkout, &st::post_host()) {
        Ok(project) => {
            vega_herdr::project::diagnostic("home project", &install.checkout, None);
            project
        }
        Err(e) => {
            if let Some(message) =
                vega_herdr::project::diagnostic("home project", &install.checkout, Some(&e))
            {
                log(&format!("home project: {message}"));
            }
            return None;
        }
    };
    keys.iter()
        .find(|k| k.as_str() == project.id.as_str())
        .cloned()
}

/// How long vegad trusts a project's decision count while its beads do not
/// change (the Dolt journal); a write refreshes it at once.
const DECISIONS_EVERY: Duration = Duration::from_secs(60);

/// Open decisions per project key: (journal mtime, when listed, decisions).
#[derive(Default)]
struct DecisionCache(
    HashMap<(String, std::path::PathBuf), (u128, Instant, Vec<vega_herdr::badges::Decision>)>,
);

impl DecisionCache {
    /// The project's open decisions (vega_herdr::badges::decisions),
    /// from bd in the checkout `vega bd init` provisioned, else in the project
    /// dir with a provisioned project's bd (vegad's PATH has none). A failed
    /// run keeps the last list.
    fn list(
        &mut self,
        key: &str,
        projects: &[vega_bd_sync::Project],
    ) -> Option<Vec<vega_herdr::badges::Decision>> {
        let location = st::project_uac_location(key).or_else(|| {
            let candidates: Vec<_> = projects.iter().filter(|p| p.key == key).collect();
            match candidates.as_slice() {
                [p] => Some(vega_herdr::project::RepoLocation {
                    common_dir: p.common.clone(),
                    checkout: p.dir.clone(),
                }),
                _ => None,
            }
        });
        let location = location?;
        let cache_key = (key.to_owned(), location.common_dir.clone());
        let current = match probes::resolve(&location.checkout, &st::post_host()) {
            Ok(current)
                if current.id.as_str() == key
                    && current.location.common_dir == location.common_dir =>
            {
                current
            }
            result => {
                let error = match result {
                    Err(e) => e,
                    Ok(current) => {
                        vega_herdr::project::identity_changed("Decision location", key, &current)
                    }
                };
                if let Some(message) = vega_herdr::project::registered_diagnostic(
                    "decisions",
                    key,
                    &location.checkout,
                    Some(&error),
                ) {
                    log(&format!("decisions: {message}"));
                }
                return self.0.get(&cache_key).map(|entry| entry.2.clone());
            }
        };
        vega_herdr::project::registered_diagnostic("decisions", key, &location.checkout, None);
        let p = projects
            .iter()
            .find(|p| p.key == key && p.common == current.location.common_dir);
        let mtime = p.map_or(0, |p| vega_bd_sync::sync::newest_mtime(&p.noms()));
        let last = self.0.get(&cache_key).cloned();
        if let Some((m, at, n)) = &last {
            if *m == mtime && at.elapsed() < DECISIONS_EVERY {
                return Some(n.clone());
            }
        }
        let Some(bd) = p.or(projects.first()).map(|p| p.bd.clone()) else {
            return last.map(|entry| entry.2);
        };
        let dir = current.location.checkout;
        let out = vega_herdr::output_within(
            std::process::Command::new(bd)
                .args([
                    "list",
                    "--label-any",
                    &format!(
                        "{},{}",
                        vega_herdr::badges::TRACKER_LABEL,
                        vega_herdr::badges::DECISION_LABEL
                    ),
                    "--json",
                    "--limit",
                    "0",
                ])
                .current_dir(dir),
            PARK_BD_TIMEOUT,
        );
        let n = match out {
            Ok(Some(o)) if o.status.success() => vega_herdr::badges::decisions(
                &vega_plugin::unwrap_bd_envelope(&String::from_utf8_lossy(&o.stdout)),
            ),
            _ => None,
        };
        let n = n.or(last.map(|l| l.2))?;
        self.0.insert(cache_key, (mtime, Instant::now(), n.clone()));
        Some(n)
    }
}

/// Missing decision data must not overwrite the last saved count. Warm caches
/// retain their last list; a cold failure refreshes the last saved count.
fn save_project_badges(
    key: &str,
    notes: usize,
    waiting: &[vega_herdr::badges::Waiting],
    decisions: Option<&[vega_herdr::badges::Decision]>,
    now: i64,
) -> vega_herdr::badges::Badges {
    let previous = vega_herdr::badges::retained(key).map(|entry| entry.0);
    let b = vega_herdr::badges::Badges {
        notes,
        waiting: waiting.len(),
        decisions: decisions.map_or_else(|| previous.map_or(0, |b| b.decisions), <[_]>::len),
    };
    if decisions.is_some() || previous.is_some() {
        vega_herdr::badges::save(key, &b, waiting, now);
    }
    b
}

/// Logs what theme.toml's `[symbols]` got wrong, once per change.
fn log_theme_errors(errors: &[String]) {
    static LAST: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
    let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
    if *last != errors {
        for e in errors {
            log(e);
        }
        *last = errors.to_vec();
    }
}

/// herdr's panes, workspaces and tabs on this host, for project membership
/// and the sidebar's `$project` and `$label`.
struct Layout {
    panes: Vec<serde_json::Value>,
    workspaces: Vec<serde_json::Value>,
    tabs: Vec<serde_json::Value>,
}

impl Layout {
    fn read() -> Layout {
        Layout::read_herdr().resolve_projects()
    }

    /// herdr's lists alone, without project resolution (identity quiesce).
    fn read_herdr() -> Layout {
        let list = |args: &[&str], k: &str| {
            herdr(args)
                .ok()
                .and_then(|v| v["result"][k].as_array().cloned())
                .unwrap_or_default()
        };
        Layout {
            panes: list(&["pane", "list"], "panes"),
            workspaces: list(&["workspace", "list"], "workspaces"),
            tabs: list(&["tab", "list"], "tabs"),
        }
    }

    /// Only this host's herdr snapshot is inspected. Never resolve a registry
    /// entry's path. Resolve once per snapshot, before repeated ID joins.
    fn resolve_projects(mut self) -> Self {
        let mut catalog = match vega_herdr::project::LocationCatalog::new(&st::post_host()) {
            Ok(catalog) => catalog,
            Err(e) => {
                log(&e.message);
                return self;
            }
        };
        let host = st::post_host();
        let mut paths = HashMap::new();
        let mut resolve = |workspace: Option<&serde_json::Value>, cwd: &str| {
            let path = vega_herdr::project::metadata_path(workspace, cwd);
            if path.as_os_str().is_empty() {
                return None;
            }
            paths
                .entry(path.clone())
                .or_insert_with(|| {
                    match probes::resolve(&path, &host).and_then(|p| catalog.add(p)) {
                        Ok(p) => {
                            vega_herdr::project::diagnostic("layout", &path, None);
                            Some(p.id.to_string())
                        }
                        Err(e) => {
                            if let Some(message) =
                                vega_herdr::project::diagnostic("layout", &path, Some(&e))
                            {
                                log(&format!("project: {message}"));
                            }
                            None
                        }
                    }
                })
                .clone()
        };
        for p in &mut self.panes {
            let ws = self
                .workspaces
                .iter()
                .find(|w| w["workspace_id"] == p["workspace_id"]);
            let id = resolve(ws, p["cwd"].as_str().unwrap_or_default());
            p["schema"] = 2.into();
            p["project"] = id.into();
        }
        for w in &mut self.workspaces {
            let id = resolve(Some(w), "");
            w["schema"] = 2.into();
            w["project"] = id.into();
        }
        self
    }

    /// The label of pane `id`'s tab, if herdr names one.
    fn tab_label(&self, id: &str) -> Option<&str> {
        let tab = &self.pane(id)?["tab_id"];
        self.tabs
            .iter()
            .find(|t| t["tab_id"] == *tab)
            .and_then(|t| t["label"].as_str())
            .filter(|l| !l.is_empty())
    }

    /// Herdr's agent kind of pane `id` (`claude`, `pi`, ...), if any.
    fn agent_kind(&self, id: &str) -> Option<&str> {
        self.pane(id)
            .and_then(|p| p["agent"].as_str())
            .filter(|k| !k.is_empty())
    }

    /// The `$kind` token args for pane `id` (first in the sidebar's second
    /// row, on every agent pane): its agent kind's `Symbols::kind`, the
    /// symbol (a Nerd Font glyph with a blank that keeps it one cell);
    /// cleared without a kind or for an empty override.
    fn kind_token(&self, sym: &vega_herdr::theme::Symbols, id: &str) -> [String; 2] {
        match self
            .agent_kind(id)
            .map(|k| sym.kind(vega_herdr::theme::agent_topic(k)))
            .filter(|g| !g.is_empty())
        {
            Some(g) => ["--token".into(), format!("kind={g}")],
            None => ["--clear-token".into(), "kind".into()],
        }
    }

    /// Distinct display kinds in this host's workspace, UAC then sentinel first.
    /// Pane IDs break ties, so list reordering does not shuffle the sidebar.
    fn workspace_kinds(
        &self,
        ws: &str,
        uacs: &[String],
        sentinels: &HashMap<String, String>,
        sym: &vega_herdr::theme::Symbols,
    ) -> Vec<String> {
        let mut panes: Vec<_> = self
            .panes
            .iter()
            .filter_map(|p| {
                let id = p["pane_id"].as_str()?;
                let kind = p["agent"].as_str().filter(|k| !k.is_empty())?;
                (p["workspace_id"].as_str() == Some(ws)).then_some((id, kind))
            })
            .collect();
        panes.sort_by_key(|(id, _)| {
            (
                if uacs.iter().any(|p| p == id) {
                    0
                } else if sentinels.contains_key(*id) {
                    1
                } else {
                    2
                },
                *id,
            )
        });
        let mut seen = Vec::new();
        let mut kinds = Vec::new();
        for (_, kind) in panes {
            let topic = vega_herdr::theme::agent_topic(kind);
            if seen.contains(&topic) {
                continue;
            }
            seen.push(topic);
            let glyph = sym.space_kind(topic).to_string();
            if !glyph.is_empty() {
                kinds.push(glyph);
            }
        }
        kinds
    }

    /// The `$label` token args for pane `id` (the sidebar's second row,
    /// after `$kind`): cleared on a UAC pane; a sentinel's symbol and tab
    /// (else its name); any other agent's tab (else its kind).
    fn label_token(
        &self,
        sym: &vega_herdr::theme::Symbols,
        id: &str,
        sentinel: Option<&str>,
        uac: bool,
    ) -> [String; 2] {
        let tab = self.tab_label(id);
        let label = match sentinel {
            _ if uac => None,
            Some(name) => Some(sentinel_label(sym, tab.unwrap_or(name))),
            None => self.agent_kind(id).map(|k| tab.unwrap_or(k).to_string()),
        };
        match label {
            Some(l) => ["--token".into(), format!("label={l}")],
            None => ["--clear-token".into(), "label".into()],
        }
    }

    fn pane(&self, id: &str) -> Option<&serde_json::Value> {
        self.panes.iter().find(|p| p["pane_id"] == id)
    }

    /// The label of pane `id`'s workspace, if herdr names one.
    fn workspace_label(&self, id: &str) -> Option<&str> {
        let ws = &self.pane(id)?["workspace_id"];
        self.workspaces
            .iter()
            .find(|w| w["workspace_id"] == *ws)
            .and_then(|w| w["label"].as_str())
            .filter(|l| !l.is_empty())
    }

    /// The `$project` token args for pane `id` (the sidebar's first row):
    /// the workspace of its local `parent` pane (a sentinel in a worktree
    /// workspace shows its project), else its own workspace.
    fn project_token(&self, id: &str, parent: Option<&str>) -> [String; 2] {
        match parent
            .and_then(|p| self.workspace_label(p))
            .or_else(|| self.workspace_label(id))
        {
            Some(l) => ["--token".into(), format!("project={l}")],
            None => ["--clear-token".into(), "project".into()],
        }
    }

    /// Whether pane `id` works on project `key` (as `vega uac` decides it).
    fn in_project(&self, key: &str, id: &str) -> bool {
        self.pane(id)
            .is_some_and(|p| vega_herdr::in_project(key, p))
    }

    /// The workspaces of project `key`: its repo's, or holding a pane that
    /// works on it; never a linked worktree's (a sentinel's), so the counts
    /// show once, on the main checkout.
    fn project_workspaces(&self, key: &str) -> Vec<String> {
        self.workspaces
            .iter()
            .filter(|w| w["worktree"]["is_linked_worktree"] != true)
            .filter(|w| {
                vega_herdr::in_project(key, w)
                    || self.panes.iter().any(|p| {
                        p["workspace_id"] == w["workspace_id"]
                            && p["pane_id"]
                                .as_str()
                                .is_some_and(|id| self.in_project(key, id))
                    })
            })
            .filter_map(|w| w["workspace_id"].as_str().map(String::from))
            .collect()
    }

    /// The board panes (by the terminal title the board sets) and their
    /// project among `keys`, else the project of their workspace's repo.
    fn boards(&self, keys: &[String]) -> Vec<(String, Option<String>)> {
        self.panes
            .iter()
            .filter(|p| {
                p["terminal_title"]
                    .as_str()
                    .is_some_and(|t| t.starts_with("vega-board"))
            })
            .filter_map(|p| {
                let id = p["pane_id"].as_str()?;
                let key = keys
                    .iter()
                    .find(|k| self.in_project(k, id))
                    .cloned()
                    .or_else(|| {
                        self.workspaces
                            .iter()
                            .find(|w| w["workspace_id"] == p["workspace_id"])
                            .and_then(|w| w["project"].as_str())
                            .map(str::to_owned)
                    });
                Some((id.to_string(), key))
            })
            .collect()
    }
}

/// A sentinel's sidebar name: the `sentinel` symbol, then its name.
fn sentinel_label(sym: &vega_herdr::theme::Symbols, name: &str) -> String {
    format!("{}{name}", sym.icon(&vega_herdr::theme::SENTINEL))
}

/// The `$host` token args: theme.toml's `[symbols.hosts]` label, else the
/// bare host name.
fn host_token(sym: &vega_herdr::theme::Symbols, host: &str) -> [String; 2] {
    ["--token".into(), format!("host={}", sym.host_label(host))]
}

/// The tokens every pane gets while vegad is in identity quiesce: `$host`,
/// `$kind`, `$project` (its workspace label) and `$label`, all from herdr's
/// layout alone, so the sidebar still names the host's agents. UAC badges,
/// sentinel names, `$git` and the workspace tokens wait for the identity.
fn quiesced_metadata(
    layout: &Layout,
    sym: &vega_herdr::theme::Symbols,
    host: &str,
) -> Vec<(String, Vec<String>)> {
    layout
        .panes
        .iter()
        .filter_map(|p| p["pane_id"].as_str().filter(|id| valid_pane_id(id)))
        .map(|id| {
            let mut args = host_token(sym, host).to_vec();
            args.extend(layout.kind_token(sym, id));
            args.extend(layout.project_token(id, None));
            args.extend(layout.label_token(sym, id, None, false));
            (id.to_string(), args)
        })
        .collect()
}

/// Reports `quiesced_metadata` to this host's herdr.
fn report_quiesced() {
    let sym = vega_herdr::theme::symbols();
    let mut cmds = Vec::new();
    for (pane, args) in quiesced_metadata(&Layout::read_herdr(), sym.as_ref(), &st::post_host()) {
        let mut cmd: Vec<String> = vec![
            "pane".into(),
            "report-metadata".into(),
            pane.clone(),
            "--source".into(),
            "vega:vegad".into(),
            "--ttl-ms".into(),
            sidebar::TTL_MS.into(),
        ];
        cmd.extend(args);
        cmds.push(cmd);
    }
    sidebar::submit(cmds);
}

/// `herdr workspace report-metadata` for workspace `ws`'s count tokens
/// (`$decisions`, `$waiting`, `$notes`, as on the UAC pane) and its
/// `$badge`: what works there (`u▲c`, the sentinel symbol), and its `$git`
/// state (`git_token`), plus ordered `$kind1`..`$kind5` glyphs; cleared when absent.
fn workspace_cmd(
    ws: &str,
    b: &vega_herdr::badges::Badges,
    badge: Option<&str>,
    git: Option<&str>,
    kinds: &[String],
    sym: &vega_herdr::theme::Symbols,
) -> Vec<String> {
    let mut cmd: Vec<String> = vec![
        "workspace".into(),
        "report-metadata".into(),
        ws.into(),
        "--source".into(),
        "vega:vegad".into(),
        "--ttl-ms".into(),
        sidebar::TTL_MS.into(),
    ];
    cmd.extend(b.count_tokens(sym));
    match badge.filter(|x| !x.is_empty()) {
        Some(x) => cmd.extend(["--token".into(), format!("badge={x}")]),
        None => cmd.extend(["--clear-token".into(), "badge".into()]),
    }
    match git {
        Some(x) => cmd.extend(["--token".into(), format!("git={x}")]),
        None => cmd.extend(["--clear-token".into(), "git".into()]),
    }
    // Herdr colours whole tokens. Clear unused slots when agents exit.
    for slot in 0..5 {
        let name = format!("kind{}", slot + 1);
        match kinds.get(slot) {
            Some(g) => cmd.extend(["--token".into(), format!("{name}={g}")]),
            None => cmd.extend(["--clear-token".into(), name]),
        }
    }
    cmd
}

/// `git status` of one checkout; well under a second even in big repos.
const GIT_STATUS_TIMEOUT: Duration = Duration::from_secs(3);

/// The `$git` token of the checkout at `path`, from `git status --porcelain=v2
/// --branch` (local refs only: vegad never fetches, like herdr's git_status).
/// `None` when git fails or times out.
fn git_state(path: &str) -> Option<String> {
    let out = vega_herdr::output_within(
        std::process::Command::new("git")
            // Never take index.lock: agents commit in these checkouts.
            .args(["--no-optional-locks", "-C", path])
            .args(["status", "--porcelain=v2", "--branch"]),
        GIT_STATUS_TIMEOUT,
    )
    .ok()??;
    out.status
        .success()
        .then(|| git_token(&String::from_utf8_lossy(&out.stdout)))
}

/// `✓` when the checkout is clean and level with its upstream, else the
/// parts that differ: `↑<ahead>`, `↓<behind>`, `●` for local changes
/// (untracked files included). No upstream counts as level.
fn git_token(porcelain: &str) -> String {
    let (mut ahead, mut behind, mut dirty) = (0u64, 0u64, false);
    for line in porcelain.lines() {
        if let Some(ab) = line.strip_prefix("# branch.ab ") {
            let mut it = ab.split(' ');
            ahead = it
                .next()
                .and_then(|x| x.trim_start_matches('+').parse().ok())
                .unwrap_or(0);
            behind = it
                .next()
                .and_then(|x| x.trim_start_matches('-').parse().ok())
                .unwrap_or(0);
        } else if !line.starts_with('#') && !line.is_empty() {
            dirty = true;
        }
    }
    let mut parts = Vec::new();
    if ahead > 0 {
        parts.push(format!("\u{2191}{ahead}"));
    }
    if behind > 0 {
        parts.push(format!("\u{2193}{behind}"));
    }
    if dirty {
        parts.push("\u{25cf}".to_string());
    }
    if parts.is_empty() {
        "\u{2713}".to_string()
    } else {
        parts.join(" ")
    }
}

/// A workspace's `$badge`: `u▲c` when it holds a UAC pane, else the
/// sentinel symbol when it holds a sentinel.
fn workspace_badge(sym: &vega_herdr::theme::Symbols, uac: bool, sentinel: bool) -> Option<String> {
    if uac {
        Some(sym.uac_name())
    } else if sentinel {
        Some(
            sym.icon(&vega_herdr::theme::SENTINEL)
                .trim_end()
                .to_string(),
        )
    } else {
        None
    }
}

/// Reports the sidebar metadata; returns the hygiene mail due (hygiene.rs)
/// as tool mail.
fn report_metadata(
    sup: &Supervisor,
    d: &Delivery,
    view: Option<&vega_registry::View>,
    decisions: &mut DecisionCache,
    hygiene: &mut hygiene::Pass,
) -> Vec<ToolMail> {
    // Every herdr report-metadata command of this refresh, for sidebar.rs.
    let mut cmds: Vec<Vec<String>> = Vec::new();
    let mut phases = Phases::new();
    let mut panes: HashMap<String, Vec<String>> = HashMap::new();
    // Local sentinels by pane, for `$label`.
    let mut sentinels: HashMap<String, String> = HashMap::new();
    // Their parent panes on this host, for `$project`.
    let mut parents: HashMap<String, String> = HashMap::new();
    let here = host();
    // Headed sentinels that wait on the Slayer (`vega uac`: for the user).
    let mut for_user: Vec<vega_herdr::badges::Waiting> = Vec::new();
    let watching = vega_ci_watch::store::waiting(vega_ci_watch::unix_now());
    // ~/.config/vega/theme.toml, re-read here: a change shows on the next refresh.
    let sym = vega_herdr::theme::symbols();
    let sym = sym.as_ref();
    for c in st::children().into_iter().filter(|c| c.machine.is_none()) {
        sentinels.insert(c.pane.clone(), c.name.clone());
        if subject_token(&c.parent.host.to_lowercase()) == here {
            parents.insert(c.pane.clone(), c.parent.pane.clone());
        }
        let done = st::done_of(None, &c.pane);
        let status = Some(d.status_of(&c.pane)).filter(|s| *s != "unknown");
        let dialog = (sup.alive(&c.pane) == Some(true))
            .then(|| local_dialog(&c.pane, d.status_of(&c.pane)))
            .flatten();
        let silence = st::silence_of(None, &c.pane);
        let attention = match &dialog {
            Some(x) => asking_dialog(x).0,
            None => st::waiting(
                st::attention(
                    sup.alive(&c.pane),
                    status,
                    done.as_ref().map(|x| x.outcome),
                    silence.as_ref(),
                    st::park_of(None, &c.pane).as_ref(),
                ),
                watching.contains_key(&c.pane),
            ),
        };
        let ci = watching
            .get(&c.pane)
            .filter(|_| attention == Some(st::Attention::Waiting))
            .map(|t| format!("waits on ci: {}", t.join(", ")));
        let title = dialog
            .map(|x| format!("{}: {}", x.kind.as_str(), x.title))
            .or(ci)
            .or_else(|| {
                silence
                    .and_then(|s| s.question)
                    .map(|q| format!("asks: {q}"))
            })
            .or_else(|| done.map(|x| x.summary).or_else(|| st::saved_task(&c.pane)))
            .unwrap_or_default();
        let mut args = vec!["--display-agent".into(), sentinel_label(sym, &c.name)];
        if !title.is_empty() {
            args.extend(["--title".into(), title.chars().take(80).collect()]);
        }
        args.extend([
            "--token".into(),
            format!("parent=\u{2191} {}", c.parent.pane),
        ]);
        if let Some(a) = attention.filter(|a| st::for_user(*a, st::mode_of(None, &c.pane, &c.name)))
        {
            for_user.push(vega_herdr::badges::Waiting {
                pane: c.pane.clone(),
                name: c.name.clone(),
                attention: a.as_str().to_string(),
                title: title.chars().take(80).collect(),
            });
        }
        match attention {
            Some(a) => args.extend(["--token".into(), format!("attention={}", a.as_str())]),
            None => args.extend(["--clear-token".into(), "attention".into()]),
        }
        panes.insert(c.pane.clone(), args);
    }
    phases.mark("sentinels");
    let unread = unread_notes(view);
    let uacs = st::project_uac_records();
    let uac_keys: Vec<String> = uacs.iter().map(|(k, _, _)| k.clone()).collect();
    let home = vega_project(&uac_keys);
    let notes_on = |key: &str| {
        unread
            .iter()
            .filter(|n| n.counts_on(key, home.as_deref(), &uac_keys))
            .count()
    };
    let live: Vec<String> = d.panes();
    phases.mark("notes");
    let layout = Layout::read();
    phases.mark("layout");
    log_theme_errors(&sym.errors);
    let projects = vega_bd_sync::store::projects();
    let now = vega_ci_watch::unix_now();
    let mut badges_of = |key: &str| {
        let waiting: Vec<vega_herdr::badges::Waiting> = for_user
            .iter()
            .filter(|w| layout.in_project(key, &w.pane))
            .cloned()
            .collect();
        let open = decisions.list(key, &projects);
        let b = save_project_badges(key, notes_on(key), &waiting, open.as_deref(), now);
        (b, waiting, open)
    };
    // Each project workspace's count tokens (herdr Spaces); the first project wins.
    let mut inboxes: Vec<(String, vega_herdr::badges::Badges)> = Vec::new();
    let mut tidy: Vec<hygiene::Project> = Vec::new();
    for (key, pane, _) in &uacs {
        let (b, waiting, open) = badges_of(key);
        tidy.push(hygiene::Project {
            key: key.clone(),
            uac: live.contains(pane).then(|| pane.clone()),
            waiting,
            decisions: open.unwrap_or_default(),
            mail: live
                .iter()
                .filter(|p| layout.in_project(key, p))
                .filter_map(|p| Some((p.clone(), (d.unread_since(p)? / 1000) as i64)))
                .collect(),
            notes: unread
                .iter()
                .filter(|n| n.counts_on(key, home.as_deref(), &uac_keys))
                .map(|n| (format!("{}={}", n.host, n.id), n.at(), n.title.clone()))
                .collect(),
        });
        for ws in layout.project_workspaces(key) {
            if !inboxes.iter().any(|(w, _)| *w == ws) {
                inboxes.push((ws, b));
            }
        }
        let args = panes.entry(pane.clone()).or_default();
        args.extend([
            "--display-agent".into(),
            b.uac_label(sym),
            "--token".into(),
            "role=uac".into(),
        ]);
        args.extend(b.uac_tokens(sym));
    }
    phases.mark("uac badges");
    // Without a UAC pane on this host, the open board panes carry the badges.
    if !uacs.iter().any(|(_, p, _)| live.contains(p)) {
        for (pane, key) in layout.boards(&uac_keys) {
            let b = match key {
                Some(k) => badges_of(&k).0,
                None => vega_herdr::badges::Badges {
                    notes: unread.len(),
                    ..Default::default()
                },
            };
            let mut args = vec![
                "pane".to_string(),
                "report-metadata".into(),
                pane,
                "--source".into(),
                "vega:vegad".into(),
                "--ttl-ms".into(),
                sidebar::TTL_MS.into(),
            ];
            if b.is_zero() {
                args.push("--clear-display-agent".into());
            } else {
                args.extend(["--display-agent".into(), b.after("vega board", sym)]);
            }
            cmds.push(args);
        }
    }
    // Each workspace holding a UAC or a local sentinel gets its `$badge`.
    let mut badges: HashMap<String, String> = HashMap::new();
    for pane in &live {
        let uac = uacs.iter().any(|(_, p, _)| p == pane);
        let Some(ws) = layout.pane(pane).and_then(|p| p["workspace_id"].as_str()) else {
            continue;
        };
        if let Some(x) = workspace_badge(sym, uac, sentinels.contains_key(pane)) {
            if uac || !badges.contains_key(ws) {
                badges.insert(ws.to_string(), x);
            }
        }
    }
    // Every workspace with a checkout and every agent pane gets its `$git`,
    // as probes.rs last read it off the loop.
    let git_of = probes::git;
    let mut gits: HashMap<String, String> = HashMap::new();
    for w in &layout.workspaces {
        let (Some(ws), Some(path)) = (
            w["workspace_id"].as_str(),
            w["worktree"]["checkout_path"].as_str(),
        ) else {
            continue;
        };
        if let Some(g) = git_of(path) {
            gits.insert(ws.to_string(), g);
        }
    }
    // Include agent-only and now-empty workspaces, to clear stale kinds too.
    for ws in layout
        .workspaces
        .iter()
        .filter_map(|w| w["workspace_id"].as_str())
    {
        if !inboxes.iter().any(|(w, _)| w == ws) {
            inboxes.push((ws.to_string(), Default::default()));
        }
    }
    let uac_panes: Vec<String> = uacs.iter().map(|(_, p, _)| p.clone()).collect();
    for (ws, b) in &inboxes {
        let kinds = layout.workspace_kinds(ws, &uac_panes, &sentinels, sym);
        let cmd = workspace_cmd(
            ws,
            b,
            badges.get(ws).map(String::as_str),
            gits.get(ws).map(String::as_str),
            &kinds,
            sym,
        );
        cmds.push(cmd);
    }
    for pane in live.iter().filter(|p| d.has_mail(p)) {
        panes
            .entry(pane.clone())
            .or_default()
            .extend(["--token".into(), "mail=\u{2709}".into()]);
        // For `vega uac` (unread mail age); the sidebar does not name it.
        if let Some(t) = d.unread_since(pane) {
            panes
                .entry(pane.clone())
                .or_default()
                .extend(["--token".into(), format!("mail_since={t}")]);
        }
    }
    // Every agent pane on this herdr names its host, project and what it
    // is; remote panes get theirs from their own vegad.
    let host = st::post_host();
    for pane in &live {
        let uac = uacs.iter().any(|(_, p, _)| p == pane);
        let label = layout.label_token(sym, pane, sentinels.get(pane).map(String::as_str), uac);
        let args = panes.entry(pane.clone()).or_default();
        args.extend(host_token(sym, &host));
        args.extend(layout.kind_token(sym, pane));
        args.extend(layout.project_token(pane, parents.get(pane).map(String::as_str)));
        args.extend(label);
        let cwd = layout.pane(pane).and_then(|p| p["cwd"].as_str());
        match cwd.and_then(git_of) {
            Some(g) => args.extend(["--token".into(), format!("git={g}")]),
            None => args.extend(["--clear-token".into(), "git".into()]),
        }
    }
    for (pane, mut args) in panes {
        if !valid_pane_id(&pane) || !live.contains(&pane) {
            continue;
        }
        if !d.has_mail(&pane) {
            args.extend(["--clear-token".into(), "mail".into()]);
        }
        if d.unread_since(&pane).is_none() {
            args.extend(["--clear-token".into(), "mail_since".into()]);
        }
        let mut cmd: Vec<String> = vec![
            "pane".into(),
            "report-metadata".into(),
            pane.clone(),
            "--source".into(),
            "vega:vegad".into(),
            "--ttl-ms".into(),
            sidebar::TTL_MS.into(),
        ];
        cmd.extend(args);
        cmds.push(cmd);
    }
    // Sent off the loop, only what changed (sidebar.rs).
    sidebar::submit(cmds);
    phases.mark("panes");
    let due = hygiene
        .run(now, &tidy)
        .into_iter()
        .map(|(pane, text)| {
            log(&format!("hygiene: mail for {pane}"));
            (hygiene::AGENT.to_string(), pane, text)
        })
        .collect();
    phases.mark("hygiene");
    phases.log_if_slow("metadata");
    due
}

/// Where a slow step spent its time: `slow: <step> phases: a 12 ms, ...`.
struct Phases {
    started: Instant,
    last: Instant,
    took: Vec<(&'static str, Duration)>,
}

impl Phases {
    fn new() -> Self {
        let now = Instant::now();
        Phases {
            started: now,
            last: now,
            took: Vec::new(),
        }
    }

    /// Ends phase `name`, begun at the last mark.
    fn mark(&mut self, name: &'static str) {
        let now = Instant::now();
        self.took.push((name, now - self.last));
        self.last = now;
    }

    fn log_if_slow(&self, step: &str) {
        if self.started.elapsed() > SLOW_STEP {
            let parts: Vec<String> = self
                .took
                .iter()
                .map(|(n, d)| format!("{n} {} ms", d.as_millis()))
                .collect();
            log(&format!("slow: {step} phases: {}", parts.join(", ")));
        }
    }
}

/// Logs a step of vegad's loop that took longer than SLOW_STEP: the sidebar
/// rows are renewed by sidebar.rs, but a slow loop delays every change.
fn slow(step: &str, since: Instant) {
    let took = since.elapsed();
    if took > SLOW_STEP {
        log(&format!("slow: {step} took {} ms", took.as_millis()));
    }
}

const SLOW_STEP: Duration = Duration::from_secs(5);

/// `bd show` of one parked bead; bd answers in well under a second.
const PARK_BD_TIMEOUT: Duration = Duration::from_secs(5);

/// Parked children whose decision bead closed get `closed`: their attention
/// turns `decided`, which wakes the parent once. One `bd show` per open
/// park, on the metadata clock; nothing runs without parks. A bead bd cannot
/// show proves nothing: the park stays (logged once).
fn check_parks(failing: &mut HashSet<String>) {
    for (m, pane, mut p) in st::parks().into_iter().filter(|(_, _, p)| !p.closed) {
        let key = m.as_ref().map_or(pane.clone(), |m| format!("{m}/{pane}"));
        let out = vega_herdr::output_within(
            std::process::Command::new(&p.bd)
                .args(["show", &p.bead, "--json"])
                .current_dir(&p.dir),
            PARK_BD_TIMEOUT,
        );
        let closed = match out {
            Ok(Some(o)) if o.status.success() => bead_closed(&String::from_utf8_lossy(&o.stdout)),
            _ => None,
        };
        match closed {
            Some(true) => {
                p.closed = true;
                st::save_park(m.as_deref(), &pane, &p);
                failing.remove(&key);
                log(&format!(
                    "park: {key} on {} closed, parent to resume it",
                    p.bead
                ));
            }
            Some(false) => {
                failing.remove(&key);
            }
            None => {
                if failing.insert(key.clone()) {
                    log(&format!(
                        "park: {key}: {} show {} failed in {}; stays parked",
                        p.bd, p.bead, p.dir
                    ));
                }
            }
        }
    }
}

/// Whether `bd show <id> --json` reports the bead closed; `None` if the
/// output is no bead.
fn bead_closed(json: &str) -> Option<bool> {
    let v: serde_json::Value = serde_json::from_str(&vega_plugin::unwrap_bd_envelope(json)).ok()?;
    let b = v.as_array().and_then(|a| a.first()).unwrap_or(&v);
    Some(b["status"].as_str()? == "closed")
}

/// How often vegad reconciles into the registry; slower after a failure.
const REGISTRY_EVERY: Duration = Duration::from_secs(15);
const REGISTRY_RETRY: Duration = Duration::from_secs(120);
/// A waiting-ledger change reconciles this soon; it batches a burst of
/// focus events (a walk through the tabs).
const WAITING_DELAY: Duration = Duration::from_secs(1);

/// vegad's side of the registry (crates/vega-registry): local state is the
/// truth; the bucket is the aggregate. Without the bucket or the broker
/// right, mail and local supervision go on and the state says why.
#[derive(Default)]
struct Registry {
    kv: Option<async_nats::jetstream::kv::Store>,
    last: std::collections::BTreeMap<String, serde_json::Value>,
    view: Option<vega_registry::View>,
    /// Finished turns last seen per remote child (`<machine>/<pane>`).
    turns: HashMap<String, u64>,
    next: Option<Instant>,
    state: String,
    /// When the other hosts' janitor summaries are pulled next.
    janitor_next: Option<Instant>,
    /// `vega gate gc` shares the janitor's five-minute cadence. A failed
    /// feature job is never allowed to take vegad down with it.
    gate_gc: GateGc,
}

enum RegistryUpdate {
    Synced(usize),
    IdentityBlocked(String),
}

/// Keeps a best-effort janitor feature on its cadence. A failure is reported
/// once until a later successful run proves it recovered.
#[derive(Default)]
struct GateGc {
    next: Option<Instant>,
    failed: bool,
}

impl GateGc {
    fn run(&mut self, job: impl FnOnce() -> Result<(), String>) {
        if self.next.is_some_and(|t| Instant::now() < t) {
            return;
        }
        // Set this before running the feature: even a broken or slow feature
        // cannot turn every relay-loop tick into a retry.
        self.next = Some(Instant::now() + janitor::PULL_EVERY);
        match job() {
            Ok(()) => self.failed = false,
            Err(e) if !self.failed => {
                log(&format!("gate gc failed: {e}; continuing"));
                self.failed = true;
            }
            Err(_) => {}
        }
    }
}

/// Run the installed gate plugin as the umbrella command would. Its output
/// belongs in the daemon log only when it failed, and a timeout makes the
/// janitor job bounded like the other daemon subprocesses.
fn run_gate_gc() -> Result<(), String> {
    const TIMEOUT: Duration = Duration::from_secs(30);
    let output =
        vega_herdr::output_within(std::process::Command::new("vega-gate").arg("gc"), TIMEOUT)
            .map_err(|e| format!("could not start vega gate gc: {e}"))?
            .ok_or_else(|| "vega gate gc timed out after 30 s".to_string())?;
    if output.status.success() {
        return Ok(());
    }
    let detail = String::from_utf8_lossy(&output.stderr)
        .lines()
        .next()
        .unwrap_or_default()
        .trim()
        .to_string();
    Err(if detail.is_empty() {
        format!("vega gate gc exited {}", output.status)
    } else {
        format!("vega gate gc exited {}: {detail}", output.status)
    })
}

impl Registry {
    async fn run(&mut self, client: &async_nats::Client, host: &str, sup: &mut Supervisor) {
        match self.reconcile(client, host).await {
            Ok(RegistryUpdate::Synced(n)) => {
                self.next = Some(Instant::now() + REGISTRY_EVERY);
                self.note(&format!("registry: in sync ({n} entries of {host})"));
                if let Some(v) = &self.view {
                    observe_remote(sup, v, &mut self.turns);
                }
                self.pull_janitor(host).await;
            }
            Ok(RegistryUpdate::IdentityBlocked(why)) => {
                // This completed v2 owner is alive, even while peers still
                // have legacy records. Keep its beat fresh before FRESH; do
                // not publish projects or expose an unsafe aggregate view.
                self.next = Some(Instant::now() + REGISTRY_EVERY);
                self.view = None;
                self.note(&format!(
                    "registry: identity blocked ({why}); heartbeat only"
                ));
            }
            Err(why) => {
                self.kv = None;
                self.view = None;
                self.next = Some(Instant::now() + REGISTRY_RETRY);
                self.note(&format!("registry: off ({why}); local state only"));
            }
        }
    }

    /// An inbox-only host owns no registry keys and may read only its control
    /// host's theme (`auth-config --registry-read`, vega-lhc.90): no
    /// reconcile, no view; the last mode read stays when the broker is away.
    async fn run_theme_only(&mut self, client: &async_nats::Client, remote: &theme::Remote) {
        match theme::control_mode(client).await {
            Ok(modes) => {
                self.next = Some(Instant::now() + REGISTRY_EVERY);
                self.note(&format!(
                    "registry: theme only ({})",
                    modes
                        .iter()
                        .map(|(h, m)| format!("{h} {}", m.as_str()))
                        .next()
                        .unwrap_or_else(|| "nothing published".into())
                ));
                if let Ok(mut r) = remote.lock() {
                    *r = modes;
                }
            }
            Err(why) => {
                self.next = Some(Instant::now() + REGISTRY_RETRY);
                self.note(&format!(
                    "registry: off ({why}); theme of the control host unknown"
                ));
            }
        }
    }

    async fn reconcile(
        &mut self,
        client: &async_nats::Client,
        host: &str,
    ) -> Result<RegistryUpdate, String> {
        if self.kv.is_none() {
            self.kv = Some(vega_registry::bucket(client).await?);
            self.last.clear();
        }
        let kv = self.kv.as_ref().expect("set above");
        let list = |what: &str, key: &str| -> Vec<serde_json::Value> {
            herdr(&[what, "list"])
                .ok()
                .and_then(|v| v["result"][key].as_array().cloned())
                .unwrap_or_default()
        };
        let desired = vega_registry::local_entries(
            host,
            &list("agent", "agents"),
            &list("workspace", "workspaces"),
        );
        let mut desired = desired;
        // vega theme's applied mode (theme.rs): headless hosts follow it.
        if let Some((k, v)) = theme::registry_entry(host) {
            desired.insert(k, v);
        }
        // vega janitor's checkouts; other hosts pull them (pull_janitor).
        if let Some((k, v)) = janitor::registry_entry(host) {
            desired.insert(k, v);
        }
        let n = desired.len();
        let t = Instant::now();
        let view = vega_registry::read(client, kv).await?;
        if t.elapsed() > SLOW_STEP {
            log(&format!(
                "slow: registry read took {} ms ({} keys, {} entries)",
                t.elapsed().as_millis(),
                view.keys.len(),
                view.entries.len()
            ));
            let mut kinds: std::collections::BTreeMap<String, usize> = Default::default();
            for k in &view.keys {
                let kind: Vec<&str> = k.split('.').take(2).collect();
                *kinds.entry(kind.join(".")).or_default() += 1;
            }
            log(&format!("slow: registry keys by kind: {kinds:?}"));
        }
        if let Err(why) = identity::guard(&view.entries) {
            vega_registry::heartbeat(kv, host).await?;
            return Ok(RegistryUpdate::IdentityBlocked(why));
        }
        // Live keys only, notes too (this host's pruned notes are deleted): a
        // deleted key's subject stays, and deleting it again every 15 s cost
        // a request per dead key.
        let present: Vec<String> = view
            .keys
            .iter()
            .filter(|k| {
                vega_registry::is_note(k)
                    || vega_registry::is_janitor(k)
                    || view.entries.contains_key(*k)
            })
            .cloned()
            .collect();
        let t = Instant::now();
        self.last = vega_registry::reconcile(kv, host, desired, &self.last, &present).await?;
        slow("registry reconcile", t);
        human::seen(host, &view);
        self.view = Some(view);
        Ok(RegistryUpdate::Synced(n))
    }

    /// The other hosts' janitor summaries into files, every PULL_EVERY.
    async fn pull_janitor(&mut self, host: &str) {
        if self.janitor_next.is_some_and(|t| Instant::now() < t) {
            return;
        }
        let (Some(kv), Some(view)) = (&self.kv, &self.view) else {
            return;
        };
        self.janitor_next = Some(Instant::now() + janitor::PULL_EVERY);
        self.gate_gc.run(run_gate_gc);
        let t = Instant::now();
        match janitor::pull(kv, view, host).await {
            Ok(0) => {}
            Ok(n) => log(&format!("janitor: {n} peer summaries updated")),
            Err(e) => log(&format!("janitor: pull failed: {e}")),
        }
        slow("janitor pull", t);
    }

    /// Logs only changes, not every 15 s.
    fn note(&mut self, line: &str) {
        let kind = line.split(" (").next().unwrap_or(line).to_string();
        if kind != self.state {
            log(line);
            self.state = kind;
        }
    }
}

/// The longest one registry round may take (it is many broker calls).
const REGISTRY_CALL: Duration = Duration::from_secs(30);

fn connected(client: &async_nats::Client) -> bool {
    client.connection_state() == async_nats::connection::State::Connected
}

/// `fut` while the client stays connected, at most `limit`: a broker that
/// drops mid-round ends the round within half a second, so the loop sees the
/// outage within LOST_AFTER (vega-lhc.205). `None` when cut short.
async fn while_up<T>(
    client: &async_nats::Client,
    limit: Duration,
    fut: impl std::future::Future<Output = T>,
) -> Option<T> {
    let down = async {
        while connected(client) {
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    };
    tokio::select! {
        v = tokio::time::timeout(limit, fut) => v.ok(),
        _ = down => None,
    }
}

impl Registry {
    /// A round cut short (REGISTRY_CALL, or the broker dropped): off until
    /// the retry.
    fn timed_out(&mut self) {
        self.kv = None;
        self.view = None;
        self.next = Some(Instant::now() + REGISTRY_RETRY);
        self.note(&format!(
            "registry: off (round cut short: broker gone or no answer within {} s); local state only",
            REGISTRY_CALL.as_secs()
        ));
    }

    fn due(&self) -> bool {
        self.next.is_none_or(|t| Instant::now() >= t)
    }

    /// Reconcile within WAITING_DELAY, unless the registry is off (its
    /// retry stays).
    fn soon(&mut self) {
        if self.kv.is_some() {
            let at = Instant::now() + WAITING_DELAY;
            self.next = Some(self.next.map_or(at, |t| t.min(at)));
        }
    }
}

/// A remote child missing from its host's registry entries counts as gone
/// only when that host's beat is REGISTRY_LAG newer than our lineage record.
fn remote_gone_proven(view: Option<&vega_registry::View>, machine: &str, pane: &str) -> bool {
    let beat = view.and_then(|v| v.beat_at(&vega_registry::token(&machine.to_lowercase())));
    let written = st::state_file_on("parents", Some(machine), pane)
        .and_then(|f| std::fs::metadata(f).ok())
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64);
    matches!((beat, written), (Some(b), Some(w)) if b > w + REGISTRY_LAG.as_secs() as i64)
}

/// Two reconcile periods: a host's entries may trail a new record this much.
const REGISTRY_LAG: Duration = Duration::from_secs(40);

/// Children on other machines: their status from the registry feeds the
/// same supervision as local ones (key `<machine>/<pane>`).
fn observe_remote(
    sup: &mut Supervisor,
    view: &vega_registry::View,
    turns: &mut HashMap<String, u64>,
) {
    for c in st::children() {
        let Some(m) = c.machine.as_deref() else {
            continue;
        };
        let Some(a) = view.agent(&vega_registry::token(&m.to_lowercase()), &c.pane) else {
            continue;
        };
        let key = format!("{m}/{}", c.pane);
        let status = a["status"].as_str().unwrap_or("unknown");
        let reported = st::done_of(Some(m), &c.pane).is_some() || st::has_replied(Some(m), &c.pane);
        // A turn that started and ended between two reconciles: herdr's
        // finished-turn count rose, so the child was working meanwhile.
        let t = a["turns"].as_u64().unwrap_or(0);
        // 0: an entry from a vegad that did not publish turns yet: unknown.
        if turns
            .insert(key.clone(), t)
            .is_some_and(|prev| prev > 0 && t > prev)
        {
            sup.observe(&key, "working", reported);
        }
        sup.observe(&key, status, reported);
    }
}

/// A status for a pane. Children on this machine are supervised; any other
/// pane's turn ends are watched too, for a provider abort (`turn_ended`).
fn observe(sup: &mut Supervisor, pane: &str, status: &str) {
    let reported =
        is_local_child(pane) && (st::done_of(None, pane).is_some() || st::has_replied(None, pane));
    sup.observe(pane, status, reported);
}

fn is_local_child(pane: &str) -> bool {
    st::children()
        .iter()
        .any(|c| c.machine.is_none() && c.pane == pane)
}

/// A pane went free after a turn a moment ago (`Supervisor::ripe`). A child
/// is confirmed silent; any other pane is read only while a pointer it has
/// not read stands. A provider abort holds the pointer (delivery.rs): an
/// overload backs off, and when the retries are used up the parent sees
/// attention `quota` (the silent record), a pane without one notifies the
/// human (vega-lhc.208).
fn turn_ended(p: &str, d: &mut Delivery) {
    let child = p.contains('/') || is_local_child(p);
    let quota = if child {
        confirm_silent(p)
    } else if d.repeat(p) {
        abort_of(p)
    } else {
        return;
    };
    let Some(q) = quota else {
        d.recovered(p);
        return;
    };
    // A pointer typed into the aborted turn may be lost (delivery.rs).
    match d.aborted(p, !q.transient) {
        delivery::Abort::Nothing => {}
        delivery::Abort::Retry(w) => log(&format!(
            "pointer: {p} turn aborted, mail pending again (retry in {} min)",
            w.as_secs() / 60
        )),
        delivery::Abort::Exhausted => log(&format!(
            "pointer: {p} turn aborted, mail pending again (held: quota exhausted)"
        )),
        delivery::Abort::GaveUp => {
            log(&format!(
                "pointer: {p} turn aborted, retries used up: held until new input"
            ));
            if !child {
                vega_herdr::notes::notify(
                    "quota",
                    Some(p),
                    &format!("vegad: {p} keeps hitting the provider limit"),
                    &format!(
                        "{} retries met: {}. Its mail waits: type into it to retry.",
                        delivery::BACKOFF.len(),
                        supervise::short(&q.error, 80)
                    ),
                );
            }
        }
    }
}

/// The provider abort a pane without a parent ended its turn on; no silent
/// record (it has no parent to tell).
fn abort_of(pane: &str) -> Option<st::Quota> {
    let screen = turn_screen(pane);
    if supervise::background_running(&screen) {
        return None;
    }
    let q = provider_abort(pane, &screen)?;
    log(&format!(
        "pointer: {pane} idle on a provider abort: {}",
        supervise::short(&q.summary(), 120)
    ));
    Some(q)
}

/// The visible screen once a turn has ended: right after a turn the agent
/// may still redraw its input box; without it the status bar would pass
/// for the turn's last line.
fn turn_screen(pane: &str) -> String {
    let read = || {
        vega_herdr::herdr_text(&["pane", "read", pane, "--source", "visible"]).unwrap_or_default()
    };
    let mut screen = read();
    for _ in 0..3 {
        if supervise::read_turn_end(&screen).is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(700));
        screen = read();
    }
    screen
}

/// A suspect went free without a report a moment ago: silent, unless it
/// reported meanwhile, waits for CI, or a background shell still runs (the
/// child waits and resumes when it ends; absence proves nothing). Returns the
/// provider abort the turn ended on.
fn confirm_silent(key: &str) -> Option<st::Quota> {
    // A child on another machine (`<machine>/<pane>`): no screen to read here.
    if let Some((m, pane)) = key.split_once('/') {
        if st::done_of(Some(m), pane).is_none() && !st::has_replied(Some(m), pane) {
            st::set_silent(Some(m), pane, true);
            log(&format!(
                "sentinel: {key} idle without done or reply: silent"
            ));
        }
        return None;
    }
    let pane = key;
    if st::done_of(None, pane).is_some() || st::has_replied(None, pane) {
        return None;
    }
    let screen = turn_screen(pane);
    let silence = supervise::read_turn_end(&screen);
    if supervise::background_running(&screen) {
        log(&format!(
            "sentinel: {pane} idle, background shell running: waiting"
        ));
        return None;
    }
    let mut silence = silence.unwrap_or_default();
    silence.quota = provider_abort(pane, &screen);
    st::save_silence(None, pane, &silence);
    match (&silence.quota, &silence.question) {
        (Some(q), _) => log(&format!(
            "sentinel: {pane} idle on a provider abort: {}",
            supervise::short(&q.summary(), 120)
        )),
        (None, Some(q)) => log(&format!(
            "sentinel: {pane} idle without done or reply, asks: {}",
            supervise::short(q, 80)
        )),
        (None, None) => log(&final_attention_line(
            pane,
            vega_ci_watch::store::waiting(vega_ci_watch::unix_now())
                .get(pane)
                .map(Vec::as_slice),
        )),
    }
    silence.quota
}

/// The terminal relay log for an idle child. A CI watch that still owes its
/// verdict changes the child's final attention from silent to waiting.
fn final_attention_line(pane: &str, waiting_on: Option<&[String]>) -> String {
    match waiting_on {
        Some(targets) if !targets.is_empty() => format!(
            "sentinel: {pane} idle without done or reply: waiting on ci: {}",
            targets.join(", ")
        ),
        _ => format!("sentinel: {pane} idle without done or reply: silent"),
    }
}

/// The provider abort a silent turn ended on (quota.rs): the session log's
/// last error when herdr names the log, else the end of the screen. An
/// exhausted quota without a reset in its text takes token-burn's.
fn provider_abort(pane: &str, screen: &str) -> Option<st::Quota> {
    let agent = vega_herdr::herdr(&["agent", "get", pane])
        .map(|v| v["result"]["agent"].clone())
        .unwrap_or_default();
    let kind = agent["agent"].as_str().unwrap_or("");
    let logged = vega_transcript::Kind::from_herdr(kind).and_then(|k| {
        let home = std::env::var("HOME").ok()?;
        let path = vega_transcript::locate(
            k,
            agent["agent_session"]["value"].as_str(),
            agent["cwd"].as_str().unwrap_or(""),
            std::path::Path::new(&home),
        )
        .ok()?;
        quota::from_entries(&vega_transcript::read(k, &path).ok()?)
    });
    let abort = logged.or_else(|| quota::from_screen(screen))?;
    let now = vega_herdr::capacity::now();
    let fallback = (!abort.transient && abort.reset.is_none())
        .then(|| {
            let p = quota::provider(kind, &abort.error)?;
            let sample = vega_herdr::capacity::read().ok()?;
            vega_herdr::capacity::for_provider(&sample, p, now)?.resets_in
        })
        .flatten();
    Some(quota::resolve(
        abort,
        now,
        vega_transcript::utc_offset(),
        fallback,
    ))
}

/// Mail between a child and its parent. From the child (on this machine
/// or on `from_host`): a reply, or with `done` its report; the child is not
/// silent. From the parent on this machine: new work, which settles the
/// child; returns true (its pointer is typed while the child works). A
/// report sent before the parent's last brief is stale: the inbox marks it,
/// and the child's outcome stays open.
fn supervise_mail(
    to: &str,
    from: &str,
    from_host: &str,
    done: Option<Outcome>,
    sent_ms: &str,
    text: &str,
) -> bool {
    let local = from_host == host();
    let children = st::children();
    if let Some(c) = child_of(&children, from, to, from_host) {
        let m = c.machine.as_deref();
        if done.is_some() && stale_brief(&children, to, from, from_host, sent_ms).is_some() {
            log(&format!(
                "done: {from} -> {to} is stale (sent before the last brief), not recorded"
            ));
        } else if let Some(outcome) = done {
            let prefix = format!("done ({}): ", outcome.as_str());
            let summary = text.strip_prefix(&prefix).unwrap_or(text).to_string();
            st::save_done(m, &c.pane, &st::Done { outcome, summary });
            log(&format!("done: {from} ({}) -> {to}", outcome.as_str()));
        }
        st::set_replied(m, &c.pane);
        st::set_silent(m, &c.pane, false);
        return false;
    }
    let from_parent = local
        && children
            .iter()
            .any(|c| c.machine.is_none() && c.pane == to && c.parent.pane == from);
    if from_parent {
        st::settle(None, to);
        log(&format!("sentinel: {to} got new work from {from}, settled"));
    }
    from_parent
}

/// herdr pane ids look like `w3V:p17`: alphanumerics, one colon.
fn valid_pane_id(p: &str) -> bool {
    let ok = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric());
    matches!(p.split_once(':'), Some((w, q)) if ok(w) && ok(q))
}

fn log(line: &str) {
    let t = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    println!("{t} {line}");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bounce_id_is_stable_per_retry_and_distinct_per_sender_and_close() {
        let o = |agent: &str| delivery::Origin {
            host: "h".into(),
            pane: "w9:p1".into(),
            agent: agent.into(),
        };
        let b = |closed_ms| delivery::Bounce {
            pane: "w1:p1".into(),
            origins: vec![Some(o("claude"))],
            done: vec![],
            closed_ms,
        };
        let id = bounce_msg_id("h", &b(1), &o("claude"));
        assert_eq!(id, bounce_msg_id("h", &b(1), &o("claude")), "a retry");
        assert_ne!(id, bounce_msg_id("h", &b(1), &o("codex")), "another agent");
        assert_ne!(id, bounce_msg_id("h", &b(2), &o("claude")), "another close");
    }

    #[test]
    fn unknown_screen_holds_until_quiet() {
        let mut d = Drafts::default();
        let t0 = Instant::now();
        let at = |s: u64| t0 + Duration::from_secs(s);
        assert!(d.unknown_holds("w1:p1", "> typ", t0), "first sight holds");
        assert!(
            d.unknown_holds("w1:p1", "> typing", at(15)),
            "changed: holds"
        );
        assert!(d.unknown_holds("w1:p1", "> typing", at(30)), "quiet 15 s");
        assert!(
            !d.unknown_holds("w1:p1", "> typing", at(35)),
            "quiet 20 s: types"
        );
        assert!(d.unknown.is_empty() && d.seen.is_empty(), "state cleared");
    }

    #[test]
    fn broker_requires_explicit_endpoint() {
        const FIXTURE: &str = "VEGA_ENDPOINT_CONFIG_FIXTURE";
        if std::env::var_os(FIXTURE).is_none() {
            let root = tempfile::tempdir().unwrap();
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "tests::broker_requires_explicit_endpoint",
                    "--nocapture",
                ])
                .env(FIXTURE, "1")
                .env("HOME", root.path())
                .env("XDG_CONFIG_HOME", root.path().join("config"))
                .env("XDG_STATE_HOME", root.path().join("state"))
                .env_remove("VEGA_NATS_URL")
                .env("VEGA_NATS_NKEY", "/must-not-read-a-key")
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            return;
        }
        assert!(url().is_empty());
        assert!(!broker_configured());
        let error = runtime()
            .unwrap()
            .block_on(connect_as("test", Seed::Host, None))
            .unwrap_err();
        assert_eq!(error.code, "broker_unreachable");
        assert_eq!(error.message, "no broker configured");
        let error = runtime()
            .unwrap()
            .block_on(async {
                tokio::time::timeout(Duration::from_secs(2), connect_patiently())
                    .await
                    .expect("unconfigured sends must not wait for SEND_WAIT")
            })
            .unwrap_err();
        assert_eq!(error.code, "broker_unreachable");
        assert_eq!(error.message, "no broker configured");
        let config =
            std::path::PathBuf::from(std::env::var_os("XDG_CONFIG_HOME").unwrap()).join("vega");
        std::fs::create_dir_all(&config).unwrap();
        std::fs::write(
            config.join("nats.url"),
            "nats://broker.example:4222\nignored\n",
        )
        .unwrap();
        assert_eq!(url(), "nats://broker.example:4222");
        assert!(broker_configured());
        std::env::set_var("VEGA_NATS_URL", "nats://override.example:4222");
        assert_eq!(url(), "nats://override.example:4222");
    }

    /// A tool shell without the pane identity (Codex `inherit = "core"`)
    /// and outside every pane's process tree must not read some inbox.
    #[test]
    fn inbox_without_pane_identity_fails_loudly() {
        use std::os::unix::fs::PermissionsExt;
        const FIXTURE: &str = "VEGA_INBOX_IDENTITY_FIXTURE";
        if std::env::var_os(FIXTURE).is_none() {
            let root = tempfile::tempdir().unwrap();
            let herdr = root.path().join("herdr");
            std::fs::write(&herdr, "#!/bin/sh\necho '{\"result\":{\"panes\":[]}}'\n").unwrap();
            std::fs::set_permissions(&herdr, std::fs::Permissions::from_mode(0o755)).unwrap();
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "tests::inbox_without_pane_identity_fails_loudly",
                    "--nocapture",
                ])
                .env(FIXTURE, "1")
                .env("HOME", root.path())
                .env("PATH", format!("{}:/bin:/usr/bin", root.path().display()))
                .env_remove("HERDR_PANE_ID")
                .env_remove("HERDR_SOCKET_PATH")
                .env("VEGA_NATS_URL", "nats://must-not-connect.invalid:4222")
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            return;
        }
        for f in [inbox(false, 10).unwrap_err(), inbox_show(1).unwrap_err()] {
            assert_eq!(f.code, "not_in_herdr");
            assert_eq!(f.message, "no herdr pane hosts this process");
            assert!(f.hint.contains("vega mux spawn"), "{}", f.hint);
        }
    }

    #[test]
    fn failed_gate_gc_is_logged_once_and_never_stops_future_ticks() {
        let mut gc = GateGc::default();
        gc.run(|| Err("fixture failure".into()));
        assert!(gc.failed);
        assert!(gc.next.is_some());

        // Move the next tick due without waiting five minutes. The second
        // feature failure is swallowed; the relay loop can carry on.
        gc.next = None;
        gc.run(|| Err("still broken".into()));
        assert!(gc.failed);

        // A later successful tick resets the one-time failure guard, so a
        // subsequent outage remains visible in the daemon log.
        gc.next = None;
        gc.run(|| Ok(()));
        assert!(!gc.failed);
    }

    #[test]
    fn local_line_is_one_line_and_cut_at_the_end() {
        let l = local_line(
            "tool vega-upgrade",
            "a.\nAsk the user.\n\nChangelog:\n  8 · x",
        );
        assert_eq!(
            l,
            "[tool vega-upgrade] a. | Ask the user. | Changelog: | 8 · x"
        );
        let long = local_line("t", &format!("keep\n{}", "x".repeat(5000)));
        assert_eq!(long.chars().count(), LOCAL_LINE_MAX);
        assert!(long.starts_with("[t] keep | x") && long.ends_with('…'));
    }

    #[test]
    fn unreachable_hint_names_the_config_when_none_is_set() {
        let none = unreachable_hint(false);
        assert!(none.contains("no broker configured (VEGA_NATS_URL, ~/.config/vega/nats.url)"));
        assert!(none.contains("vega onboard"));
        assert!(!unreachable_hint(true).contains("no broker configured"));
    }

    #[test]
    fn workspace_cmd_reports_or_clears_the_workspace_tokens() {
        let sym = vega_herdr::theme::parse("[symbols.icons]\ndecisions = \"D\"\n");
        let b = vega_herdr::badges::Badges {
            decisions: 2,
            ..Default::default()
        };
        assert_eq!(
            workspace_cmd("w1", &b, None, None, &[], &sym),
            [
                "workspace",
                "report-metadata",
                "w1",
                "--source",
                "vega:vegad",
                "--ttl-ms",
                sidebar::TTL_MS,
                "--clear-token",
                "notes",
                "--token",
                "decisions=D 2",
                "--clear-token",
                "waiting",
                "--clear-token",
                "badge",
                "--clear-token",
                "git",
                "--clear-token",
                "kind1",
                "--clear-token",
                "kind2",
                "--clear-token",
                "kind3",
                "--clear-token",
                "kind4",
                "--clear-token",
                "kind5",
            ]
        );
        let ws = workspace_cmd(
            "w1",
            &Default::default(),
            Some("S"),
            Some("G"),
            &["C".into()],
            &sym,
        );
        assert_eq!(
            ws[ws.len() - 14..],
            [
                "--token",
                "badge=S",
                "--token",
                "git=G",
                "--token",
                "kind1=C",
                "--clear-token",
                "kind2",
                "--clear-token",
                "kind3",
                "--clear-token",
                "kind4",
                "--clear-token",
                "kind5"
            ]
        );
    }

    #[test]
    fn workspace_kinds_prioritize_roles_deduplicate_and_isolate_workspaces() {
        let mut l = Layout {
            panes: serde_json::from_value(serde_json::json!([
                {"pane_id":"w1:p1", "workspace_id":"w1", "agent":"pi"},
                {"pane_id":"w1:p2", "workspace_id":"w1", "agent":"claude"},
                {"pane_id":"w1:p3", "workspace_id":"w1", "agent":"codex"},
                {"pane_id":"w1:p4", "workspace_id":"w1", "agent":"claude"},
                {"pane_id":"w1:p5", "workspace_id":"w1", "agent":"opencode"},
                {"pane_id":"w1:p6", "workspace_id":"w1", "agent":"agy"},
                {"pane_id":"w1:p7", "workspace_id":"w1", "agent":"copilot"},
                {"pane_id":"w1:p8", "workspace_id":"w1"},
                {"pane_id":"w2:p1", "workspace_id":"w2", "agent":"pi"}
            ]))
            .unwrap(),
            workspaces: Vec::new(),
            tabs: Vec::new(),
        };
        let sym = vega_herdr::theme::parse(
            "[symbols.icons]\nclaude = \"C\"\ncodex = \"X\"\npi = \"P\"\ncopilot = \"G\"\nother_agent = \"?\"\n",
        );
        let sentinels = HashMap::from([("w1:p3".into(), "sentinel".into())]);
        let uacs = vec!["w1:p4".into()];
        assert_eq!(
            l.workspace_kinds("w1", &uacs, &sentinels, &sym),
            ["C", "X", "P", "?", "G"]
        );
        l.panes.reverse();
        assert_eq!(
            l.workspace_kinds("w1", &uacs, &sentinels, &sym),
            ["C", "X", "P", "?", "G"]
        );
        assert_eq!(l.workspace_kinds("w1", &[], &sentinels, &sym)[0], "X");
        // Each remote host computes the same way from its own layout; local
        // UAC/sentinel IDs cannot pull another workspace's kinds into it.
        assert_eq!(l.workspace_kinds("w2", &uacs, &sentinels, &sym), ["P"]);
        assert!(l
            .workspace_kinds("gone", &uacs, &sentinels, &sym)
            .is_empty());
        let blank = vega_herdr::theme::parse("[symbols.icons]\npi = \"\"\n");
        assert!(l.workspace_kinds("w2", &[], &sentinels, &blank).is_empty());
        assert_eq!(
            l.workspace_kinds("w2", &[], &sentinels, &Default::default()),
            ["π"]
        );
        l.panes.clear();
        assert!(l.workspace_kinds("w1", &uacs, &sentinels, &sym).is_empty());
    }

    #[test]
    fn git_token_checks_a_clean_level_checkout_else_names_the_difference() {
        let head = "# branch.oid abc\n# branch.head main\n";
        assert_eq!(
            git_token(&format!(
                "{head}# branch.upstream origin/main\n# branch.ab +0 -0\n"
            )),
            "\u{2713}"
        );
        // No upstream: level.
        assert_eq!(git_token(head), "\u{2713}");
        assert_eq!(
            git_token(&format!(
                "{head}# branch.ab +2 -1\n1 .M N... 100644 100644 100644 a b f.rs\n"
            )),
            "\u{2191}2 \u{2193}1 \u{25cf}"
        );
        assert_eq!(git_token(&format!("{head}? new.txt\n")), "\u{25cf}");
    }

    #[test]
    fn workspace_badge_prefers_the_uac() {
        let sym = vega_herdr::theme::parse(
            "[symbols]\nuac_text = \"u^c\"\n[symbols.icons]\nuac = \"\"\nsentinel = \"S\"\n",
        );
        assert_eq!(workspace_badge(&sym, true, true).as_deref(), Some("u^c"));
        assert_eq!(workspace_badge(&sym, false, true).as_deref(), Some("S"));
        assert_eq!(workspace_badge(&sym, false, false), None);
    }

    #[test]
    fn sentinel_label_prefixes_the_symbol() {
        let sym = vega_herdr::theme::parse("[symbols.icons]\nsentinel = \"S\"\n");
        assert_eq!(sentinel_label(&sym, "pr5"), "S pr5");
        let none = vega_herdr::theme::parse("[symbols.icons]\nsentinel = \"\"\n");
        assert_eq!(sentinel_label(&none, "pr5"), "pr5");
    }

    fn fixture_layout() -> Layout {
        let panes = serde_json::json!([
            {"pane_id": "w1:p1", "tab_id": "w1:t1", "agent": "claude"},
            {"pane_id": "w1:p2", "tab_id": "w1:t2", "agent": "claude"},
            {"pane_id": "w1:p3", "tab_id": "w1:t3", "agent": "pi"},
            {"pane_id": "w1:p4", "tab_id": "w1:t9"},
        ]);
        let tabs = serde_json::json!([
            {"tab_id": "w1:t1", "label": "vega"},
            {"tab_id": "w1:t2", "label": "label-row"},
            {"tab_id": "w1:t3", "label": "deimos"},
        ]);
        Layout {
            panes: panes.as_array().unwrap().clone(),
            workspaces: Vec::new(),
            tabs: tabs.as_array().unwrap().clone(),
        }
    }

    #[test]
    fn label_token_names_sentinels_and_agents_and_clears_the_uac() {
        let sym = vega_herdr::theme::parse("[symbols.icons]\nsentinel = \"S\"\n");
        let l = fixture_layout();
        assert_eq!(
            l.label_token(&sym, "w1:p1", None, true),
            ["--clear-token", "label"]
        );
        assert_eq!(
            l.label_token(&sym, "w1:p2", Some("lr"), false),
            ["--token", "label=S label-row"]
        );
        assert_eq!(
            l.label_token(&sym, "w1:p3", None, false),
            ["--token", "label=deimos"]
        );
        // No tab label: the sentinel's name, the agent kind.
        assert_eq!(
            l.label_token(&sym, "w1:p4", Some("lr"), false),
            ["--token", "label=S lr"]
        );
        let mut untabbed = fixture_layout();
        untabbed.tabs.clear();
        assert_eq!(
            untabbed.label_token(&sym, "w1:p3", None, false),
            ["--token", "label=pi"]
        );
        // Neither sentinel nor a known agent kind: nothing to show.
        assert_eq!(
            l.label_token(&sym, "w1:p4", None, false),
            ["--clear-token", "label"]
        );
    }

    #[test]
    fn kind_token_puts_the_kind_symbol_on_every_agent_pane() {
        let sym = vega_herdr::theme::parse(
            "[symbols.icons]\nclaude = \"C\"\npi = \"P\"\nother_agent = \"?\"\n",
        );
        let mut l = fixture_layout();
        l.panes
            .push(serde_json::json!({"pane_id": "w1:p5", "agent": "opencode"}));
        // UAC and sentinel panes run claude: the kind shows there too.
        assert_eq!(l.kind_token(&sym, "w1:p1"), ["--token", "kind=C"]);
        assert_eq!(l.kind_token(&sym, "w1:p2"), ["--token", "kind=C"]);
        assert_eq!(l.kind_token(&sym, "w1:p3"), ["--token", "kind=P"]);
        assert_eq!(l.kind_token(&sym, "w1:p5"), ["--token", "kind=?"]);
        assert_eq!(l.kind_token(&sym, "w1:p4"), ["--clear-token", "kind"]);
        let nerd = vega_herdr::theme::Symbols::default();
        assert_eq!(
            l.kind_token(&nerd, "w1:p1"),
            ["--token", "kind=\u{EC82}\u{2800}"]
        );
        let blank = vega_herdr::theme::parse("[symbols.icons]\npi = \"\"\n");
        assert_eq!(l.kind_token(&blank, "w1:p3"), ["--clear-token", "kind"]);
    }

    #[test]
    fn project_token_names_the_parents_workspace_else_the_own() {
        let panes = serde_json::json!([
            {"pane_id": "w1:p1", "workspace_id": "w1"},
            {"pane_id": "w2:p1", "workspace_id": "w2"},
            {"pane_id": "w3:p1", "workspace_id": "w3"},
        ]);
        let workspaces = serde_json::json!([
            {"workspace_id": "w1", "label": "vega"},
            {"workspace_id": "w2", "label": "sentinel/theseus-23-1"},
            {"workspace_id": "w3", "label": ""},
        ]);
        let l = Layout {
            panes: panes.as_array().unwrap().clone(),
            workspaces: workspaces.as_array().unwrap().clone(),
            tabs: Vec::new(),
        };
        // A sentinel in a worktree workspace: its local parent's workspace.
        assert_eq!(
            l.project_token("w2:p1", Some("w1:p1")),
            ["--token", "project=vega"]
        );
        // Parent gone (or remote, so not passed): the own workspace.
        assert_eq!(
            l.project_token("w2:p1", Some("w9:p1")),
            ["--token", "project=sentinel/theseus-23-1"]
        );
        // A plain pane: its own workspace.
        assert_eq!(l.project_token("w1:p1", None), ["--token", "project=vega"]);
        // No workspace label anywhere: cleared.
        assert_eq!(l.project_token("w3:p1", None), ["--clear-token", "project"]);
    }

    #[test]
    fn host_token_carries_the_configured_label_or_the_bare_name() {
        let sym = vega_herdr::theme::parse("[symbols.hosts]\nalpha = \"A\"\n");
        assert_eq!(host_token(&sym, "alpha"), ["--token", "host=A"]);
        assert_eq!(host_token(&sym, "beta"), ["--token", "host=beta"]);
    }

    #[test]
    fn quiesced_metadata_names_host_and_kind_on_every_pane() {
        let sym = vega_herdr::theme::parse(
            "[symbols.hosts]\nganymede = \"G\"\n[symbols.icons]\nclaude = \"C\"\npi = \"P\"\n",
        );
        let mut l = fixture_layout();
        l.panes[0]["workspace_id"] = "w1".into();
        l.workspaces = vec![serde_json::json!({"workspace_id": "w1", "label": "vega"})];
        l.panes.push(serde_json::json!({"pane_id": "bogus id"}));
        let got: HashMap<_, _> = quiesced_metadata(&l, &sym, "ganymede")
            .into_iter()
            .collect();
        // Every valid pane, with the plain tab label: no UAC or sentinel
        // records before the identity is ready.
        assert_eq!(
            got["w1:p1"],
            [
                "--token",
                "host=G",
                "--token",
                "kind=C",
                "--token",
                "project=vega",
                "--token",
                "label=vega"
            ]
        );
        assert_eq!(
            got["w1:p3"],
            [
                "--token",
                "host=G",
                "--token",
                "kind=P",
                "--clear-token",
                "project",
                "--token",
                "label=deimos"
            ]
        );
        // A pane without an agent keeps its host, nothing else.
        assert_eq!(
            got["w1:p4"],
            [
                "--token",
                "host=G",
                "--clear-token",
                "kind",
                "--clear-token",
                "project",
                "--clear-token",
                "label"
            ]
        );
        assert_eq!(got.len(), 4);
    }

    #[test]
    fn skill_has_valid_frontmatter() {
        vega_plugin::assert_guide(SKILL, "post");
    }

    #[test]
    fn final_attention_logs_waiting_with_owed_ci_targets() {
        let targets = [
            "owner/repo#42".to_string(),
            "owner/repo@deadbeef".to_string(),
        ];
        assert_eq!(
            final_attention_line("w3V:p2", Some(&targets)),
            "sentinel: w3V:p2 idle without done or reply: waiting on ci: owner/repo#42, owner/repo@deadbeef"
        );
        assert_eq!(
            final_attention_line("w3V:p2", None),
            "sentinel: w3V:p2 idle without done or reply: silent"
        );
    }

    #[test]
    fn tool_senders_are_vega_names_with_a_prefix() {
        assert_eq!(tool_sender("vega-review").unwrap(), "tool vega-review");
        for bad in [
            "the user",
            "claude",
            "vega-",
            "vega-Review",
            "vega-re view",
            "vega review",
            "vega-a1",
            "xvega-a",
        ] {
            assert_eq!(tool_sender(bad).unwrap_err().code, "usage", "{bad}");
        }
    }

    #[test]
    fn only_pane_ids_reach_herdr() {
        assert!(valid_pane_id("w3V:p17"));
        assert!(!valid_pane_id("--help"));
        assert!(!valid_pane_id("natspeer"));
        assert!(!valid_pane_id("w3V:p1:x"));
        assert!(!valid_pane_id(":p1"));
    }

    #[test]
    fn inbox_prefix_is_per_host() {
        assert_eq!(inbox_prefix("proteus"), "_INBOX_proteus");
    }

    #[test]
    fn durations_parse_with_unit() {
        assert_eq!(parse_duration("30m").unwrap(), Duration::from_secs(1800));
        assert_eq!(
            parse_duration("7d").unwrap(),
            Duration::from_secs(7 * 86400)
        );
        assert_eq!(parse_duration("2x").err().unwrap().code, "usage");
        assert_eq!(parse_duration("h").err().unwrap().code, "usage");
        assert_eq!(parse_duration("").err().unwrap().code, "usage");
    }

    #[test]
    fn archive_limits() {
        assert_eq!(ARCHIVE_MAX_AGE.as_secs(), 365 * 86400);
        assert_eq!(ARCHIVE_MAX_BYTES, 1024 * 1024 * 1024);
    }

    #[test]
    fn layout_places_panes_and_boards_in_projects() {
        use serde_json::json;
        let layout = Layout {
            panes: vec![
                json!({"schema":2, "project":"github.com/team/vega", "pane_id": "w1:p2", "workspace_id": "w1", "cwd": "/Users/u/src/vega.fix"}),
                json!({"schema":2, "project":"github.com/team/sk", "pane_id": "w2:p1", "workspace_id": "w2", "cwd": "/Users/u/src/sk/x"}),
                json!({"schema":2, "project":"github.com/team/other", "pane_id": "w3:p1", "workspace_id": "w3", "cwd": "/srv/x",
                    "terminal_title": "vega-board-other"}),
                json!({"schema":2, "project":"github.com/team/vega", "pane_id": "w1:p3", "workspace_id": "w1", "cwd": "/Users/u/src/vega",
                    "terminal_title": "vega-board-vega"}),
            ],
            workspaces: vec![
                json!({"schema":2, "project":"github.com/team/vega", "workspace_id": "w1", "worktree": {"repo_key": "/Users/u/src/vega/.git"}}),
                json!({"workspace_id": "w2"}),
                json!({"schema":2, "project":"github.com/team/other", "workspace_id": "w3", "worktree": {"repo_key": "/Users/u/src/other/.git"}}),
            ],
            tabs: Vec::new(),
        };
        let vega = "github.com/team/vega";
        // A sibling worktree belongs through its workspace's repo_key.
        assert!(layout.in_project(vega, "w1:p2"));
        assert!(!layout.in_project(vega, "w2:p1"));
        // The snapshot has already resolved this pane to its project ID.
        assert!(layout.in_project("github.com/team/sk", "w2:p1"));
        assert!(!layout.in_project(vega, "w9:p9"));
        assert_eq!(
            layout.boards(&[vega.to_string()]),
            vec![
                (
                    "w3:p1".to_string(),
                    Some("github.com/team/other".to_string())
                ),
                ("w1:p3".to_string(), Some(vega.to_string())),
            ]
        );
    }

    #[test]
    fn project_workspaces_skip_linked_worktrees() {
        use serde_json::json;
        let key = "/Users/u/src/vega/.git";
        let layout = Layout {
            panes: Vec::new(),
            workspaces: vec![
                json!({"schema":2, "project":"github.com/team/vega", "workspace_id": "w1", "worktree": {"repo_key": key,
                    "is_linked_worktree": false}}),
                json!({"schema":2, "project":"github.com/team/vega", "workspace_id": "w2", "worktree": {"repo_key": key,
                    "is_linked_worktree": true}}),
            ],
            tabs: Vec::new(),
        };
        assert_eq!(layout.project_workspaces("github.com/team/vega"), ["w1"]);
    }

    #[test]
    fn subjects_keep_pane_ids_and_round_trip() {
        let s = inbox_subject("proteus", "w3V:p17", "ganymede");
        assert_eq!(s, "vega.inbox.proteus.w3V:p17.ganymede");
        assert_eq!(parse_subject(&s), Some(("w3V:p17", "ganymede")));
        assert_eq!(parse_subject("vega.inbox.a.b"), None);
        assert_eq!(inbox_subject("a.b", "x y", "c"), "vega.inbox.a_b.x_y.c");
        assert_eq!(
            pane_filter("proteus", "w3V:p17"),
            "vega.inbox.proteus.w3V:p17.*"
        );
        assert_eq!(stream_name("proteus"), "VEGA_INBOX_proteus");
    }

    #[test]
    fn consumer_names_are_safe() {
        assert_eq!(consumer_name("proteus", "w3V:p17"), "inbox-proteus-w3V_p17");
    }

    #[test]
    fn fast_replies_stop_at_the_limit() {
        let now = 1_000_000;
        let (thread, mut hop) = next_hop(None, now).unwrap();
        assert_eq!(hop, 1);
        assert!(!thread.is_empty());
        for expected in 2..=MAX_HOPS {
            let token = format!("{thread}/{hop}@{now}");
            let (same, next) = next_hop(Some(&token), now + 1).unwrap();
            assert_eq!(same, thread);
            assert_eq!(next, expected);
            hop = next;
        }
        for attempt in 9..=10 {
            let token = format!("{thread}/{hop}@{now}");
            assert_eq!(
                next_hop(Some(&token), now + attempt).unwrap_err().code,
                "hop_limit"
            );
        }
    }

    #[test]
    fn twenty_slow_replies_preserve_the_thread_and_reset_hops() {
        let mut now = 1_000_000;
        let mut hop = MAX_HOPS;
        for _ in 0..20 {
            let token = format!("work/{hop}@{now}");
            now += FAST_REPLY_WINDOW.as_millis() + 1;
            let (thread, next) = next_hop(Some(&token), now).unwrap();
            assert_eq!(thread, "work");
            assert_eq!(next, 1);
            hop = next;
        }
    }

    #[test]
    fn reply_window_boundary_and_clock_skew() {
        let token = "work/8@1000000";
        assert_eq!(
            next_hop(Some(token), 1_119_999).unwrap_err().code,
            "hop_limit"
        );
        assert_eq!(
            next_hop(Some(token), 1_120_000).unwrap(),
            ("work".into(), 1)
        );
        assert_eq!(
            next_hop(Some(token), 999_999).unwrap_err().code,
            "hop_limit"
        );
        assert_eq!(next_hop(Some("work/2@1000000"), 1_000_001).unwrap().1, 3);
    }

    #[test]
    fn legacy_and_malformed_reply_tokens() {
        assert_eq!(next_hop(Some("abc/1"), 0).unwrap(), ("abc".into(), 2));
        for token in ["abc/8", "abc/4294967295"] {
            assert_eq!(
                next_hop(Some(token), u128::MAX).unwrap_err().code,
                "hop_limit"
            );
        }
        for token in [
            "abc",
            "/3",
            "abc/3@",
            "abc/3@no",
            "abc/3@1@2",
            "abc/4294967296",
        ] {
            assert_eq!(
                next_hop(Some(token), 0).unwrap_err().code,
                "usage",
                "{token}"
            );
        }
    }

    #[test]
    fn capped_reply_hints_match_token_timing() {
        let legacy = next_hop(Some("abc/8"), 1_000_001).unwrap_err();
        assert!(legacy
            .hint
            .contains("legacy token has no timestamp and stays capped"));
        assert!(legacy.hint.contains("vega post inbox --show <id>"));
        assert!(legacy.hint.contains("upgraded host"));
        assert!(!legacy.hint.contains("after 120 s"));
        let timed = next_hop(Some("abc/8@1000000"), 1_000_001).unwrap_err();
        assert!(timed
            .hint
            .contains("same timestamped --re token after 120 s"));
        assert!(!timed.hint.contains("legacy"));
    }

    #[test]
    fn a_report_before_the_last_brief_is_stale() {
        assert_eq!(sent_before("1000", Some(2000)), Some(2000));
        assert_eq!(sent_before("2000", Some(2000)), None);
        assert_eq!(sent_before("3000", Some(2000)), None);
        assert_eq!(sent_before("1000", None), None);
        assert_eq!(sent_before("?", Some(2000)), None);
    }

    #[test]
    fn inbox_show_takes_an_id_and_conflicts_with_reading() {
        let c = Cli::try_parse_from(["vega post", "inbox", "--show", "138"]).unwrap();
        assert!(matches!(
            c.cmd,
            Some(Cmd::Inbox {
                show: Some(138),
                ..
            })
        ));
        assert!(Cli::try_parse_from(["vega post", "inbox", "--show", "1", "--keep"]).is_err());
    }

    #[test]
    fn bd_show_tells_a_closed_bead() {
        assert_eq!(
            bead_closed(r#"[{"id":"x-1","status":"closed"}]"#),
            Some(true)
        );
        assert_eq!(
            bead_closed(r#"[{"id":"x-1","status":"open"}]"#),
            Some(false)
        );
        assert_eq!(
            bead_closed(r#"{"id":"x-1","status":"in_progress"}"#),
            Some(false)
        );
        assert_eq!(
            bead_closed(r#"{"data":[{"id":"x-1","status":"closed"}],"schema_version":1}"#),
            Some(true)
        );
        assert_eq!(bead_closed("[]"), None);
        assert_eq!(bead_closed("no bead"), None);
    }

    #[test]
    fn send_takes_a_pane_and_text_or_a_uac_and_text() {
        let parse = |args: &[&str]| {
            let cli = Cli::try_parse_from(["vega post", "send"].iter().chain(args))
                .map_err(|e| e.kind())?;
            let Some(Cmd::Send {
                args, host, uac, ..
            }) = cli.cmd
            else {
                unreachable!()
            };
            Ok::<_, ErrorKind>((args, host, uac))
        };
        let (args, host, uac) = parse(&["w3V:p2", "ping"]).unwrap();
        let (to, text) = send_args(&args, host.as_deref(), uac.as_deref(), false).unwrap();
        assert!(matches!(to, To::Pane("w3V:p2", None)));
        assert_eq!(text, "ping");
        let (args, host, uac) = parse(&["--uac", "vega", "ping"]).unwrap();
        let (to, text) = send_args(&args, host.as_deref(), uac.as_deref(), false).unwrap();
        assert!(matches!(to, To::Uac("vega")));
        assert_eq!(text, "ping");
        let (args, host, uac) = parse(&["--uac", "vega", "w3V:p2", "ping"]).unwrap();
        assert_eq!(
            send_args(&args, host.as_deref(), uac.as_deref(), false)
                .unwrap_err()
                .code,
            "usage"
        );
        let (args, host, uac) = parse(&["ping"]).unwrap();
        assert_eq!(
            send_args(&args, host.as_deref(), uac.as_deref(), false)
                .unwrap_err()
                .code,
            "usage"
        );
        assert_eq!(
            parse(&["--uac", "vega", "--host", "titan", "ping"]).unwrap_err(),
            ErrorKind::ArgumentConflict
        );
        let one = ["ping".to_string()];
        let to = send_args(&one, None, None, true).unwrap().0;
        assert!(matches!(to, To::Uber));
        let two = ["w3V:p2".to_string(), "ping".to_string()];
        assert_eq!(send_args(&two, None, None, true).unwrap_err().code, "usage");
        for conflict in [["--uber", "--uac", "vega"], ["--uber", "--host", "titan"]] {
            assert_eq!(
                parse(&[conflict.as_slice(), &["ping"]].concat()).unwrap_err(),
                ErrorKind::ArgumentConflict
            );
        }
    }

    #[test]
    fn send_kind_refuses_bad_bodies_before_sending() {
        assert_eq!(mail_kind(None, "plain text").unwrap(), None);
        let applied = r#"{"id":"vega-lhc.209~AbCdEfGhIjKlMnOpQrStUv","result":"applied"}"#;
        assert_eq!(
            mail_kind(Some("uber.applied"), applied).unwrap(),
            Some(vega_uber_msg::Kind::Applied)
        );
        let refused = |kind, body: &str| mail_kind(Some(kind), body).unwrap_err().code;
        assert_eq!(refused("uber.tell", "{}"), "uber_kind");
        assert_eq!(refused("uber.applied", "not json"), "uber_malformed");
        assert_eq!(refused("uber.brief", &"x".repeat(129)), "uber_too_large");
    }

    #[test]
    fn a_bounce_names_the_pane_and_the_way_on() {
        let t = bounce_text(
            "w30:p1",
            "proteus",
            "the UAC of spellkave is w5S:p1 on ganymede now",
        );
        assert!(
            t.starts_with("bounce: w30:p1 on proteus closed before it read your mail"),
            "{t}"
        );
        assert!(t.contains("w5S:p1 on ganymede"), "{t}");
        assert!(!t.contains('\n'));
        // No reply path: a bounce comes from a tool, never a pane.
        assert_eq!(tool_sender("vega-post").unwrap(), BOUNCE_AGENT);
    }

    #[test]
    fn pointer_is_one_short_line() {
        assert_eq!(
            pointer(1),
            "[vega-post] 1 new message for you. Run: vega post inbox"
        );
        assert!(!pointer(3).contains('\n'));
    }
}
