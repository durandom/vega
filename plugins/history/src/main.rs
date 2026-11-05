use clap::{Parser, Subcommand};
use std::io::Write;
use std::path::PathBuf;
use vega_herdr::{cell, print_failure, Failure};
use vega_history::{failure, Filter};
const HELP: &str = include_str!("../help/history.md");
const SKILL: &str = include_str!("../SKILL.md");

#[derive(Parser)]
#[command(disable_version_flag = true, disable_help_subcommand = true)]
struct Cli {
    #[command(subcommand)]
    command: Option<Cmd>,
}
#[derive(Subcommand)]
enum Cmd {
    Status,
    Read(Options),
    Export(Options),
}
#[derive(clap::Args)]
struct Options {
    #[command(flatten)]
    filter: Filter,
    #[arg(long)]
    viewer_key: Option<PathBuf>,
}
fn main() -> std::process::ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() == 1 && matches!(args[0].as_str(), "--version" | "-v" | "-V") {
        println!("{}", vega_build::version_line!());
        return std::process::ExitCode::SUCCESS;
    }
    if args == ["--skill"] {
        print!("{SKILL}");
        return std::process::ExitCode::SUCCESS;
    }
    let result = match Cli::try_parse() {
        Err(e) if e.kind() == clap::error::ErrorKind::DisplayHelp => {
            print!("{HELP}");
            return std::process::ExitCode::SUCCESS;
        }
        Err(e) => Err(Failure::new("usage", e.to_string(), HELP)),
        Ok(cli) => match cli.command {
            None => home(),
            Some(Cmd::Status) => status(),
            Some(Cmd::Read(opts)) => run(opts, false),
            Some(Cmd::Export(opts)) => run(opts, true),
        },
    };
    match result {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            print_failure(&e);
            std::process::ExitCode::from(if e.code == "usage" { 2 } else { 1 })
        }
    }
}
fn home() -> Result<(), Failure> {
    let bin = std::env::current_exe().map_err(|e| failure("binary", e))?;
    println!(
        "bin: {}",
        cell(&vega_herdr::collapse_home(&bin.display().to_string()))
    );
    println!("description: Read central event history and inspect local publication evidence");
    status()?;
    println!("help[2]:\n  vega history read --viewer-key <path>\n  vega history export --viewer-key <path> --bead <id>");
    Ok(())
}
fn status() -> Result<(), Failure> {
    let status_error = |error: String| {
        Failure::new("history_status", error, "inspect this host's ~/.local/state/vega/history-outbox/<host> (or XDG_STATE_HOME); repair publish.state or directory permissions")
    };
    let dir =
        vega_herdr::history::root().ok_or_else(|| status_error("no state directory".into()))?;
    let state = vega_herdr::history::read_status(&dir).map_err(|e| status_error(e.to_string()))?;
    let pending = vega_herdr::history::pending(&dir)
        .map_err(|e| status_error(e.to_string()))?
        .len();
    println!("history{{host,last_acked_ms,last_acked_seq,pending,last_error}}:");
    println!(
        "  {},{},{},{},{}",
        cell(&vega_herdr::state::post_host()),
        state
            .last_acked_ms
            .map(|v| v.to_string())
            .unwrap_or("null".into()),
        state
            .last_acked_seq
            .map(|v| v.to_string())
            .unwrap_or("null".into()),
        pending,
        state
            .last_error
            .as_deref()
            .map(cell)
            .unwrap_or("null".into())
    );
    Ok(())
}
fn run(opts: Options, export: bool) -> Result<(), Failure> {
    let key = opts
        .viewer_key
        .or_else(|| {
            vega_herdr::service_config::get("VEGA_HISTORY_VIEWER_KEY", "history.viewer-key")
                .map(PathBuf::from)
        })
        .ok_or_else(|| failure("viewer_key", "a separate viewer seed path is required"))?;
    let url = vega_herdr::service_config::get("VEGA_NATS_URL", "nats.url")
        .ok_or_else(|| failure("history_broker", "no broker configured"))?;
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| failure("runtime", e))?;
    rt.block_on(async {
        let client = vega_history::connect(&url, &key).await?;
        let limit = opts
            .filter
            .limit
            .unwrap_or(if export { u64::MAX } else { 100 });
        let mut rows = Vec::new();
        let mut emitted = 0;
        let mut out = std::io::BufWriter::new(std::io::stdout().lock());
        let (total, end) = vega_history::scan(client, &opts.filter, |seq, e| {
            if emitted < limit {
                if export {
                    serde_json::to_writer(&mut out, &e).map_err(|e| failure("output", e))?;
                    writeln!(out).map_err(|e| failure("output", e))?;
                } else {
                    rows.push(format!(
                        "  {seq},{},{},{},{}",
                        e["ts"],
                        cell(e["host"].as_str().unwrap()),
                        cell(e["kind"].as_str().unwrap()),
                        cell(e["id"].as_str().unwrap())
                    ));
                }
                emitted += 1;
            }
            Ok(())
        })
        .await?;
        if export {
            out.flush().map_err(|e| failure("output", e))?;
            eprintln!(
                "history: {emitted} of {total} matching events exported through sequence {end}"
            );
        } else {
            println!(
                "count: {} of {total} matching events (snapshot through sequence {end})",
                rows.len()
            );
            println!("events[{}]{{seq,ts,host,kind,id}}:", rows.len());
            for row in rows {
                println!("{row}");
            }
            println!(
                "help: vega history export --viewer-key {}{}",
                cell(&key.display().to_string()),
                scope(&opts.filter)
            );
            if total > emitted {
                println!("help: increase --limit <count> to see more matching events");
            }
        }
        Ok(())
    })
}
fn scope(f: &Filter) -> String {
    let mut args = vec![];
    for (key, value) in [
        ("project", &f.project),
        ("bead", &f.bead),
        ("host", &f.host),
    ] {
        if let Some(value) = value {
            args.push(format!(" --{key} {}", cell(value)));
        }
    }
    if let Some(since) = f.since {
        args.push(format!(" --since {since}"));
    }
    if f.after > 0 {
        args.push(format!(" --after {}", f.after));
    }
    args.concat()
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn guide() {
        vega_plugin::assert_guide(SKILL, "history");
        <Cli as clap::CommandFactory>::command().debug_assert();
    }
    #[test]
    fn rejects_unknown_flags_and_invalid_values() {
        for args in [
            vec!["history", "status", "--host", "other"],
            vec!["history", "read", "--snce", "1"],
            vec!["history", "export", "--limit", "0"],
            vec!["history", "read", "--since", "yesterday"],
        ] {
            assert!(Cli::try_parse_from(args).is_err());
        }
    }
}
