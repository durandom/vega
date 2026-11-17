// Native mail client and minimal local pointer worker; no full vegad jobs.
mod client_identity;
mod client_key;
#[cfg(windows)]
mod core_doctor;
#[allow(dead_code)] // Shared policy also includes Unix daemon features.
mod delivery;
mod native_relay;
mod reach;
#[allow(dead_code)] // Shared module includes Unix terminal operations.
mod terminals;

use async_nats::jetstream::{self, consumer::pull, stream};
use async_nats::HeaderMap;
use clap::{Parser, Subcommand};
use futures_util::StreamExt;
use std::collections::{HashMap, VecDeque};
use std::env;
use std::process::ExitCode;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use vega_herdr::state::{self as st, Outcome};
use vega_herdr::{cell, collapse_home, herdr, local_time, print_failure, Failure};
include!("client_common.rs");

#[derive(Parser)]
#[command(
    name = "vega post",
    bin_name = "vega post",
    disable_version_flag = true,
    about = "Native agent mail client",
    after_help = include_str!("../help/client.md")
)]
struct Cli {
    #[arg(long)]
    skill: bool,
    #[arg(short = 'V', short_alias = 'v', long)]
    version: bool,
    #[command(subcommand)]
    command: Option<Cmd>,
}
#[derive(Subcommand)]
enum Cmd {
    /// Broker, host, verified caller and unread count
    Status,
    /// Inspect the local hc core session without starting it or needing a broker
    #[cfg(windows)]
    Doctor,
    /// Send mail, or reply with the timestamped --re token from inbox
    #[command(
        after_help = "Examples:\n  vega post send --host proteus t7 \"<text>\"\n  vega post send --uac . \"<text>\"\n  vega post send --host proteus t7 --re <thread>/<hop>@<ms> \"<reply>\""
    )]
    Send {
        #[arg(value_names = ["TARGET", "TEXT"], num_args = 1..=2, required = true)]
        args: Vec<String>,
        #[arg(long, conflicts_with = "uac")]
        host: Option<String>,
        #[arg(long)]
        uac: Option<String>,
        #[arg(long, conflicts_with_all = ["host", "uac"])]
        uber: bool,
        #[arg(long)]
        re: Option<String>,
        #[arg(long)]
        now: bool,
        #[arg(long)]
        kind: Option<String>,
    },
    /// Read mail and ack it; --keep and --show preserve read state
    #[command(
        after_help = "Examples:\n  vega post inbox\n  vega post inbox --keep\n  vega post inbox --show <id>"
    )]
    Inbox {
        #[arg(long)]
        keep: bool,
        #[arg(long, conflicts_with_all = ["keep", "max"])]
        show: Option<u64>,
        #[arg(long, default_value_t = 20, value_parser = clap::value_parser!(u16).range(1..))]
        max: u16,
    },
    /// Read the archive without acknowledging messages
    #[command(
        after_help = "Examples:\n  vega post log --since 2h\n  vega post log --pane t7 --full\n  vega post log --json --limit 50"
    )]
    Log {
        #[arg(long)]
        pane: Option<String>,
        #[arg(long)]
        from: Option<String>,
        #[arg(long)]
        since: Option<String>,
        #[arg(long, default_value_t = 20)]
        limit: usize,
        #[arg(long)]
        full: bool,
        #[arg(long)]
        json: bool,
    },
    /// Report completion to the recorded parent or explicit --to
    #[command(
        after_help = "Examples:\n  vega post done --outcome succeeded \"<summary>\"\n  vega post done --to t7 --host proteus --outcome failed \"<blocker>\""
    )]
    Done {
        #[arg(long, value_parser = ["succeeded", "failed"])]
        outcome: String,
        summary: String,
        #[arg(long)]
        to: Option<String>,
        #[arg(long, requires = "to")]
        host: Option<String>,
    },
    /// Start or run the minimal local hc pointer worker
    Relay {
        #[command(subcommand)]
        action: native_relay::Action,
    },
    /// Create this host's own user nkey; prints only the public key
    Keygen,
    /// Enroll a fresh owner-local v2 identity; does not run legacy cutover
    Identity {
        #[command(subcommand)]
        action: client_identity::Action,
    },
}
fn main() -> ExitCode {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(e) if e.kind() == clap::error::ErrorKind::DisplayHelp => {
            print!("{e}");
            return ExitCode::SUCCESS;
        }
        Err(e) => {
            print_failure(&Failure::new("usage", e.to_string(), "vega post --help"));
            return ExitCode::from(2);
        }
    };
    if cli.version {
        println!("{}", vega_build::version_line!());
        return ExitCode::SUCCESS;
    }
    if cli.skill {
        print!("{}", include_str!("../CLIENT-SKILL.md"));
        return ExitCode::SUCCESS;
    }
    let result = match cli.command {
        None | Some(Cmd::Status) => home(),
        #[cfg(windows)]
        Some(Cmd::Doctor) => core_doctor::doctor(),
        Some(Cmd::Relay { action }) => native_relay::run(action),
        Some(Cmd::Keygen) => client_key::keygen(),
        Some(Cmd::Identity { action }) => client_identity::run(action),
        Some(Cmd::Send {
            args,
            host,
            uac,
            uber,
            re,
            now,
            kind,
        }) => send_args(&args, host.as_deref(), uac.as_deref(), uber).and_then(|(to, text)| {
            let kind = mail_kind(kind.as_deref(), text)?;
            send(to, text, re.as_deref(), now, None, kind, None)
        }),
        Some(Cmd::Done {
            outcome,
            summary,
            to,
            host,
        }) => done(&outcome, &summary, to.as_deref(), host.as_deref()),
        Some(Cmd::Inbox { keep, show, max }) => match show {
            Some(id) => inbox_show(id),
            None => inbox(keep, max as usize),
        },
        Some(Cmd::Log {
            pane,
            from,
            since,
            limit,
            full,
            json,
        }) => log_view(
            pane.as_deref(),
            from.as_deref(),
            since.as_deref(),
            limit,
            full,
            json,
        ),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(f) => {
            let code = if f.code == "usage" { 2 } else { 1 };
            print_failure(&f);
            ExitCode::from(code)
        }
    }
}

