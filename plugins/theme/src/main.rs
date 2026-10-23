//! `vega theme`: the OS dark/light mode for every TUI (docs/design/theme.md).
//!
//! `vega theme` shows the OS mode, the wanted and applied mode and each
//! tool's setting with its drift; `vega theme dark|light|auto` sets the
//! preference and applies it; `vega theme apply` applies the due mode again.
//! vegad does the same on every OS change and types `/config theme=<mode>`
//! into idle Claude panes (plugins/post/src/theme.rs).

use std::process::ExitCode;
use std::time::Duration;

use clap::error::ErrorKind;
use clap::{CommandFactory, Parser, Subcommand};
use vega_herdr::{cell, local_time, print_failure, Failure};
use vega_theme_sync::adapters::{self, Current, Outcome};
use vega_theme_sync::{os, prefs, resolve, state, Env, Mode, Pref, Source};

const SKILL: &str = include_str!("../SKILL.md");

#[derive(Parser)]
#[command(
    name = "vega theme",
    bin_name = "vega theme",
    disable_version_flag = true,
    about = "One dark/light mode for every TUI, from the OS (or the control host)",
    after_help = "Without a command: OS mode, wanted and applied mode, each tool's setting and drift.\n\nExamples:\n  vega theme              # status\n  vega theme dark         # fix dark and apply\n  vega theme auto         # follow the OS again\n  vega theme apply        # apply the due mode (idempotent)"
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
    /// Fix dark mode in theme.toml and apply it
    Dark,
    /// Fix light mode in theme.toml and apply it
    Light,
    /// Follow the OS (headless: the control host) and apply
    Auto,
    /// Apply the due mode to every tool again (idempotent)
    Apply,
    /// Moved: alias for `vega config diff|print|apply herdr`
    #[command(
        after_help = "Moved to `vega config` (unit herdr). This alias runs:\n  vega theme herdr            -> vega config diff herdr\n  vega theme herdr --print    -> vega config print herdr\n  vega theme herdr --apply    -> vega config apply herdr"
    )]
    Herdr {
        /// Print the rendered file to stdout
        #[arg(long, conflicts_with = "apply")]
        print: bool,
        /// Check, write and reload
        #[arg(long)]
        apply: bool,
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
    // Moved to `vega config` (vega-lhc.107); kept as a thin alias.
    if let Some(Cmd::Herdr { print, apply }) = cli.cmd {
        let action = if print {
            "print"
        } else if apply {
            "apply"
        } else {
            "diff"
        };
        return vega_plugin::run_plugin("config", &[action, "herdr"]);
    }
    let result = Env::from_env()
        .ok_or_else(|| Failure::new("no_home", "HOME is not set", "run it as a user with a home"))
        .and_then(|env| match cli.cmd {
            None => home(&env),
            Some(Cmd::Dark) => set(&env, Pref::Fixed(Mode::Dark)),
            Some(Cmd::Light) => set(&env, Pref::Fixed(Mode::Light)),
            Some(Cmd::Auto) => set(&env, Pref::Auto),
            Some(Cmd::Apply) => apply(&env),
            Some(Cmd::Herdr { .. }) => unreachable!("handled above"),
        });
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(f) => {
            print_failure(&f);
            ExitCode::from(if f.code == "usage" { 2 } else { 1 })
        }
    }
}

fn load(env: &Env) -> Result<prefs::Prefs, Failure> {
    prefs::load(env).map_err(|e| {
        Failure::new(
            "bad_prefs",
            e,
            "fix ~/.config/vega/theme.toml (mode = \"auto\"|\"dark\"|\"light\")",
        )
    })
}

/// Where this host stands: OS mode, headless, and the due mode.
struct Host {
    os: Option<Mode>,
    headless: bool,
    /// The control host's mode, read only when it decides (headless, auto).
    remote: Result<Option<Mode>, String>,
    due: Option<(Mode, Source)>,
}

fn host(p: &prefs::Prefs) -> Host {
    let os = os::mode();
    let headless = os::headless(p.headless, os);
    let control = vega_registry::token(&p.control);
    let remote = if headless
        && p.mode == Pref::Auto
        && vega_herdr::state::is_inbox_only(&vega_herdr::state::post_host())
    {
        // Inbox-only hosts may read just this key (vega-lhc.90).
        vega_registry::get_one_blocking(&vega_registry::theme_key(&control), Duration::from_secs(4))
            .map(|v| v.and_then(|v| Mode::parse(v["mode"].as_str()?)))
    } else if headless && p.mode == Pref::Auto {
        vega_registry::read_blocking(Duration::from_secs(4))
            .map(|v| v.theme(&control).and_then(Mode::parse))
    } else {
        Ok(None)
    };
    let due = resolve(
        p.mode,
        headless,
        os,
        &p.control,
        remote.as_ref().ok().copied().flatten(),
    );
    Host {
        os,
        headless,
        remote,
        due,
    }
}

