//! `vega config`: status, diff, apply and print of the config vega owns
//! (docs/design/config.md).

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::error::ErrorKind;
use clap::{CommandFactory, Parser, Subcommand};
use vega_config::{infra, sources, Ctx, Env, Outcome, Unit};
use vega_herdr::{cell, print_failure, Failure};

const SKILL: &str = include_str!("../SKILL.md");

#[derive(Parser)]
#[command(
    name = "vega config",
    bin_name = "vega config",
    disable_version_flag = true,
    args_conflicts_with_subcommands = true,
    about = "The coding agents', herdr's and dcg's config, owned by vega: status, diff, apply",
    after_help = "Without a command: each unit's target, ownership and state, and the local sources.\nService endpoints: first-line files under $XDG_CONFIG_HOME/vega (normally ~/.config/vega):\nnats.url, bd.remote, bd.sql, janitor-embed.url. Environment overrides win; no endpoint defaults.\nSee docs/design/config.md, Service endpoints, before upgrading existing estate hosts.\n\nExamples:\n  vega config                 # status\n  vega config diff codex.md   # what apply would change\n  vega config apply           # every unit\n  vega config apply hooks     # the agent hooks only\n  vega config init instructions/vega.md"
)]
struct Cli {
    /// Print the agent guide for this plugin, SKILL.md form (read by `vega skills`)
    #[arg(long)]
    skill: bool,
    /// Print the bare version
    #[arg(short = 'V', short_alias = 'v', long)]
    version: bool,
    /// Units or groups to show (default: all)
    units: Vec<String>,
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Unified diff of every target apply would change
    Diff {
        /// Units or groups (default: all)
        units: Vec<String>,
    },
    /// Write the units (default: all); a unit chezmoi manages is left alone
    #[command(
        after_help = "Results: written, unchanged, managed (chezmoi owns a target: nothing written), skipped (tool not set up here), rejected (herdr config check), failed. Exit 1 on rejected or failed.\nThe last line is `summary: written N, unchanged N, managed N, skipped N, failed N`.\n\nExamples:\n  vega config apply\n  vega config apply herdr\n  vega config apply hooks"
    )]
    Apply {
        /// Units or groups (default: all)
        units: Vec<String>,
    },
    /// Print the rendered file of a single-file unit
    Print { unit: String },
    /// Copy a repo default to ~/.config/vega/config/ for local editing
    #[command(
        after_help = "Sources: herdr/config.toml, instructions/<part>.md, claude/output-styles/<name>.md,\nclaude/settings.json, agentsview/LOCAL-SETUP.md, dcg/config.toml, dcg/packs/oc.yaml. A local file that exists is never overwritten. With nonstandard XDG_CONFIG_HOME, edit dcg/config.toml packs.custom_paths to point at that dcg/packs directory after init (init copies literal defaults).\n\nExamples:\n  vega config init instructions/vega.md\n  vega config init claude/settings.json"
    )]
    Init { source: String },
    /// The infra vega needs on this host: level, state, what it enables, next step
    #[command(
        after_help = "Local checks only (PATH without the shims, env vars, config files); nothing touches the network.\nRequired: the core (agents in herdr panes, guides, config). Optional: one feature set each.\n\nExamples:\n  vega config infra"
    )]
    Infra,
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
    let result = ctx().and_then(|ctx| match cli.cmd {
        None => status(&ctx, &cli.units),
        Some(Cmd::Diff { units }) => diff(&ctx, &units),
        Some(Cmd::Apply { units }) => apply(&ctx, &units),
        Some(Cmd::Print { unit }) => print(&ctx, &unit),
        Some(Cmd::Init { source }) => init(&ctx, &source),
        Some(Cmd::Infra) => infra(&ctx),
    });
    match result {
        Ok(code) => code,
        Err(f) => {
            print_failure(&f);
            ExitCode::from(if f.code == "usage" { 2 } else { 1 })
        }
    }
}

fn ctx() -> Result<Ctx, Failure> {
    Ctx::from_env()
        .ok_or_else(|| Failure::new("no_home", "HOME is not set", "run it as a user with a home"))
}

fn units(names: &[String]) -> Result<Vec<Unit>, Failure> {
    vega_config::select(names).map_err(|e| Failure::new("usage", e, "vega config lists the units"))
}

fn managed(ctx: &Ctx) -> Result<BTreeSet<PathBuf>, Failure> {
    vega_config::chezmoi_managed(&ctx.chezmoi).map_err(|e| {
        Failure::new(
            "chezmoi_failed",
            e,
            "fix chezmoi (vega asks it which targets it still manages), then rerun",
        )
    })
}

fn render_failed(e: String) -> Failure {
    Failure::new(
        "render_failed",
        e,
        "fix the source in ~/.config/vega/config/ (or the repo's config/)",
    )
}

fn tilde(env: &Env, p: &Path) -> String {
    match p.strip_prefix(&env.home) {
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => p.display().to_string(),
    }
}

