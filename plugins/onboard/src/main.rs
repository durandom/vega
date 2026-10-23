//! `vega onboard`: first run and upgrade of a host (docs/design/onboarding.md).

use std::path::Path;
use std::process::ExitCode;

use clap::error::ErrorKind;
use clap::{CommandFactory, Parser, Subcommand};
use vega_config::infra::{self, Level, Probe, Row};
use vega_config::{Ctx, Outcome, Unit};
use vega_herdr::{cell, print_failure, Failure};
use vega_onboard::{Block, Done, Mode, Step};

const SKILL: &str = include_str!("../SKILL.md");

#[derive(Parser)]
#[command(
    name = "vega onboard",
    bin_name = "vega onboard",
    disable_version_flag = true,
    args_conflicts_with_subcommands = true,
    about = "Onboard this host to vega; after an upgrade, what changed since the last look",
    after_help = "Without a command: mode (first run, upgrade, current), the changes since the last seen changelog block,\nthe required and optional infra with next steps, and what apply would do per config unit. Never writes.\n\nExamples:\n  vega onboard\n  vega onboard apply\n  vega onboard changes --all"
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
    /// Write the default config (never over your own files) and record the changelog as seen
    #[command(
        after_help = "Hooks and skills always; whole files (CLAUDE.md, AGENTS.md, output styles, herdr's config.toml,\ndcg.config and dcg.packs) and Claude's settings keys only where you have none: yours are `kept`.\nRecords the newest changelog block in $XDG_STATE_HOME/vega/onboard.json.\nExit 1 when a unit failed (the block is recorded anyway).\n\nExamples:\n  vega onboard apply"
    )]
    Apply,
    /// Changelog entries since the last seen block
    Changes {
        /// Every block, not only the unseen ones
        #[arg(long)]
        all: bool,
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
                "valid usage below",
            ));
            print!("{}", Cli::command().render_help());
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
    let result = Ctx::from_env()
        .ok_or_else(|| Failure::new("no_home", "HOME is not set", "run it as a user with a home"))
        .and_then(|ctx| match cli.cmd {
            None => home(&ctx),
            Some(Cmd::Apply) => apply(&ctx),
            Some(Cmd::Changes { all }) => changes(&ctx, all),
        });
    match result {
        Ok(code) => code,
        Err(f) => {
            print_failure(&f);
            ExitCode::from(if f.code == "usage" { 2 } else { 1 })
        }
    }
}

fn blocks() -> Result<Vec<Block>, Failure> {
    let b = vega_onboard::parse(vega_onboard::CHANGELOG).map_err(|e| {
        Failure::new(
            "changelog_invalid",
            e,
            "the embedded CHANGELOG.md is broken; report it",
        )
    })?;
    Ok(b)
}

fn print_changes(list: &[&Block]) {
    let n: usize = list.iter().map(|b| b.entries.len()).sum();
    println!("changes[{n}]{{block,date,kind,change}}:");
    for b in list {
        for e in &b.entries {
            println!("  {},{},{},{}", b.n, b.date, e.kind, cell(&e.text));
        }
    }
}

fn print_infra(rows: &[Row], level: Level) {
    let of: Vec<&Row> = rows.iter().filter(|r| r.component.level == level).collect();
    match level {
        Level::Required => {
            println!("required[{}]{{component,state,next}}:", of.len());
            for r in of {
                println!("  {},{},{}", r.name(), cell(&r.state), cell(r.next()));
            }
        }
        Level::Optional => {
            println!("optional[{}]{{component,state,enables,next}}:", of.len());
            for r in of {
                println!(
                    "  {},{},{},{}",
                    r.name(),
                    cell(&r.state),
                    cell(r.component.enables),
                    cell(r.next())
                );
            }
        }
    }
}

fn managed(ctx: &Ctx) -> Result<std::collections::BTreeSet<std::path::PathBuf>, Failure> {
    vega_config::chezmoi_managed(&ctx.chezmoi).map_err(|e| {
        Failure::new(
            "chezmoi_failed",
            e,
            "fix chezmoi (vega asks it which targets it manages), then rerun",
        )
    })
}

/// What apply would do with a unit, for the home view.
fn plan_cell(
    ctx: &Ctx,
    u: Unit,
    managed: &std::collections::BTreeSet<std::path::PathBuf>,
) -> String {
    let plan = match vega_config::plan(ctx, u) {
        Ok(p) => p,
        Err(e) => return format!("error: {e}"),
    };
    let m = vega_config::managed_targets(&plan, managed);
    let state = vega_config::state(&plan, &m);
    match vega_onboard::decide(ctx, u) {
        Ok(Step::Keep(why)) => format!(
            "keep: {why} (vega config diff {0}; vega config apply {0} takes the default)",
            u.name()
        ),
        Ok(Step::Apply) if state == "ok" => "ok".into(),
        Ok(Step::Apply) if state.starts_with("skipped") || state.starts_with("managed") => state,
        Ok(Step::Apply) => format!("write ({state})"),
        Err(e) => format!("error: {e}"),
    }
}