async fn connect(
    role: &str,
    _: Option<tokio::sync::mpsc::UnboundedSender<()>>,
) -> Result<async_nats::Client, Failure> {
    let url = url();
    if url.is_empty() {
        return Err(Failure::new(
            "broker_unreachable",
            "no broker configured",
            "set VEGA_NATS_URL or write the first line of %APPDATA%\\vega\\nats.url",
        ));
    }
    let options = match nkey_path(Seed::Host) {
        Some(path) => {
            let seed = client_key::read_seed(std::path::Path::new(&path))?;
            async_nats::ConnectOptions::with_nkey(seed)
        }
        None => async_nats::ConnectOptions::new(),
    };
    outage::bounded("connect", async {
        options.name(format!("vega-post {role} {}",host()))
            .custom_inbox_prefix(inbox_prefix(&host()))
            .connection_timeout(vega_registry::BROKER_WAIT)
            .request_timeout(Some(vega_registry::BROKER_WAIT))
            .event_callback(|event| async move {
                if let async_nats::Event::ServerError(error) = event {
                    if error.to_string().to_lowercase().contains("permissions violation") {
                        print_failure(&Failure::new("permission_denied", error.to_string(), "ask the operator to check this host's own inbox, reply prefix and registry grants"));
                        std::process::exit(1);
                    }
                }
            }).connect(&url).await.map_err(|e| {
                let code = if e.to_string().contains("authorization violation") {
                    "auth_failed"
                } else {
                    "broker_unreachable"
                };
                Failure::new(code, e.to_string(), "check the endpoint and this host's own provisioned public key")
            })
    }).await
}
mod outage {
    use super::*;
    pub async fn bounded<T>(
        what: &str,
        future: impl std::future::Future<Output = Result<T, Failure>>,
    ) -> Result<T, Failure> {
        within(vega_registry::BROKER_WAIT, what, future).await
    }
    pub async fn within<T>(
        wait: Duration,
        what: &str,
        future: impl std::future::Future<Output = Result<T, Failure>>,
    ) -> Result<T, Failure> {
        tokio::time::timeout(wait, future)
            .await
            .unwrap_or_else(|_| {
                Err(Failure::new(
                    "broker_unreachable",
                    format!("{what}: timed out"),
                    "check the configured broker and network",
                ))
            })
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn native_guide_has_valid_frontmatter() {
        vega_plugin::assert_guide(include_str!("../CLIENT-SKILL.md"), "post");
    }
}