fn no_due(p: &prefs::Prefs, h: &Host) -> Failure {
    let why = match &h.remote {
        Err(e) => format!("registry: {e}"),
        Ok(_) => format!("{} published no fresh mode", p.control),
    };
    Failure::new(
        "no_mode",
        format!("headless host follows {}, but {why}", p.control),
        "vega theme dark|light fixes a mode here; or check vegad on the control host",
    )
}

fn home(env: &Env) -> Result<(), Failure> {
    let p = load(env)?;
    let h = host(&p);
    println!(
        "os: {}",
        h.os.map_or("unknown (no readable appearance)", Mode::as_str)
    );
    println!(
        "host: {}{}",
        vega_herdr::state::post_host(),
        if h.headless {
            format!(", headless, follows {}", p.control)
        } else {
            String::new()
        }
    );
    println!("mode: {} (theme.toml)", p.mode.as_str());
    match &h.due {
        Some((m, s)) => println!("due: {} ({})", m.as_str(), s.label()),
        None => println!("due: none ({})", no_due(&p, &h).message),
    }
    match state::read(env) {
        Some(a) => println!(
            "applied: {} ({}, {})",
            a.mode.as_str(),
            a.source,
            local_time(Some(a.at)).unwrap_or_else(|| a.at.to_string())
        ),
        None => println!("applied: never on this host"),
    }
    println!(
        "live claude: {}",
        if p.live_claude {
            "on (vegad types /config theme=<mode> into idle claude panes)"
        } else {
            "off (theme.toml live_claude = false)"
        }
    );
    let due = h.due.as_ref().map(|(m, _)| *m);
    let all = adapters::status_all(env, &p);
    println!("tools[{}]{{tool,value,mode,status}}:", all.len());
    let mut drift = 0;
    for s in &all {
        let mode = match &s.current {
            Current::Mode(m) => m.as_str().to_string(),
            Current::Native => "auto".into(),
            Current::Unknown => "?".into(),
            Current::Absent(_) | Current::Error(_) => "-".into(),
        };
        let status = if !p.enabled(s.tool) {
            "disabled".to_string()
        } else {
            match &s.current {
                Current::Native => "native".into(),
                Current::Absent(r) => format!("skip: {r}"),
                Current::Error(e) => format!("error: {e}"),
                _ if due.is_some_and(|d| s.drift(d)) => {
                    drift += 1;
                    match s.read_only {
                        Some(owner) => {
                            format!("drift, read-only ({owner}): set [theme] auto_switch = true")
                        }
                        None => "drift".into(),
                    }
                }
                _ => "ok".into(),
            }
        };
        let value = if s.value.is_empty() { "-" } else { &s.value };
        println!("  {},{},{},{}", s.tool, cell(value), mode, cell(&status));
    }
    if drift > 0 {
        println!("help: vega theme apply fixes the drift vega owns");
    }
    Ok(())
}

fn set(env: &Env, pref: Pref) -> Result<(), Failure> {
    prefs::set_mode(env, pref)
        .map_err(|e| Failure::new("write_failed", e, "check ~/.config/vega/theme.toml"))?;
    println!("mode: {} (theme.toml)", pref.as_str());
    apply(env)
}

fn apply(env: &Env) -> Result<(), Failure> {
    let p = load(env)?;
    let h = host(&p);
    let Some((mode, source)) = h.due.clone() else {
        return Err(no_due(&p, &h));
    };
    let out = vega_theme_sync::apply_due(env, &p, mode, &source)
        .map_err(|e| Failure::new("write_failed", e, "check $XDG_STATE_HOME/vega"))?;
    println!("applied: {} ({})", mode.as_str(), source.label());
    println!("tools[{}]{{tool,result}}:", out.len());
    let mut failed = 0;
    for (tool, o) in &out {
        if matches!(o, Outcome::Failed(_)) {
            failed += 1;
        }
        println!("  {tool},{}", cell(&o.label()));
    }
    if vega_herdr::state::relay_running() {
        println!("running claude panes: vegad switches them when idle");
    } else {
        println!(
            "running claude panes: unchanged (vegad not running); type /config theme={} there",
            mode.as_str()
        );
    }
    if failed > 0 {
        return Err(Failure::new(
            "apply_failed",
            format!("{failed} tool(s) failed"),
            "see the results above; the other tools are set",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skill_has_valid_frontmatter() {
        vega_plugin::assert_guide(SKILL, "theme");
    }
}