fn status(ctx: &Ctx, names: &[String]) -> Result<ExitCode, Failure> {
    let us = units(names)?;
    let managed = managed(ctx)?;
    println!("host: {}", vega_herdr::state::post_host());
    println!("units[{}]{{unit,target,ownership,state}}:", us.len());
    let mut notes = Vec::new();
    let mut drift = false;
    for u in &us {
        let plan = vega_config::plan(ctx, *u).map_err(render_failed)?;
        let m = vega_config::managed_targets(&plan, &managed);
        let st = vega_config::state(&plan, &m);
        drift |= st.starts_with("differs") || st.starts_with("missing");
        let st = if vega_config::has_local(&plan) {
            format!("{st}, local source")
        } else {
            st
        };
        println!(
            "  {},{},{},{}",
            u.name(),
            cell(u.target()),
            cell(u.ownership()),
            cell(&st)
        );
        for p in &m {
            notes.push((
                u.name(),
                format!("chezmoi manages {}: apply leaves it", tilde(&ctx.env, p)),
            ));
        }
        for p in vega_config::skills_overlap(&plan, &managed, &ctx.env.home) {
            notes.push((
                u.name(),
                format!(
                    "chezmoi also manages {}: the dotfiles drop it",
                    tilde(&ctx.env, &p)
                ),
            ));
        }
        notes.extend(plan.notes.iter().map(|n| (u.name(), n.clone())));
    }
    if !notes.is_empty() {
        println!("notes[{}]{{unit,note}}:", notes.len());
        for (u, n) in &notes {
            println!("  {u},{}", cell(n));
        }
    }
    let local = sources::local_files(&ctx.env);
    if local.is_empty() {
        println!(
            "sources: {} repo defaults, none local (vega config init <source> to edit one)",
            sources::DEFAULTS.len()
        );
    } else {
        println!("local[{}]{{source,use}}:", local.len());
        for l in &local {
            let used = sources::default(l).is_some()
                || (l.starts_with(&format!("{}/", sources::STYLES_DIR)) && l.ends_with(".md"))
                || (l.starts_with("dcg/packs/") && (l.ends_with(".yaml") || l.ends_with(".yml")));
            println!(
                "  {},{}",
                cell(l),
                if used {
                    "overrides"
                } else {
                    "unused (no such source)"
                }
            );
        }
    }
    let rows = infra::check(&infra::Probe::from_env(ctx.env.clone()), ctx);
    println!("infra: {} (vega config infra)", infra::summary(&rows));
    if drift {
        println!("help: vega config diff shows the changes; vega config apply writes them");
    }
    Ok(ExitCode::SUCCESS)
}

