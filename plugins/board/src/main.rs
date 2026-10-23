//! vega board - the UAC's open decisions and the repo's beads in a herdr pane:
//! List / Table / Kanban over bd issues, docked as a side panel or floating as
//! a popup. Copied from herdr-beads (see README.md, Provenance).

mod app;
mod app_live;
mod badges;
mod bd;
mod decisions;
mod form;
mod inbox;
mod input;
mod jump;
mod keys;
mod model;
mod notes;
mod selftest;
mod uac;
mod ui;
mod vegad;
mod views;
mod writer;

use crate::app::App;
use crate::bd::live::{Signal, Watcher};
use crate::model::{Mode, Scope};
use anyhow::Result;
use clap::{Parser, ValueEnum};
use ratatui::backend::{Backend, CrosstermBackend};
use ratatui::crossterm::event::{
    self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
    Event, KeyEventKind,
};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen, SetTitle,
};
use ratatui::Terminal;
use std::io::{self, IsTerminal};
use std::time::Duration;

#[derive(Clone, Copy, ValueEnum)]
enum ModeArg {
    Dock,
    Popup,
}

#[derive(Parser)]
#[command(
    name = "vega board",
    version = env!("VEGA_VERSION"),
    about = "The UAC's open decisions and the repo's beads in a herdr pane (TUI)",
    after_help = "Examples:\n  vega-board                  # popup board for the beads of the current repo\n  vega-board --mode dock      # narrow side panel\n  vega-board --selftest       # print the beads as text, no TUI\n\nIn the board: the Inbox lists what needs you (decisions, waiting panes, notes), m answers (vega uac answer), x dismisses a decision (bd close with your reason), D toggles lane/all beads, ? lists all keys.\nIn herdr: bind vega.board.toggle-dock or vega.board.open-board."
)]
struct Cli {
    /// Layout: a narrow docked side panel or a wide floating popup
    #[arg(long, value_enum, default_value = "popup")]
    mode: ModeArg,
    /// Start in the global scope (bd --global, needs a shared-server bd)
    #[arg(long)]
    global: bool,
    /// Print the beads as plain text and exit (no TUI)
    #[arg(long)]
    selftest: bool,
    /// Print the agent guide for this plugin, SKILL.md form (read by `vega skills`)
    #[arg(long)]
    skill: bool,
}

const SKILL: &str = include_str!("../SKILL.md");
/// Resolve the repo the board should scope to. herdr runs the pane in the
/// plugin dir (no `.beads`), but injects HERDR_SOCKET_PATH / HERDR_WORKSPACE_ID
/// / HERDR_PANE_ID / HERDR_BIN_PATH - so we ask `herdr pane list` for the
/// focused pane's cwd in our workspace (same trick herdr-flist uses).
fn resolve_repo_cwd() -> Option<String> {
    let me = std::env::var("HERDR_PANE_ID").unwrap_or_default();
    let ws = std::env::var("HERDR_WORKSPACE_ID").unwrap_or_default();
    let bin = std::env::var("HERDR_BIN_PATH").unwrap_or_else(|_| "herdr".to_string());
    let out = std::process::Command::new(&bin)
        .arg("pane")
        .arg("list")
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).ok()?;
    let panes = v.get("result")?.get("panes")?.as_array()?;

    let others: Vec<&serde_json::Value> = panes
        .iter()
        .filter(|p| p.get("pane_id").and_then(|x| x.as_str()).unwrap_or("") != me)
        .collect();
    let cwd = |p: &serde_json::Value| p.get("cwd").and_then(|x| x.as_str()).map(String::from);
    let in_ws = |p: &serde_json::Value| {
        !ws.is_empty() && p.get("workspace_id").and_then(|x| x.as_str()) == Some(ws.as_str())
    };
    let focused = |p: &serde_json::Value| p.get("focused").and_then(|x| x.as_bool()) == Some(true);
    let has_beads = |c: &str| std::path::Path::new(c).join(".beads").exists();

    // Preference order: a pane with a real .beads dir wins (focused first, my
    // workspace next), then any focused pane, then anything.
    let mut candidates: Vec<String> = Vec::new();
    for p in &others {
        if let Some(c) = cwd(p) {
            let score = (has_beads(&c) as u8) * 4 + (focused(p) as u8) * 2 + (in_ws(p) as u8);
            candidates.push(format!("{score:02}\u{1}{c}"));
        }
    }
    candidates.sort();
    candidates
        .last()
        .and_then(|s| s.split('\u{1}').nth(1))
        .map(String::from)
}

