//! `vega upgrade`: pull, install, moshi-hook, then the changes
//! (docs/design/upgrade.md).

use std::process::{Command, ExitCode};

use clap::error::ErrorKind;
use clap::{CommandFactory, Parser};
use vega_config::infra::Probe;
use vega_config::Ctx;
use vega_herdr::{cell, print_failure, Failure};
use vega_onboard::upgrade;

const SKILL: &str = include_str!("../upgrade/SKILL.md");

#[derive(Parser)]
#[command(
    name = "vega upgrade",
    bin_name = "vega upgrade",
    disable_version_flag = true,
    about = "Upgrade vega: pull the checkout, install, upgrade moshi-hook, show the changes",
    after_help = "Pulls the checkout this host was installed from (fast-forward only), runs `mise run install-cli`\nwhen the installed commit is not HEAD, upgrades moshi-hook where installed (brew upgrade, brew services\nrestart, moshi-hook doctor), then prints `vega onboard` of the new binary. Refuses a dirty or diverged\ncheckout and hosts that get vega by `vega deploy`. Never applies config: `vega onboard apply` does.\n\nExamples:\n  vega upgrade"
)]
struct Cli {
    /// Print the agent guide for this plugin, SKILL.md form (read by `vega skills`)
    #[arg(long)]
    skill: bool,
    /// Print the bare version
    #[arg(short = 'V', short_alias = 'v', long)]
    version: bool,
}

/// Lines of `moshi-hook doctor` shown; the rest is cut.
const DOCTOR_LINES: usize = 40;

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
    let Some(ctx) = Ctx::from_env() else {
        print_failure(&Failure::new(
            "no_home",
            "HOME is not set",
            "run it as a user with a home",
        ));
        return ExitCode::from(1);
    };
    let state = ctx.env.state.join("vega");
    let probe = Probe::from_env(ctx.env.clone());
    let report = match upgrade::run(&state, &probe) {
        Ok(r) => r,
        Err(r) => {
            print_failure(&Failure::new(r.code, r.message, &r.hint));
            return ExitCode::from(1);
        }
    };
    println!(
        "checkout: {}",
        vega_herdr::collapse_home(&report.checkout.display().to_string())
    );
    println!("steps[{}]{{step,result}}:", report.steps.len());
    for s in &report.steps {
        println!("  {},{}", s.step, cell(&s.result));
    }
    if let Some((code, text)) = &report.doctor {
        let lines: Vec<&str> = text.lines().collect();
        println!("moshi-hook doctor (exit {code}):");
        for l in lines.iter().take(DOCTOR_LINES) {
            println!("  {l}");
        }
        if lines.len() > DOCTOR_LINES {
            println!(
                "  ... {} more lines: moshi-hook doctor",
                lines.len() - DOCTOR_LINES
            );
        }
    }
    let vega_ok = report
        .steps
        .iter()
        .filter(|s| s.step == "pull" || s.step == "install")
        .all(|s| s.ok);
    if vega_ok {
        // The new binary embeds the new changelog; this process has the old one.
        let onboard = ctx.env.home.join(".local/bin/vega-onboard");
        let bin = if onboard.exists() {
            onboard.into_os_string()
        } else {
            "vega-onboard".into()
        };
        match Command::new(&bin).output() {
            Ok(o) => print!("{}", String::from_utf8_lossy(&o.stdout)),
            Err(e) => println!("onboard: vega onboard failed to start: {e}"),
        }
    }
    if report.failed() {
        print_failure(&Failure::new(
            "step_failed",
            format!(
                "{} step(s) failed",
                report.steps.iter().filter(|s| !s.ok).count()
            ),
            "see the steps above; fix the cause and rerun vega upgrade",
        ));
        return ExitCode::from(1);
    }
    println!("help: summarise the changes for the user, act on migration entries, then vega onboard apply on their OK");
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skill_has_valid_frontmatter() {
        vega_plugin::assert_guide(SKILL, "upgrade");
    }
}