fn home(ctx: &Ctx) -> Result<ExitCode, Failure> {
    let b = blocks()?;
    let newest = vega_onboard::newest(&b);
    let seen = vega_onboard::seen(&ctx.env);
    println!("host: {}", vega_herdr::state::post_host());
    println!("vega: {}", vega_build::version_line!());
    match vega_onboard::mode(seen, newest) {
        Mode::FirstRun => println!(
            "mode: first run (changelog block {newest}; vega onboard changes --all lists them)"
        ),
        Mode::Upgrade { from, to } => {
            println!("mode: upgrade (last seen block {from}, now {to})");
            print_changes(&vega_onboard::since(&b, from));
        }
        Mode::Current(n) => println!("mode: current (changes: none since block {n})"),
    }
    let probe = Probe::from_env(ctx.env.clone());
    let rows = infra::check(&probe, ctx);
    println!("infra: {}", infra::summary(&rows));
    print_infra(&rows, Level::Required);
    print_infra(&rows, Level::Optional);
    let managed = managed(ctx)?;
    println!("config[{}]{{unit,apply}}:", Unit::ALL.len());
    for u in Unit::ALL {
        // The plan cannot tell; apply would skip it.
        let text = if u == Unit::Herdr && !rows.iter().any(|r| r.name() == "herdr" && r.ok) {
            "skipped: herdr not installed".into()
        } else {
            plan_cell(ctx, u, &managed)
        };
        println!("  {},{}", u.name(), cell(&text));
    }
    let broker = probe.broker().is_some();
    println!(
        "{}",
        vega_onboard::vegad_line(vega_onboard::vegad_installed(&ctx.env), broker)
    );
    let required_gaps = rows
        .iter()
        .any(|r| r.component.level == Level::Required && !r.ok);
    if required_gaps {
        println!("help: install the required parts first (next column), then vega onboard apply");
    } else if seen != Some(newest) {
        println!(
            "help: vega onboard apply writes the config above and records block {newest} as seen"
        );
    } else {
        println!(
            "help: optional parts as needed (next column); vega config shows the config units"
        );
    }
    Ok(ExitCode::SUCCESS)
}

fn tilde(ctx: &Ctx, p: &str) -> String {
    match Path::new(p).strip_prefix(&ctx.env.home) {
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => p.to_string(),
    }
}

fn apply(ctx: &Ctx) -> Result<ExitCode, Failure> {
    let b = blocks()?;
    let newest = vega_onboard::newest(&b);
    let managed = managed(ctx)?;
    let done = vega_onboard::apply(ctx, &managed);
    println!("results[{}]{{unit,result}}:", done.len());
    let mut failed = 0;
    let mut kept = 0;
    for (u, d) in &done {
        let text = match d {
            Done::Kept(why) => {
                kept += 1;
                format!("kept: {why} (vega config diff {})", u.name())
            }
            Done::Config(Outcome::Written(ps)) => format!(
                "written: {}",
                ps.iter()
                    .map(|p| tilde(ctx, p))
                    .collect::<Vec<_>>()
                    .join("; ")
            ),
            Done::Config(Outcome::Unchanged) => "unchanged".into(),
            Done::Config(Outcome::Managed(_)) => {
                format!("managed by chezmoi: left alone (vega config {})", u.name())
            }
            Done::Config(Outcome::Skipped(r)) => format!("skipped: {r}"),
            Done::Config(Outcome::Rejected(l)) => {
                failed += 1;
                format!("rejected by herdr config check: {}", l.join("; "))
            }
            Done::Config(Outcome::Failed(e)) => {
                failed += 1;
                format!("failed: {e}")
            }
        };
        println!("  {},{}", u.name(), cell(&text));
    }
    vega_onboard::record(&ctx.env, newest, env!("VEGA_VERSION"))
        .map_err(|e| Failure::new("state_failed", e, "check $XDG_STATE_HOME/vega is writable"))?;
    println!("seen: changelog block {newest}");
    println!("summary: kept {kept}, failed {failed}");
    if failed > 0 {
        return Err(Failure::new(
            "apply_failed",
            format!("{failed} unit(s) failed or were rejected"),
            "see the results above; vega config shows each unit",
        ));
    }
    if kept > 0 {
        println!("help: kept units are yours; vega config diff <unit> shows the default");
    }
    Ok(ExitCode::SUCCESS)
}

fn changes(ctx: &Ctx, all: bool) -> Result<ExitCode, Failure> {
    let b = blocks()?;
    let seen = vega_onboard::seen(&ctx.env).unwrap_or(0);
    let list = vega_onboard::since(&b, if all { 0 } else { seen });
    if list.is_empty() {
        println!("changes: none since block {seen} (vega onboard changes --all lists every block)");
    } else {
        print_changes(&list);
    }
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skill_has_valid_frontmatter() {
        vega_plugin::assert_guide(SKILL, "onboard");
    }
}