fn apply_working_dir() {
    // 1) explicit override from the launcher
    if let Ok(dir) = std::env::var("VEGA_BOARD_CWD") {
        if !dir.is_empty() {
            let _ = std::env::set_current_dir(&dir);
        }
    }
    // 2) if still no .beads here, ask herdr for the workspace's repo
    if !std::path::Path::new(".beads").exists() {
        if let Some(dir) = resolve_repo_cwd() {
            let _ = std::env::set_current_dir(&dir);
        }
    }
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    if cli.skill {
        print!("{SKILL}");
        return Ok(());
    }
    let mode = match cli.mode {
        ModeArg::Dock => Mode::Dock,
        ModeArg::Popup => Mode::Popup,
    };
    let scope = if cli.global {
        Scope::Global
    } else {
        Scope::Repo
    };
    let selftest = cli.selftest;

    apply_working_dir();

    if selftest {
        return selftest::run(scope);
    }

    if !io::stdout().is_terminal() {
        eprintln!("error: vega-board is a TUI and needs a terminal");
        eprintln!("hint: vega-board --selftest prints the beads as text");
        std::process::exit(2);
    }

    // Tag the pane so the launcher can find (and toggle) it via `herdr pane list`.
    let pane_title = match mode {
        Mode::Dock => "vega-board-dock",
        Mode::Popup => "vega-board-popup",
    };
    enable_raw_mode()?;
    let mut out = io::stdout();
    execute!(
        out,
        EnterAlternateScreen,
        EnableMouseCapture,
        EnableBracketedPaste,
        SetTitle(pane_title)
    )?;
    let mut terminal = Terminal::new(CrosstermBackend::new(out))?;

    let res = run_app(&mut terminal, App::new(mode, scope));

    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture,
        DisableBracketedPaste
    )?;
    terminal.show_cursor()?;

    res
}

fn run_app<B: Backend>(terminal: &mut Terminal<B>, mut app: App) -> Result<()> {
    // The journal is per workspace, so live updates follow the repo scope only.
    let watcher = Watcher::start(app.show_closed);
    let mut mode = ui::theme::ModeWatch::new(vega_theme_sync::Env::from_env());
    loop {
        mode.poll();
        terminal.draw(|f| ui::render(f, &mut app))?;
        if app.should_quit {
            break;
        }
        // Wake up regularly so changes made elsewhere show without a keypress.
        if event::poll(Duration::from_millis(250))? {
            match event::read()? {
                Event::Key(k) if k.kind == KeyEventKind::Press => keys::handle_key(&mut app, k),
                Event::Mouse(m) => keys::handle_mouse(&mut app, m),
                Event::Paste(s) => keys::handle_paste(&mut app, &s),
                _ => {}
            }
        }
        app.finish_writes();
        watcher.set_show_closed(app.show_closed);
        let repo = app.scope == Scope::Repo;
        for signal in watcher.poll() {
            match signal {
                Signal::Live(on) => app.live = on,
                Signal::Record(rec) if repo => app.apply_record(*rec),
                Signal::Blocked(state) if repo => app.apply_blocked(*state),
                Signal::Snapshot(snap) => app.apply_snapshot(*snap),
                Signal::LoadFailed { scope, error } => app.load_failed(scope, &error),
                Signal::Detail { scope, id, bead } => app.apply_detail(scope, id, bead.map(|b| *b)),
                _ => {}
            }
        }
        if app.take_reload_wanted() {
            watcher.request_load(app.scope);
        }
        if let Some(id) = app.detail_wanted.take() {
            watcher.request_show(app.scope, id);
        }
        if app.should_quit {
            break;
        }
    }
    Ok(())
}

#[cfg(test)]
mod cli_tests {
    use super::*;

    #[test]
    fn skill_has_valid_frontmatter() {
        vega_plugin::assert_guide(SKILL, "board");
    }

    #[test]
    fn cli_is_consistent() {
        use clap::CommandFactory;
        Cli::command().debug_assert();
    }
}