fn infra(ctx: &Ctx) -> Result<ExitCode, Failure> {
    let rows = infra::check(&infra::Probe::from_env(ctx.env.clone()), ctx);
    println!("infra: {}", infra::summary(&rows));
    println!(
        "components[{}]{{component,level,state,enables}}:",
        rows.len()
    );
    for r in &rows {
        println!(
            "  {},{},{},{}",
            r.name(),
            r.component.level.as_str(),
            cell(&r.state),
            cell(r.component.enables)
        );
    }
    let gaps: Vec<_> = rows.iter().filter(|r| !r.ok).collect();
    if gaps.is_empty() {
        println!("gaps: none");
    } else {
        println!("gaps[{}]{{component,next}}:", gaps.len());
        for r in gaps {
            println!("  {},{}", r.name(), cell(r.next()));
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn diff(ctx: &Ctx, names: &[String]) -> Result<ExitCode, Failure> {
    let us = units(names)?;
    let managed = managed(ctx)?;
    let mut changed = 0;
    for u in &us {
        let plan = vega_config::plan(ctx, *u).map_err(render_failed)?;
        if plan.skip.is_some() {
            continue;
        }
        let m = vega_config::managed_targets(&plan, &managed);
        for f in plan.files.iter().filter(|f| f.changed()) {
            changed += 1;
            let label = tilde(&ctx.env, &f.path);
            let held = if m.contains(&f.path) {
                " (managed by chezmoi: apply leaves it)"
            } else {
                ""
            };
            println!("unit: {} {label}{held}", u.name());
            if f.link && !f.follow {
                println!("note: a symlink now; apply replaces it with a file");
            }
            print!(
                "{}",
                unified(
                    f.live.as_deref().unwrap_or(""),
                    &f.text,
                    &label,
                    f.live.is_none()
                )
            );
        }
        if let Some((_, missing)) = &plan.skills {
            if !missing.is_empty() {
                changed += 1;
                println!("unit: skills");
                println!(
                    "missing: {} (apply runs vega skills lib sync)",
                    missing.join(" ")
                );
            }
        }
    }
    if changed == 0 {
        println!("diff: none (every selected unit is up to date)");
    }
    Ok(ExitCode::SUCCESS)
}

/// `diff -u` of two texts.
fn unified(old: &str, new: &str, label: &str, missing: bool) -> String {
    use std::io::Write;
    use std::process::{Command, Stdio};
    let dir = std::env::temp_dir().join(format!("vega-config-diff.{}", std::process::id()));
    if std::fs::create_dir_all(&dir).is_err() {
        return "(diff not available)\n".into();
    }
    let a = dir.join("live");
    let _ = std::fs::write(&a, old);
    let live_label = if missing {
        "/dev/null".to_string()
    } else {
        format!("live {label}")
    };
    let child = Command::new("diff")
        .args([
            "-u",
            "--label",
            &live_label,
            "--label",
            &format!("vega {label}"),
        ])
        .arg(&a)
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn();
    let out = match child {
        Ok(mut c) => {
            if let Some(mut stdin) = c.stdin.take() {
                let _ = stdin.write_all(new.as_bytes());
            }
            c.wait_with_output()
                .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
                .unwrap_or_default()
        }
        Err(_) => "(diff not available)\n".into(),
    };
    let _ = std::fs::remove_dir_all(&dir);
    out
}

fn apply(ctx: &Ctx, names: &[String]) -> Result<ExitCode, Failure> {
    let us = units(names)?;
    let managed = managed(ctx)?;
    println!("results[{}]{{unit,result}}:", us.len());
    let mut counts: Vec<(&str, usize)> = [
        "written",
        "unchanged",
        "managed",
        "skipped",
        "rejected",
        "failed",
    ]
    .iter()
    .map(|k| (*k, 0))
    .collect();
    let mut issues = Vec::new();
    for u in &us {
        let o = vega_config::apply(ctx, *u, &managed);
        if let Some(c) = counts.iter_mut().find(|(k, _)| *k == o.kind()) {
            c.1 += 1;
        }
        let text = match &o {
            Outcome::Written(d) => format!(
                "written: {}",
                d.iter()
                    .map(|p| tilde(&ctx.env, Path::new(p)))
                    .collect::<Vec<_>>()
                    .join("; ")
            ),
            Outcome::Unchanged => "unchanged".into(),
            Outcome::Managed(ps) => format!(
                "managed by chezmoi: {} (the dotfiles drop it, then vega config apply {})",
                ps.iter()
                    .map(|p| tilde(&ctx.env, p))
                    .collect::<Vec<_>>()
                    .join(" "),
                u.name()
            ),
            Outcome::Skipped(r) => format!("skipped: {r}"),
            Outcome::Rejected(lines) => {
                issues.extend(lines.iter().map(|l| (u.name(), l.clone())));
                "rejected by herdr config check (the old file stays)".into()
            }
            Outcome::Failed(e) => format!("failed: {e}"),
        };
        println!("  {},{}", u.name(), cell(&text));
    }
    if !issues.is_empty() {
        println!("issues[{}]{{unit,issue}}:", issues.len());
        for (u, l) in &issues {
            println!("  {u},{}", cell(l));
        }
    }
    let failed = counts
        .iter()
        .filter(|(k, _)| matches!(*k, "rejected" | "failed"))
        .map(|(_, n)| n)
        .sum::<usize>();
    println!(
        "summary: {}",
        counts
            .iter()
            .map(|(k, n)| format!("{k} {n}"))
            .collect::<Vec<_>>()
            .join(", ")
    );
    if failed > 0 {
        return Err(Failure::new(
            "apply_failed",
            format!("{failed} unit(s) failed or were rejected"),
            "see the results above; the other units are applied",
        ));
    }
    Ok(ExitCode::SUCCESS)
}

fn print(ctx: &Ctx, name: &str) -> Result<ExitCode, Failure> {
    let us = units(&[name.to_string()])?;
    let [u] = us.as_slice() else {
        return Err(Failure::new(
            "usage",
            format!("{name} is a group"),
            "name one unit",
        ));
    };
    let plan = vega_config::plan(ctx, *u).map_err(render_failed)?;
    if let Some(r) = &plan.skip {
        return Err(Failure::new(
            "skipped",
            format!("{name}: {r}"),
            "nothing to render on this host",
        ));
    }
    match plan.files.as_slice() {
        [f] => {
            print!("{}", f.text);
            Ok(ExitCode::SUCCESS)
        }
        _ => Err(Failure::new(
            "usage",
            format!("{name} renders {} files", plan.files.len()),
            "vega config diff shows them",
        )),
    }
}

fn init(ctx: &Ctx, source: &str) -> Result<ExitCode, Failure> {
    let p = sources::init(&ctx.env, source).map_err(|e| {
        Failure::new(
            "init_failed",
            e,
            "sources: herdr/config.toml, instructions/<part>.md, claude/output-styles/<name>.md, claude/settings.json, agentsview/LOCAL-SETUP.md, dcg/config.toml, dcg/packs/oc.yaml; nonstandard XDG_CONFIG_HOME: adjust dcg packs.custom_paths after init",
        )
    })?;
    println!(
        "source: {} (copied from the repo default)",
        tilde(&ctx.env, &p)
    );
    println!("help: edit it, then vega config diff; vega config apply writes the targets");
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skill_has_valid_frontmatter() {
        vega_plugin::assert_guide(SKILL, "config");
    }
}
