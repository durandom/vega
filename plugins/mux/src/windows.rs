// Remote target only: policy and terminal creation stay on the Unix coordinator.
mod spawn_record;

use clap::{CommandFactory, Parser, Subcommand};
use serde_json::json;
use std::{
    env,
    path::{Path, PathBuf},
    process::ExitCode,
};
use vega_herdr::{print_failure, Failure};

#[derive(Parser)]
#[command(name = "vega mux", bin_name = "vega mux", disable_version_flag = true,
    about = "Native Windows remote launch preparation and records",
    after_help = include_str!("../help/windows.md"))]
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
    /// Prepare a native executable and existing repository trust (JSON)
    Prepare {
        #[arg(long, value_parser = ["codex", "claude"])]
        kind: String,
        #[arg(long)]
        cwd: String,
        #[arg(long)]
        worktree: Option<String>,
        #[arg(long, requires = "worktree")]
        base: Option<String>,
    },
    /// Persist a coordinator's launch state and parent lineage (internal JSON)
    #[command(hide = true)]
    RecordSpawn { payload: String },
}
fn main() -> ExitCode {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(e) if e.kind() == clap::error::ErrorKind::DisplayHelp => {
            let _ = e.print();
            return ExitCode::SUCCESS;
        }
        Err(e) => {
            let message = e
                .to_string()
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
            let sub = env::args().nth(1);
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
        print!("{}", include_str!("../WINDOWS.md"));
        return ExitCode::SUCCESS;
    }
    let result = match cli.command {
        Some(Cmd::Prepare {
            kind,
            cwd,
            worktree,
            ..
        }) => prepare(&kind, &cwd, worktree.is_some()),
        Some(Cmd::RecordSpawn { payload }) => spawn_record::receive(&payload),
        None => {
            println!("description: Native Windows mux remote target\ncommands: prepare, record-spawn\nhelp: Run `vega mux --help`; spawn from a Unix coordinator");
            Ok(())
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(f) => {
            vega_herdr::print_failure_json(&f);
            ExitCode::from(if f.code == "usage" || f.code == "unsupported" {
                2
            } else {
                1
            })
        }
    }
}

fn prepare(kind: &str, cwd: &str, worktree: bool) -> Result<(), Failure> {
    if worktree {
        return Err(Failure::new(
            "unsupported",
            "Windows targets do not support --worktree",
            "create the checkout separately and spawn with --cwd <absolute Windows path>",
        ));
    }
    if !Path::new(cwd).is_absolute() || !Path::new(cwd).is_dir() {
        return Err(Failure::new(
            "usage",
            "prepare requires an existing local absolute cwd",
            "pass the target's absolute checkout path, for example G:\\src\\repo",
        ));
    }
    let executable = resolve_exe(kind).ok_or_else(|| {
        Failure::new(
            "executable_not_found",
            format!("cannot find {kind}.exe on the target PATH"),
            "make the native harness .exe available on the target's hc environment PATH",
        )
    })?;
    println!(
        "{}",
        json!({"schema":1,"platform":"windows","executable":executable,
        "cwd":cwd,"how":"","trust":vega_herdr::launch::apply(kind, cwd)})
    );
    Ok(())
}

// Only native .exe files: cmd/bat shims need a shell and change argv semantics.
fn resolve_exe(kind: &str) -> Option<PathBuf> {
    env::split_paths(&env::var_os("PATH")?)
        .filter(|p| p.is_absolute())
        .map(|p| p.join(format!("{kind}.exe")))
        .find(|p| p.is_file())
}

#[cfg(test)]
mod tests {
    #[test]
    fn native_guide_is_valid() {
        vega_plugin::assert_guide(include_str!("../WINDOWS.md"), "mux");
    }
}
