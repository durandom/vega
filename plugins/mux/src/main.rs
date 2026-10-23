//! `vega mux`: agent-to-agent messaging over herdr.
//!
//! MVP scope: list agents (home view) and `send`. A message is delivered as a
//! new prompt via `herdr agent prompt`, with a footer that tells the receiver
//! how to reply. No coordinator, no mailbox: the reply is just another `send`.

mod dialog;
mod hook;
mod next;
mod reopen;
mod transcript;
mod trust;
mod worktree;

use std::env;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use clap::error::ErrorKind;
use clap::{CommandFactory, Parser, Subcommand};
use serde_json::Value;
use vega_herdr::capacity;
use vega_herdr::lanes;
use vega_herdr::state::{
    children, drop_kept_brief, forget, keep_brief, kept_brief, mode_of, parent_of, relay_running,
    save_mode, save_parent, save_task, saved_task, set_replied, set_silent, settle, state_file,
    KeptBrief, Mode,
};
use vega_herdr::{
    agent_name, cell, collapse_home, herdr, herdr_local, herdr_text, local_host, local_time,
    machine, me, me_line, mflag, print_failure, qualify, set_machine, status_counts, status_rank,
    str_of, Failure, Me,
};

#[derive(Parser)]
#[command(
    name = "vega mux",
    bin_name = "vega mux",
    disable_version_flag = true,
    about = "Agent-to-agent messaging over herdr",
    after_help = "Examples:\n  vega mux                         # agents in this workspace\n  vega mux --all                   # all workspaces, plus panes without an agent\n  vega mux show w3V:p2             # kind, name, status, title and recent output\n  vega mux transcript w3V:p2 --last # its last reply, from its session log\n  vega mux send w3V:p2 \"ping\"       # send a message; the reply arrives as a new prompt\n  vega mux send rev \"status?\"       # target an agent by its name instead of its pane\n  vega mux spawn rev --kind codex  # start a codex agent in a new tab\n  vega mux close fix-login         # close a worker (its worktree workspace)"
)]
struct Cli {
    /// List agents in all workspaces, plus workspace labels and panes without an agent
    #[arg(long)]
    all: bool,
    /// Act on the agents of another herdr machine (label from `herdr machine list`)
    #[arg(long, global = true)]
    machine: Option<String>,
    /// Print the agent guide for this plugin, SKILL.md form (read by `vega skills`)
    #[arg(long)]
    skill: bool,
    /// Print the bare version
    #[arg(short = 'V', short_alias = 'v', long)]
    version: bool,
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

const SKILL: &str = include_str!("../SKILL.md");

#[derive(Subcommand)]
enum Cmd {
    /// Agent hooks: `ask-parent` (PreToolUse) and `install`; run by vega's install
    #[command(hide = true)]
    Hook {
        #[command(subcommand)]
        action: HookCmd,
    },
    /// Answer only a question dialog; trust and permission stay with the Slayer
    #[command(
        after_help = "Rechecks before acting, navigates with arrows, and verifies the next screen.\nExit 2 for every other dialog kind. Text requires Claude's empty Type something.\neditor; unsupported editors are refused. Answers one question at a time; inspect\nany next question or review screen with `vega mux dialog <target>`.\n\nExamples:\n  vega mux answer w3V:p2 --option 2\n  vega mux answer rev --text Blue"
    )]
    Answer {
        /// Pane id or unique agent name
        target: String,
        /// Numbered option to select with arrow keys
        #[arg(long, required_unless_present = "text", conflicts_with = "text")]
        option: Option<usize>,
        /// Free text for the question's answer editor
        #[arg(long, required_unless_present = "option", conflicts_with = "option")]
        text: Option<String>,
    },
    /// Classify the visible dialog (exit 0 present, 1 absent)
    #[command(after_help = "Examples:\n  vega mux dialog w3V:p2\n  vega mux dialog rev --json")]
    Dialog {
        /// Pane id or unique agent name
        target: String,
        /// Print JSON including the raw screen
        #[arg(long)]
        json: bool,
    },
    /// Send a message to another agent; it arrives as a new prompt with a reply footer
    #[command(
        after_help = "Examples:\n  vega mux send w3V:p2 \"ping\"\n  vega mux send reviewer \"Review done? Reply with a 1-line status.\"\n  vega mux send rev --file /tmp/review-notes.md\n  printf '%s' \"$msg\" | vega mux send rev -"
    )]
    Send {
        /// Target agent: pane id (e.g. w3V:p2) or unique herdr agent name
        target: String,
        /// Message text; `-` reads it from stdin
        #[arg(required_unless_present = "file", conflicts_with = "file")]
        text: Option<String>,
        /// Read the message text from a file (no shell quoting needed)
        #[arg(long, value_name = "PATH")]
        file: Option<PathBuf>,
        /// Urgent: paste it into the pane at once, even mid-turn, instead of mail
        #[arg(long)]
        now: bool,
    },
    /// Show one agent: kind, name, status, parent, title, cwd and recent output
    #[command(after_help = "Examples:\n  vega mux show w3V:p2\n  vega mux show rev --lines 200")]
    Show {
        /// Target agent: pane id (e.g. w3V:p2) or unique herdr agent name
        target: String,
        /// Number of recent terminal lines to read
        #[arg(long, default_value_t = 30)]
        lines: u32,
    },
    /// Read an agent's session log: prompts, replies, tool calls and errors
    #[command(
        after_help = "Reads the agent's own log, not the screen: claude ~/.claude/projects, codex\n~/.codex/sessions, pi ~/.pi/agent/sessions, agy ~/.gemini/antigravity-cli/brain.\nherdr names the session; without one, the cwd's only log is used, or the error\nlists the candidates for --session. Replies are numbered: pass the last number\nyou saw to --since. Text is cut at 300 chars (--full); --last prints it whole.\nThis machine only.\n\nExamples:\n  vega mux transcript w3V:p2              # newest 20 entries\n  vega mux transcript rev --last          # its last reply, full text\n  vega mux transcript rev --since 12      # everything after reply 12\n  vega mux transcript rev --count"
    )]
    Transcript {
        /// Target agent: pane id (e.g. w3V:p2) or unique herdr agent name
        target: String,
        /// Only the last assistant reply, untruncated
        #[arg(long, conflicts_with_all = ["since", "count"])]
        last: bool,
        /// Only entries after reply N (the `reply` column, or `replies:` of an earlier call)
        #[arg(long, value_name = "N", conflicts_with = "count")]
        since: Option<usize>,
        /// Only the number of replies and entries
        #[arg(long)]
        count: bool,
        /// Number of newest entries to show
        #[arg(long, value_name = "N", default_value_t = 20)]
        entries: usize,
        /// Do not truncate entry text
        #[arg(long)]
        full: bool,
        /// Session id or log path, when herdr names none and the cwd has several
        #[arg(long, value_name = "ID|PATH")]
        session: Option<String>,
    },
    /// Show or set who answers a child's questions: headless (its parent) or headed (the user)
    #[command(
        after_help = "Spawn and adopt default to headed: the user answers questions in the pane.\nWith --headless, the ask-parent hook denies in-pane questions\n(AskUserQuestion, request_user_input, request_user_input_async) and names the\ncommand to ask the parent; a headed codex async question mails the parent a copy.\nChildren without a valid mode record stay headless for compatibility.\nHeaded asking, silent and input do not wake the parent; exited, failure,\nquota, completed and decided still do. Without a mode: print the current one.\n\nExamples:\n  vega mux mode w55:p1 headed\n  vega mux mode fix headless\n  vega mux mode fix"
    )]
    Mode {
        /// Target agent: pane id (e.g. w3V:p2) or unique herdr agent name
        target: String,
        /// The new mode; omit it to print the current one
        #[arg(value_enum)]
        mode: Option<ModeArg>,
    },
    /// Focus the agent that has waited longest for you (done or blocked), on any host
    #[command(
        after_help = "vegad keeps the queue (docs/design/mux-next.md): an agent waits from its first\ndone or blocked until it works again, its question goes away, it closes, or\nit gets the focus; herdr marking it seen (a tab visit) does not count. Other\nhosts' queues come from the registry; without it, this host's only.\n\nA pane on another machine is focused there and a toast names the machine:\nherdr cannot switch your client, so switch with prefix+w. Nothing waiting:\na toast, `none`, exit 0. No default vega key binding. For cross-machine\nclient switching, use herdr's goto overlay: prefix+g, then d.\n\nExamples:\n  vega mux next\n  vega mux next --status blocked,done"
    )]
    Next {
        /// Which waits count, comma-separated: done, blocked
        #[arg(long, value_delimiter = ',', default_value = "done", value_parser = clap::builder::PossibleValuesParser::new(next::STATUSES))]
        status: Vec<String>,
    },
    /// Close an agent: its worktree workspace, or only its pane elsewhere
    #[command(
        after_help = "A spawned --worktree agent has its own herdr workspace: close closes that\nworkspace when no other agent runs in it. Otherwise it closes only the agent's\npane. Your own pane and workspace are refused. Close before `wt remove <branch>`.\n\nExamples:\n  vega mux close fix-login\n  vega mux close w4C:p1"
    )]
    Close {
        /// Target agent: pane id (e.g. w4C:p1) or unique herdr agent name
        target: String,
    },
    /// Respawn a closed agent in its recorded cwd, resuming its session
    #[command(after_help = include_str!("../help/reopen.md"))]
    Reopen {
        /// Recorded pane id or unique agent name
        target: String,
    },
    /// Start a new agent in a herdr tab, split or workspace, without approval prompts
    #[command(
        after_help = include_str!("../help/spawn.md")
    )]
    Spawn {
        /// Short kebab-case agent name, also the tab label: [a-z][a-z0-9-]{0,23}, e.g. link-parsing
        #[arg(required_unless_present = "resume")]
        name: Option<String>,
        /// herdr agent kind, e.g. codex, claude, pi, agy (see `herdr agent`)
        #[arg(long, required_unless_present = "resume")]
        kind: Option<String>,
        /// Deliver the saved brief of a spawn that stopped at a dialog, once the user answered it
        #[arg(long, value_name = "PANE", conflicts_with_all = ["name", "kind", "cwd", "at", "prompt", "prompt_file", "worktree", "env", "no_parent", "headless", "args"])]
        resume: Option<String>,
        /// Working directory of the new agent [default: current directory]
        #[arg(long)]
        cwd: Option<String>,
        /// Where to start it: new tab in your workspace, split of your pane, or new workspace [default: tab]
        #[arg(long, value_enum, conflicts_with = "worktree")]
        at: Option<At>,
        /// First task, sent with your reply footer once the agent is ready; `-` reads stdin
        #[arg(long, conflicts_with = "prompt_file")]
        prompt: Option<String>,
        /// Read the first task from a file (no shell quoting needed)
        #[arg(long, value_name = "PATH")]
        prompt_file: Option<PathBuf>,
        /// Start in a git worktree for this branch (created if missing), in its own herdr workspace
        #[arg(long, value_name = "BRANCH")]
        worktree: Option<String>,
        /// Base of a new --worktree branch, e.g. origin/main after `git fetch -q`, or main when it is ahead [default: the local default branch]
        #[arg(long, value_name = "REF", requires = "worktree")]
        base: Option<String>,
        /// Env for the agent and its tool shells, repeatable, e.g. --env VEGA_GATE_ROLE=reviewer
        #[arg(long, value_name = "K=V", value_parser = parse_env)]
        env: Vec<String>,
        /// Record no parent: the brief gets no done-report footer, and vegad and vega uac do not track the agent (one that reports by its own command, e.g. a gate reviewer)
        #[arg(long)]
        no_parent: bool,
        /// Redirect in-pane questions to the parent [default: headed, the user answers]
        #[arg(long)]
        headless: bool,
        /// Extra agent args, appended after the defaults
        #[arg(last = true)]
        args: Vec<String>,
    },
}

#[derive(Subcommand)]
enum HookCmd {
    /// PreToolUse hook: deny in-pane questions of an agent with a parent
    AskParent,
    /// Moved: alias for `vega config apply hooks` (claude, codex and pi)
    Install {
        /// Ignored: `vega config apply hooks` always includes codex
        #[arg(long)]
        codex: bool,
    },
}

#[derive(Clone, Copy, clap::ValueEnum)]
enum ModeArg {
    Headless,
    Headed,
}

#[derive(Clone, Copy, clap::ValueEnum)]
enum At {
    Tab,
    Split,
    Workspace,
}

/// Where `spawn` starts the agent.
enum Place {
    At(At),
    /// The worktree's own herdr workspace; `--cwd` picks the repo.
    Worktree {
        branch: String,
        base: Option<String>,
    },
}

/// Text from an argument, `-` for stdin, or a file: agents pass long prompts
/// without fighting shell quoting. Empty text is a usage error.
fn read_text(
    arg: Option<&str>,
    file: Option<&PathBuf>,
    what: &str,
) -> Result<Option<String>, Failure> {
    let text = match (arg, file) {
        (Some("-"), _) => {
            let mut s = String::new();
            std::io::stdin().read_to_string(&mut s).map_err(|e| {
                Failure::new(
                    "usage",
                    format!("cannot read the {what} from stdin: {e}"),
                    "pipe UTF-8 text into the command",
                )
            })?;
            s
        }
        (Some(t), _) => t.to_string(),
        (None, Some(f)) => std::fs::read_to_string(f).map_err(|e| {
            Failure::new(
                "usage",
                format!("cannot read the {what} from {}: {e}", f.display()),
                "pass a readable UTF-8 file",
            )
        })?,
        (None, None) => return Ok(None),
    };
    if text.trim().is_empty() {
        return Err(Failure::new(
            "usage",
            format!("{what} text is empty"),
            "pass non-empty text, `-` for stdin, or a file",
        ));
    }
    Ok(Some(text))
}

/// Agent args added by `spawn`, per herdr kind. Spawned agents run without
/// approval prompts; codex also needs `--no-daemon`, because its shared daemon
/// keeps the `HERDR_*` env of the pane it first started in, and skips its
/// startup update check, whose "Update available" screen blocks the start.
fn default_args(kind: &str) -> &'static [&'static str] {
    match kind {
        "codex" => &[
            "--no-daemon",
            "--dangerously-bypass-approvals-and-sandbox",
            "-c",
            "check_for_update_on_startup=false",
        ],
        "claude" => &["--dangerously-skip-permissions"],
        "agy" => &["--dangerously-skip-permissions", "--sandbox"],
        "pi" => &["--approve"],
        _ => &[],
    }
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
            let message = message.trim_start_matches("error: ");
            print_failure(&Failure::new("usage", message, "valid usage below"));
            // Inline the relevant command's help so the agent can self-correct
            // in one turn instead of calling --help next.
            let mut cmd = Cli::command();
            cmd.build(); // propagates `vega mux` into subcommand usage lines
            let sub = env::args().nth(1).filter(|a| !a.starts_with('-'));
            let help = match sub.as_deref().and_then(|s| cmd.find_subcommand_mut(s)) {
                Some(sc) => sc.render_help(),
                None => cmd.render_help(),
            };
            print!("{help}");
            return ExitCode::from(2);
        }
    };
    set_machine(cli.machine.clone());
    if cli.version {
        println!("{}", vega_build::version_line!());
        return ExitCode::SUCCESS;
    }
    if cli.skill {
        print!("{SKILL}");
        return ExitCode::SUCCESS;
    }
    if let Some(f) = cli.cmd.as_ref().and_then(validate) {
        print_failure(&f);
        return ExitCode::from(2);
    }
    let result = match cli.cmd {
        None => home(cli.all),
        Some(Cmd::Hook {
            action: HookCmd::AskParent,
        }) => {
            hook::ask_parent();
            Ok(())
        }
        // Moved to `vega config apply hooks` (vega-lhc.107); codex is always included.
        Some(Cmd::Hook {
            action: HookCmd::Install { .. },
        }) => return vega_plugin::run_plugin("config", &["apply", "hooks"]),
        Some(Cmd::Send {
            target,
            text,
            file,
            now,
        }) => read_text(text.as_deref(), file.as_ref(), "message")
            .and_then(|t| send(&target, &t.expect("clap requires text or --file"), now)),
        Some(Cmd::Dialog { target, json }) => return dialog::show(&target, json),
        Some(Cmd::Answer {
            target,
            option,
            text,
        }) => dialog::answer(&target, option, text.as_deref()),
        Some(Cmd::Show { target, lines }) => show(&target, lines),
        Some(Cmd::Transcript {
            target,
            last,
            since,
            count,
            entries,
            full,
            session,
        }) => {
            let view = match (last, since, count) {
                (true, _, _) => transcript::View::Last,
                (_, Some(n), _) => transcript::View::Since(n),
                (_, _, true) => transcript::View::Count,
                _ => transcript::View::Tail(entries),
            };
            return transcript::run(&target, view, full, session.as_deref());
        }
        Some(Cmd::Close { target }) => close(&target),
        Some(Cmd::Reopen { target }) => reopen::run(&target),
        Some(Cmd::Next { status }) => next::run(&status),
        Some(Cmd::Mode { target, mode }) => set_mode(&target, mode),
        Some(Cmd::Spawn {
            name,
            kind,
            cwd,
            at,
            prompt,
            prompt_file,
            worktree,
            base,
            env,
            no_parent,
            headless,
            args,
            resume,
        }) => match resume {
            Some(pane) => resume_spawn(&pane),
            None => read_text(prompt.as_deref(), prompt_file.as_ref(), "prompt").and_then(|p| {
                let place = match worktree {
                    Some(branch) => Place::Worktree { branch, base },
                    None => Place::At(at.unwrap_or(At::Tab)),
                };
                let (name, kind) = (name.expect("clap"), kind.expect("clap"));
                spawn(
                    &name,
                    &kind,
                    cwd,
                    place,
                    p.as_deref(),
                    &env,
                    no_parent,
                    spawn_mode(headless),
                    &args,
                )
            }),
        },
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(f) => {
            print_failure(&f);
            ExitCode::from(if f.code == "usage" { 2 } else { 1 })
        }
    }
}

/// Usage checks clap cannot express: empty strings are valid arguments to clap
/// but never a valid target or message.
fn validate(cmd: &Cmd) -> Option<Failure> {
    let empty = |s: &str| s.trim().is_empty();
    match cmd {
        Cmd::Answer { target, .. }
        | Cmd::Dialog { target, .. }
        | Cmd::Send { target, .. }
        | Cmd::Show { target, .. }
        | Cmd::Transcript { target, .. }
        | Cmd::Close { target }
        | Cmd::Mode { target, .. }
            if empty(target) =>
        {
            Some(Failure::new(
                "usage",
                "target is empty",
                "run `vega mux` to list agents, then pass a pane id or agent name",
            ))
        }
        Cmd::Spawn {
            worktree: Some(b), ..
        } if empty(b) => Some(Failure::new(
            "usage",
            "--worktree branch is empty",
            "pass a branch name, e.g. --worktree fix-login",
        )),
        Cmd::Spawn {
            worktree: Some(_), ..
        } if machine().is_some() => Some(Failure::new(
            "usage",
            "--worktree works only on this machine",
            "spawn with --cwd <existing checkout> on that machine, or drop --machine",
        )),
        _ => None,
    }
}

fn home(all: bool) -> Result<(), Failure> {
    let Me {
        pane: me,
        workspace: my_workspace,
        agent: my_agent,
        program: my_program,
        cwd: my_cwd,
    } = me()?;
    let list = herdr(&["agent", "list"])?;
    let agents = list["result"]["agents"]
        .as_array()
        .cloned()
        .unwrap_or_default();

    let bin = env::current_exe()
        .map(|p| collapse_home(&p.display().to_string()))
        .unwrap_or_else(|_| "unknown".into());
    println!("bin: {bin}");
    println!("description: Agent-to-agent messaging over herdr");
    // Pane ids are per machine: on another machine, every agent is "other".
    let remote = machine().is_some();
    let my_name = agents
        .iter()
        .find(|a| !remote && a["pane_id"] == me.as_str())
        .map(agent_name)
        .unwrap_or("-");
    println!("me: {}", me_line(&me, &my_program, my_name, &my_cwd));
    if let Some(m) = machine() {
        println!("machine: {m}");
    }

    let (mut shown, hidden): (Vec<&Value>, Vec<&Value>) = agents
        .iter()
        .filter(|a| remote || a["pane_id"] != me.as_str())
        .partition(|a| remote || all || a["workspace_id"] == my_workspace.as_str());
    shown.sort_by_key(|a| status_rank(str_of(a, "agent_status")));

    let scope = match machine() {
        Some(m) => format!("machine {m}"),
        None if all => "all workspaces".to_string(),
        None => format!("workspace {my_workspace}"),
    };
    if all {
        let ws = herdr(&["workspace", "list"])?;
        let ws = ws["result"]["workspaces"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        for line in workspace_table(&ws) {
            println!("{line}");
        }
    }
    if shown.is_empty() {
        println!("agents: 0 other agents in {scope}");
    } else {
        println!("status: {}", status_counts(&shown));
        for line in agent_table(&shown) {
            println!("{line}");
        }
    }
    if !hidden.is_empty() {
        println!(
            "others: {} in other workspaces (vega mux --all)",
            plural(hidden.len(), "agent")
        );
    }
    if all {
        let list = herdr(&["pane", "list"])?;
        let rows: Vec<(Value, String)> = list["result"]["panes"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|p| p["agent"].is_null() && (remote || p["pane_id"] != me.as_str()))
            .map(|p| {
                let info = herdr(&["pane", "process-info", "--pane", str_of(p, "pane_id")])
                    .map(|v| v["result"]["process_info"].clone())
                    .unwrap_or(Value::Null); // the pane closed meanwhile
                (p.clone(), command_of(&info))
            })
            .collect();
        for line in pane_table(&rows) {
            println!("{line}");
        }
    }
    if my_agent == NO_AGENT {
        println!("note: herdr detects no agent in your pane, so peers cannot reply to it");
    }
    if shown.is_empty() && hidden.is_empty() {
        println!("help: start another agent in a herdr pane, then run `vega mux` again");
    } else {
        let m = mflag();
        println!("help[2]:");
        println!("  vega mux{m} show <pane|name>            # status and recent output");
        println!("  vega mux{m} send <pane|name> \"<text>\"   # reply arrives as a new prompt");
    }
    Ok(())
}

/// What `close` acts on: the agent's own worktree workspace, or its pane.
#[derive(Debug, PartialEq)]
enum Closing {
    Workspace,
    Pane,
}

/// A linked-worktree workspace with no other agent belongs to this agent
/// (`spawn --worktree`); closing it leaves no empty workspace behind.
fn closing(workspace: &Value, others_in_workspace: usize) -> Closing {
    if workspace["worktree"]["is_linked_worktree"] == true && others_in_workspace == 0 {
        Closing::Workspace
    } else {
        Closing::Pane
    }
}

fn close(target: &str) -> Result<(), Failure> {
    let a = match herdr(&["agent", "get", target]) {
        Ok(v) => v["result"]["agent"].clone(),
        // A sentinel whose pane is gone (`exited` in `vega uac`): closing
        // settles it by dropping its records.
        Err(f) if f.code == "agent_not_found" && has_records(target) => {
            forget(target);
            println!("closed: {target} (pane already gone; records dropped)");
            return Ok(());
        }
        Err(f) => return Err(f),
    };
    let pane = str_of(&a, "pane_id").to_string();
    let ws_id = str_of(&a, "workspace_id").to_string();
    let ws = herdr(&["workspace", "get", &ws_id])?["result"]["workspace"].clone();
    let agents = herdr(&["agent", "list"])?;
    let others = agents["result"]["agents"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|o| o["workspace_id"] == ws_id.as_str() && o["pane_id"] != pane.as_str())
        .count();
    let what = closing(&ws, others);
    // Pane ids are per machine: on another machine nothing is ours.
    if machine().is_none() {
        let me = me()?;
        if me.pane == pane || (what == Closing::Workspace && me.workspace == ws_id) {
            return Err(Failure::new(
                "usage",
                format!("{target} is your own pane or workspace"),
                "close another agent; run `vega mux` to list them",
            ));
        }
    }
    reopen::snapshot(&a);
    // herdr may reuse the pane id as soon as it closes. Drop supervision first.
    forget(&pane);
    match what {
        Closing::Workspace => {
            herdr(&["workspace", "close", &ws_id])?;
            println!(
                "closed: workspace {ws_id} (worktree {}, agent {pane})",
                cell(&collapse_home(str_of(&ws["worktree"], "checkout_path")))
            );
            let checkout = str_of(&ws["worktree"], "checkout_path");
            println!("help: {}", remove_hint(checkout));
        }
        Closing::Pane => {
            herdr(&["pane", "close", &pane])?;
            println!("closed: pane {pane}");
        }
    }
    Ok(())
}

/// `vega mux mode <target> [headless|headed]`: records the mode for the
/// agent that runs in the pane now; without a mode prints it.
fn set_mode(target: &str, mode: Option<ModeArg>) -> Result<(), Failure> {
    let a = herdr(&["agent", "get", target])?["result"]["agent"].clone();
    let pane = str_of(&a, "pane_id");
    let (name, mode) = agent_mode(&a, mode)?;
    let name = name.as_str();
    let parent = parent_of(pane, name);
    println!("mode: {} ({pane} {})", mode.as_str(), cell(name));
    match (parent, mode) {
        (None, _) => {
            println!("note: no parent recorded: the hook leaves its dialogs alone in either mode")
        }
        (Some(p), Mode::Headless) => println!(
            "questions: to its parent {} (ask-parent hook)",
            p.label(&local_host())
        ),
        (Some(_), Mode::Headed) => println!("questions: stay in its pane (the user answers)"),
    }
    Ok(())
}

fn agent_mode(a: &Value, requested: Option<ModeArg>) -> Result<(String, Mode), Failure> {
    let pane = str_of(a, "pane_id");
    let name = mode_agent_name(a);
    if name == "-" {
        return Err(Failure::new(
            "agent_not_ready",
            format!("agent name is not visible yet in pane {pane}; mode unchanged"),
            &format!("answer its startup dialog in the pane, then retry `vega mux mode {pane}`"),
        ));
    }
    if let Some(m) = requested {
        let m = match m {
            ModeArg::Headless => Mode::Headless,
            ModeArg::Headed => Mode::Headed,
        };
        save_mode(machine(), pane, &name, m);
    }
    let mode = mode_of(machine(), pane, &name);
    Ok((name, mode))
}

/// herdr may omit the name at a startup dialog. Recover it only from a
/// matching undelivered spawn, rather than trusting a stale pane's mode.
fn mode_agent_name(agent: &Value) -> String {
    if agent["name"].as_str().is_some_and(|n| !n.is_empty()) {
        return agent_name(agent).into();
    }
    kept_brief(str_of(agent, "pane_id"))
        .filter(|record| same_agent(record, agent))
        .map_or_else(|| agent_name(agent).into(), |record| record.name)
}

fn has_records(pane: &str) -> bool {
    ["parents", "modes", "done", "silent", "replied"]
        .iter()
        .any(|k| state_file(k, pane).is_some_and(|f| f.exists()))
}

fn show(target: &str, lines: u32) -> Result<(), Failure> {
    let info = herdr(&["agent", "get", target])?;
    let a = &info["result"]["agent"];
    let pane = str_of(a, "pane_id");
    let name = mode_agent_name(a);
    let lines_arg = lines.to_string();
    let read = |source: &str| {
        herdr_text(&[
            "agent", "read", target, "--source", source, "--lines", &lines_arg,
        ])
    };
    // herdr refuses the history of a working agent that uses the alternate
    // screen (agent_not_idle): show what is visible instead of failing.
    let raw = match read("recent-unwrapped") {
        Err(f) if f.code == "agent_not_idle" => read("visible")?,
        r => r?,
    };
    // herdr pads a short history with blank rows at the top. If the window is
    // full and its first row has content, older output exists.
    let truncated = raw.lines().count() >= lines as usize
        && raw.lines().next().is_some_and(|l| !l.trim().is_empty());
    let recent = strip_chrome(&raw);

    println!("pane: {pane}");
    println!("kind: {}", str_of(a, "agent"));
    println!("name: {}", cell(&name));
    println!("status: {}", str_of(a, "agent_status"));
    println!(
        "parent: {}",
        parent_of(pane, &name).map_or("-".into(), |p| p.label(&local_host()))
    );
    println!("mode: {}", mode_of(machine(), pane, &name).as_str());
    println!("cwd: {}", cell(&collapse_home(str_of(a, "cwd"))));
    println!(
        "title: {}",
        cell(&title_of(a, saved_task(str_of(a, "pane_id")).as_deref()))
    );
    if recent.is_empty() {
        println!("recent: 0 lines");
    } else {
        println!("recent[{}]:", recent.len());
        for l in &recent {
            println!("  {l}");
        }
    }
    if truncated {
        println!(
            "help: older output exists; run `vega mux show {pane} --lines {}`",
            lines * 4
        );
    }
    Ok(())
}

/// What `vega_herdr::me` reports as the agent of a pane without one.
const NO_AGENT: &str = "unknown";

/// `workspaces[n]{id,label}`: a legend for the pane id prefixes in `--all`.
fn workspace_table(ws: &[Value]) -> Vec<String> {
    let mut out = vec![format!("workspaces[{}]{{id,label}}:", ws.len())];
    for w in ws {
        out.push(format!(
            "  {},{}",
            str_of(w, "workspace_id"),
            cell(str_of(w, "label"))
        ));
    }
    out
}

/// The command line of a pane's foreground job (its process group leader),
/// e.g. `btop`, `ssh ganymede` or `zsh` for an idle shell.
fn command_of(info: &Value) -> String {
    let procs = info["foreground_processes"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default();
    let leader = procs
        .iter()
        .find(|p| p["pid"] == info["foreground_process_group_id"])
        .or(procs.first());
    let Some(p) = leader else {
        return "-".into();
    };
    let cmd = p["cmdline"].as_str().or(p["argv0"].as_str()).unwrap_or("-");
    // A login shell's argv0 starts with `-`.
    shorten(cmd.trim().trim_start_matches('-'), 40)
}

/// `panes[n]{pane,command,cwd}` for panes without an agent: they take no
/// `send`, so they get their own table instead of empty agent columns.
fn pane_table(rows: &[(Value, String)]) -> Vec<String> {
    if rows.is_empty() {
        return vec!["panes: 0 panes without an agent".into()];
    }
    let mut out = vec![format!("panes[{}]{{pane,command,cwd}}:", rows.len())];
    for (p, cmd) in rows {
        out.push(format!(
            "  {},{},{}",
            str_of(p, "pane_id"),
            cell(cmd),
            cell(&collapse_home(str_of(p, "cwd")))
        ));
    }
    out
}

/// TOON header and rows for `vega mux`. The `name` column is dropped when no
/// agent has one: most agents are started by hand, and an all-`-` column
/// costs tokens per row without telling anything. `kind` always stays: herdr
/// lists only detected agents, so it is never empty.
fn agent_table(agents: &[&Value]) -> Vec<String> {
    let names = agents.iter().any(|a| agent_name(a) != "-");
    let fields = if names {
        "pane,kind,name,status,cwd,title"
    } else {
        "pane,kind,status,cwd,title"
    };
    let mut out = vec![format!("agents[{}]{{{fields}}}:", agents.len())];
    for a in agents {
        let name = if names {
            format!("{},", cell(agent_name(a)))
        } else {
            String::new()
        };
        out.push(format!(
            "  {},{},{name}{},{},{}",
            str_of(a, "pane_id"),
            cell(str_of(a, "agent")),
            str_of(a, "agent_status"),
            cell(&collapse_home(str_of(a, "cwd"))),
            cell(&shorten(
                &title_of(a, saved_task(str_of(a, "pane_id")).as_deref()),
                60
            )),
        ));
    }
    out
}

/// What the agent works on. Claude Code keeps a summary in its terminal
/// title; Codex and pi only show their directory there, so fall back to the
/// task `spawn --prompt` saved for the pane (`saved_task`).
fn title_of(a: &Value, task: Option<&str>) -> String {
    let title = clean_title(str_of(a, "terminal_title_stripped"), str_of(a, "cwd"));
    match task.map(str::trim) {
        Some(l)
            if title == "-"
                && !l.is_empty()
                && !l.chars().all(|c| c.is_ascii_digit())
                && l != agent_name(a) =>
        {
            l.to_string()
        }
        _ => title,
    }
}

/// The task's first line, saved by `spawn --prompt` as the title fallback.
fn task_label(prompt: &str) -> Option<String> {
    let line = prompt.lines().map(str::trim).find(|l| !l.is_empty())?;
    Some(shorten(line, 60))
}

fn plural(n: usize, word: &str) -> String {
    if n == 1 {
        format!("1 {word}")
    } else {
        format!("{n} {word}s")
    }
}

/// How many trailing non-blank lines can belong to an agent's input UI.
const CHROME_TAIL: usize = 10;

/// Drops blank lines and the agent's input UI at the bottom of the screen:
/// everything from the first separator rule or input prompt in the last
/// lines. Covers Claude Code (`─` rule, `❯` prompt, status bar), Codex
/// (`›` prompt or popup options, status line) and pi (`─` box, cwd and cost
/// lines).
fn strip_chrome(raw: &str) -> Vec<String> {
    let mut lines: Vec<&str> = raw.lines().filter(|l| !l.trim().is_empty()).collect();
    let tail_start = lines.len().saturating_sub(CHROME_TAIL);
    if let Some(offset) = lines[tail_start..].iter().position(|l| is_chrome_start(l)) {
        lines.truncate(tail_start + offset);
    }
    lines.iter().map(|l| l.trim_end().to_string()).collect()
}

fn is_chrome_start(line: &str) -> bool {
    let t = line.trim();
    let rule = t.chars().count() >= 8 && t.chars().all(|c| c == '─');
    rule || t.starts_with('❯') || t.starts_with('›')
}

/// Terminal titles from Codex and pi only name the program or directory.
/// Strip their prefixes and show `-` when nothing about the task is left.
fn clean_title(title: &str, cwd: &str) -> String {
    // Codex appends a braille spinner while it works: `vega ⠹`.
    let mut t = title
        .trim_end_matches(|c: char| c.is_whitespace() || ('\u{2800}'..='\u{28FF}').contains(&c))
        .trim();
    for prefix in ["[ ! ] Action Required | ", "π - "] {
        t = t.strip_prefix(prefix).unwrap_or(t);
    }
    let dir = cwd.rsplit('/').next().unwrap_or("");
    if t.is_empty() || t == dir {
        "-".into()
    } else {
        t.to_string()
    }
}

fn shorten(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let head: String = s.chars().take(max - 1).collect();
        format!("{head}…")
    }
}

/// Mail through vega post when vegad delivers it (local relay running, or
/// another machine): the target gets one pointer when it is free, and the
/// reply command once per inbox batch. `--now`, no relay here, or a relay in
/// local mode (no broker): a direct paste with a one-line footer.
fn send(target: &str, text: &str, now: bool) -> Result<(), Failure> {
    let relay = relay_running() && !vega_herdr::state::relay_local();
    if !now && (machine().is_some() || relay) {
        match send_post(target, text) {
            Ok(pane) => {
                println!("sent: {target} ({pane}) as mail; it gets a pointer when it is free (from its parent: also while it works)");
                println!("help: the reply arrives as your own pointer; do not poll; --now pastes at once");
                return Ok(());
            }
            // The broker is away: the paste still works on this machine.
            Err(f) if f.code == "broker_unreachable" && machine().is_none() => {
                println!("note: broker unreachable, pasting directly");
            }
            Err(f) => return Err(f),
        }
    }
    deliver(target, text, Footer::Peer)?;
    println!("sent: {target} (pasted)");
    println!("help: the reply arrives as a new prompt in this session; do not poll");
    Ok(())
}

/// `vega post send` to the target's pane; returns the pane id.
fn send_post(target: &str, text: &str) -> Result<String, Failure> {
    let a = herdr(&["agent", "get", target])?["result"]["agent"].clone();
    let pane = str_of(&a, "pane_id").to_string();
    let mut args = vec!["post".to_string(), "send".into()];
    if let Some(m) = machine() {
        args.push("--host".into());
        args.push(m.to_lowercase());
    }
    args.push(pane.clone());
    args.push(text.to_string());
    let out = Command::new("vega")
        .args(&args)
        .output()
        .map_err(|e| Failure::new("vega_not_found", e.to_string(), "install vega"))?;
    if out.status.success() {
        return Ok(pane);
    }
    // vega post prints `error{code,message,hint}:` then one row.
    let stdout = String::from_utf8_lossy(&out.stdout);
    let row = stdout.lines().nth(1).unwrap_or("").trim().to_string();
    let code = row.split(',').next().unwrap_or("post_failed").to_string();
    Err(Failure::new(
        &code,
        row,
        "see `vega post`; --now pastes directly",
    ))
}

/// The BEADS_ACTOR a local agent in `cwd` gets: `<host>/<worktree dir>`, the
/// value the dotfiles bd shim derives, so a lane's beads carry its actor on a
/// host without the shim too. None outside a git repo (the shim would derive
/// per call there) and on another machine (the actor is that host's).
fn beads_actor(cwd: &str) -> Option<String> {
    if machine().is_some() {
        return None;
    }
    lanes::actor(Path::new(cwd))
}

fn actor_env(actor: &str) -> String {
    format!("BEADS_ACTOR={actor}")
}

/// `--env K=V`: a shell variable name, then any value (empty too).
fn parse_env(s: &str) -> Result<String, String> {
    let (k, _) = s
        .split_once('=')
        .ok_or_else(|| format!("{s} is not K=V, e.g. VEGA_GATE_ROLE=reviewer"))?;
    let mut chars = k.chars();
    let valid = chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_');
    if !valid {
        return Err(format!(
            "{k} is not a variable name: [A-Za-z_][A-Za-z0-9_]*"
        ));
    }
    Ok(s.to_string())
}

/// The pane env of a spawned agent: lane actor, caller socket and colour defaults,
/// then `--env`, whose explicit values win. herdr launches from its server env,
/// so the caller's SSH agent socket must be forwarded explicitly.
/// herdr sets it on the pane's shell; claude,
/// codex (`--no-daemon`) and pi pass it on to their tool shells.
fn spawn_env(
    kind: &str,
    actor: Option<&str>,
    socket: Option<&str>,
    user: &[String],
) -> Vec<String> {
    let own = user.iter().any(|e| e.starts_with("BEADS_ACTOR="));
    let color = (kind == "codex" && !user.iter().any(|e| e.starts_with("FORCE_COLOR=")))
        .then(|| "FORCE_COLOR=1".to_string());
    let socket = socket
        .filter(|_| !user.iter().any(|e| e.starts_with("SSH_AUTH_SOCK=")))
        .map(|socket| format!("SSH_AUTH_SOCK={socket}"));
    actor
        .filter(|_| !own)
        .map(actor_env)
        .into_iter()
        .chain(socket)
        .chain(color)
        .chain(user.iter().cloned())
        .collect()
}

/// herdr's `--env` flags for `env`, one per variable.
fn env_args(env: &[String]) -> Vec<&str> {
    env.iter().flat_map(|e| ["--env", e.as_str()]).collect()
}

/// Codex filters the pane env again for tool shells (`inherit = "core"`
/// drops all but a few). Set just the keys vega needs there, each `(name,
/// caller value)`, preserving the project's other policy fields: the SSH agent
/// socket and the herdr pane identity (`HERDR_PANE_ID`, `HERDR_SOCKET_PATH`)
/// that `vega ci watch`, the gate and the orchestration read. Explicit env
/// beats the caller; extra CLI overrides come last so Codex gives them precedence.
fn spawn_args(
    kind: &str,
    keep: &[(&str, Option<&str>)],
    user: &[String],
    extra: &[String],
) -> Vec<String> {
    let mut args: Vec<String> = default_args(kind).iter().map(|s| (*s).into()).collect();
    if kind == "codex" {
        for (name, caller) in keep {
            let prefix = format!("{name}=");
            let value = user
                .iter()
                .rev()
                .find_map(|e| e.strip_prefix(&prefix))
                .or(*caller);
            if let Some(value) = value {
                // JSON quoting plus escaping DEL gives a TOML basic string too.
                let value = serde_json::to_string(value)
                    .expect("env string serializes")
                    .replace('\u{7f}', "\\u007f");
                args.extend([
                    "-c".into(),
                    format!("shell_environment_policy.set.{name}={value}"),
                ]);
            }
        }
    }
    args.extend_from_slice(extra);
    args
}

/// Replaces the fresh shell pane `pane` with a split of it whose shell has
/// the spawn env set; the new pane.
fn with_env(pane: &str, cwd: &str, env: &[String]) -> Result<String, Failure> {
    let mut args = vec![
        "pane",
        "split",
        "--pane",
        pane,
        "--direction",
        "right",
        "--cwd",
        cwd,
        "--no-focus",
    ];
    args.extend(env_args(env));
    let v = herdr(&args)?;
    let new = str_of(&v["result"]["pane"], "pane_id").to_string();
    // A leftover empty shell is harmless; the spawn goes on in the new pane.
    let _ = herdr(&["pane", "close", pane]);
    Ok(new)
}

fn start_spawn(
    pane: &str,
    name: &str,
    mode: Mode,
    mut start: impl FnMut() -> Result<Value, Failure>,
) -> Result<Value, Failure> {
    save_mode(machine(), pane, name, mode);
    // A fresh pane can still be busy with its shell start: one retry.
    match start() {
        Err(f) if f.code == "agent_pane_busy" => {
            std::thread::sleep(std::time::Duration::from_secs(1));
            start()
        }
        r => r,
    }
}

fn close_failed_spawn(pane: &str, close: impl FnOnce() -> Result<Value, Failure>) -> bool {
    if close().is_err() {
        return false;
    }
    forget(pane);
    true
}

fn spawn_mode(headless: bool) -> Mode {
    if headless {
        Mode::Headless
    } else {
        Mode::Headed
    }
}

#[allow(clippy::too_many_arguments)]
fn spawn(
    name: &str,
    kind: &str,
    cwd: Option<String>,
    place: Place,
    prompt: Option<&str>,
    user_env: &[String],
    no_parent: bool,
    mode: Mode,
    extra: &[String],
) -> Result<(), Failure> {
    spawn_inner(
        name, kind, cwd, place, prompt, user_env, no_parent, mode, extra, None,
    )
    .map(|_| ())
}

#[allow(clippy::too_many_arguments)]
fn spawn_inner(
    name: &str,
    kind: &str,
    cwd: Option<String>,
    place: Place,
    prompt: Option<&str>,
    user_env: &[String],
    no_parent: bool,
    mode: Mode,
    extra: &[String],
    reopening: Option<&vega_herdr::state::SpawnRecord>,
) -> Result<String, Failure> {
    if machine().is_some() && matches!(place, Place::At(At::Split)) {
        return Err(Failure::new(
            "usage",
            "--at split needs a pane on this machine",
            "use --at tab or --at workspace with --machine",
        ));
    }
    // Headroom of the kind's provider, read while the agent starts.
    let capacity = {
        let (kind, extra) = (kind.to_string(), extra.to_vec());
        std::thread::spawn(move || {
            // An unreadable ledger covers nothing: the warning stays.
            let ledger = vega_herdr::resets::read().unwrap_or_default();
            capacity::spawn_check(&kind, &extra, capacity::read(), &ledger, capacity::now())
        })
    };
    if !valid_agent_name(name) {
        return Err(Failure::new(
            "invalid_agent_name",
            format!("{name} is not a valid agent name"),
            "use a short kebab-case name for the task (it labels the tab): [a-z][a-z0-9-], at most 24 chars, e.g. link-parsing or bead-6-92",
        ));
    }
    // Check before creating a pane, so a taken name leaves no empty pane behind.
    match herdr(&["agent", "get", name]) {
        Ok(v) => {
            return Err(Failure::new(
                "name_taken",
                format!(
                    "{name} is already running in {}",
                    str_of(&v["result"]["agent"], "pane_id")
                ),
                "pick another name, or `vega mux send` to the running agent",
            ))
        }
        Err(f) if f.code == "agent_not_found" => {}
        Err(f) => return Err(f),
    }
    // herdr ignores a `~/…` cwd (the shell opens in $HOME), so expand it with
    // the home of the machine the agent runs on.
    let cwd = match (cwd, machine()) {
        (Some(c), None) => expand_home(&c, &env::var("HOME").unwrap_or_default()),
        (Some(c), Some(m)) if c.starts_with('~') => expand_home(&c, &remote_home(m)?),
        (Some(c), Some(_)) => c,
        (None, None) => env::current_dir()
            .map(|p| p.display().to_string())
            .map_err(|e| Failure::new("no_cwd", e.to_string(), "pass --cwd <dir>"))?,
        (None, Some(_)) => {
            return Err(Failure::new(
                "usage",
                "--cwd is required with --machine",
                "pass a directory on that machine, e.g. --cwd ~/src/project",
            ))
        }
    };

    if machine().is_none() && !no_parent {
        // Validate before creating a pane/worktree or starting an agent.
        vega_herdr::project::resolve_optional(Path::new(&cwd), &vega_herdr::state::post_host())?;
    }

    // The tab is labelled with the short agent name; the task is saved per
    // pane instead (`save_task`), as the title fallback for Codex and pi.
    let mut tree: Option<worktree::Opened> = None;
    let actor;
    let socket = std::env::var("SSH_AUTH_SOCK").ok();
    let (pane, place) = match place {
        Place::Worktree { branch, base } => {
            let w = worktree::open(&cwd, &branch, base.as_deref())?;
            // Cosmetic: the workspace already carries the branch name.
            let _ = herdr(&["tab", "rename", &w.tab, name]);
            // herdr's worktree commands take no --env: swap the root pane for
            // a split of it that has the env.
            actor = beads_actor(&w.path);
            let env = spawn_env(kind, actor.as_deref(), socket.as_deref(), user_env);
            let pane = if env.is_empty() {
                w.pane.clone()
            } else {
                with_env(&w.pane, &w.path, &env)?
            };
            let r = (pane, format!("workspace {} (worktree)", w.workspace));
            tree = Some(w);
            r
        }
        Place::At(At::Tab) => {
            // Locally, always pass our workspace: without it herdr uses the
            // focused one. On another machine the focused workspace is right.
            let workspace = match machine() {
                None => Some(me()?.workspace),
                Some(_) => None,
            };
            let mut args = vec!["tab", "create"];
            if let Some(ws) = &workspace {
                args.extend(["--workspace", ws]);
            }
            args.extend(["--cwd", &cwd, "--label", name, "--no-focus"]);
            actor = beads_actor(&cwd);
            let env = spawn_env(kind, actor.as_deref(), socket.as_deref(), user_env);
            args.extend(env_args(&env));
            let v = herdr(&args)?;
            let tab = str_of(&v["result"]["tab"], "tab_id").to_string();
            (
                str_of(&v["result"]["root_pane"], "pane_id").to_string(),
                format!("tab {tab}"),
            )
        }
        Place::At(At::Split) => {
            let me = me()?;
            let layout = herdr(&["pane", "layout", "--pane", &me.pane])?;
            let direction = split_direction(&layout["result"]["layout"], &me.pane);
            actor = beads_actor(&cwd);
            let env = spawn_env(kind, actor.as_deref(), socket.as_deref(), user_env);
            let mut args = vec![
                "pane",
                "split",
                "--pane",
                &me.pane,
                "--direction",
                direction,
                "--cwd",
                &cwd,
                "--no-focus",
            ];
            args.extend(env_args(&env));
            let v = herdr(&args)?;
            (
                str_of(&v["result"]["pane"], "pane_id").to_string(),
                format!("split {direction} of {}", me.pane),
            )
        }
        Place::At(At::Workspace) => {
            actor = beads_actor(&cwd);
            let env = spawn_env(kind, actor.as_deref(), socket.as_deref(), user_env);
            let mut args = vec![
                "workspace",
                "create",
                "--cwd",
                &cwd,
                "--label",
                name,
                "--no-focus",
            ];
            args.extend(env_args(&env));
            let v = herdr(&args)?;
            let ws = str_of(&v["result"]["workspace"], "workspace_id").to_string();
            (
                str_of(&v["result"]["root_pane"], "pane_id").to_string(),
                format!("workspace {ws}"),
            )
        }
    };

    let mut args: Vec<&str> = vec![
        "agent",
        "start",
        name,
        "--kind",
        kind,
        "--pane",
        &pane,
        "--timeout",
        "60000",
        "--",
    ];
    let cwd = tree.as_ref().map_or(cwd, |w| w.path.clone());
    // Trust lives in local config files: a remote agent keeps its own.
    let trust = match machine() {
        None => trust::apply(kind, &cwd),
        Some(_) => None,
    };
    // A failed start keeps the worktree: it may hold the user's work.
    let kept = tree
        .as_ref()
        .map(|w| format!("; worktree {} kept", collapse_home(&w.path)))
        .unwrap_or_default();
    // herdr's socket is per machine: only a local spawn knows it.
    let herdr_socket = std::env::var("HERDR_SOCKET_PATH")
        .ok()
        .filter(|p| !p.is_empty() && machine().is_none());
    let keep = [
        ("SSH_AUTH_SOCK", socket.as_deref()),
        ("HERDR_PANE_ID", Some(pane.as_str())),
        ("HERDR_SOCKET_PATH", herdr_socket.as_deref()),
    ];
    let agent_args = spawn_args(kind, &keep, user_env, extra);
    args.extend(agent_args.iter().map(String::as_str));
    // Record every spawn before startup, including one that stops at a dialog.
    // Resume keeps this choice; legacy children without a record stay headless.
    let mut lineage = reopening
        .cloned()
        .unwrap_or_else(|| vega_herdr::state::SpawnRecord {
            pane: pane.clone(),
            name: name.into(),
            kind: kind.into(),
            cwd: reopen::recorded_cwd(&cwd),
            parent: me()
                .ok()
                .filter(|_| !no_parent)
                .map(|m| vega_herdr::state::Parent {
                    pane: m.pane,
                    host: local_host(),
                }),
            mode: mode.as_str().into(),
            task: prompt.and_then(task_label),
            session: None,
            extra: extra.to_vec(),
        });
    lineage.pane = pane.clone();
    let saved = reopen::save_best_effort(&lineage);
    // The replacement already holds the full identity, even if startup stops
    // at a dialog. Keep one record per reopened agent so its name stays usable.
    if let Some(old) = reopening.filter(|old| saved && old.pane != pane) {
        if let Some(file) = state_file("spawns", &old.pane) {
            let _ = std::fs::remove_file(file);
        }
    }
    vega_herdr::state::restore_parent(&pane, name, lineage.parent.as_ref());
    let started = start_spawn(&pane, name, mode, || herdr(&args));
    reopen::capture(&mut lineage);
    reopen::save_best_effort(&lineage);
    let record = prompt.map(|text| KeptBrief {
        name: name.to_string(),
        kind: kind.to_string(),
        cwd: cwd.clone(),
        brief: text.to_string(),
        no_parent,
    });
    if let Err(f) = started {
        // For the user: `io:w1:p2` on another machine, so selecting it in
        // herdr (vega open) jumps there.
        let at = qualify(machine(), &pane);
        // The agent never started, or exited at once: keep its last words,
        // then close our pane.
        if no_agent(&pane) {
            let screen =
                herdr_text(&["pane", "read", &pane, "--source", "recent", "--lines", "40"])
                    .unwrap_or_default();
            if close_failed_spawn(&pane, || herdr(&["pane", "close", &pane])) {
                let brief = if prompt.is_some() {
                    "; brief: not delivered"
                } else {
                    ""
                };
                let lines = last_output(&screen);
                return Err(Failure::new(
                    &f.code,
                    format!(
                        "{} (pane {at} closed{kept}{brief}){}",
                        f.message,
                        output_tail(&lines)
                    ),
                    startup_hint(&lines),
                ));
            }
        }
        // The pane stays open: keep the brief so it can follow the dialog.
        let (brief, then) = undelivered(&pane, record.as_ref());
        // A folder trust dialog is a human decision: never confirm it.
        let screen =
            herdr_text(&["pane", "read", &pane, "--source", "visible"]).unwrap_or_default();
        if asks_folder_trust(&screen) {
            return Err(Failure::new(
                "trust_required",
                format!("{name} asks whether to trust {cwd} (pane {at} stays open{kept}{brief})"),
                &format!(
                    "tell the user to confirm it in pane {at}, or spawn in a folder the agent already trusts; vega never confirms trust{then}"
                ),
            ));
        }
        // Codex asks per hooks.json content; a worktree inherits the main
        // checkout's approvals, so one approval there covers all worktrees.
        if asks_hooks_review(&screen) {
            return Err(Failure::new(
                "hooks_review_required",
                format!("{name} asks to review the repo's codex hooks (pane {at} stays open{kept}{brief})"),
                &format!(
                    "tell the user to review them in pane {at}; approved once in the main checkout, new worktrees inherit it; vega never approves new hooks{then}"
                ),
            ));
        }
        // Any other dialog the agent stopped at, e.g. codex's update screen.
        if let Some(d) = vega_herdr::dialog::classify(&screen, kind, "unknown") {
            return Err(Failure::new(
                &f.code,
                format!(
                    "{} at the {} dialog: {} (pane {at} stays open{kept}{brief})",
                    f.message,
                    d.kind.as_str(),
                    d.title
                ),
                &format!("{}{then}", dialog_hint(&d.kind, &at)),
            ));
        }
        return Err(Failure::new(
            &f.code,
            format!("{} (pane {at} stays open{kept}{brief})", f.message),
            &format!(
                "run `vega mux{} show {name}` to see the screen (login prompt?){then}",
                mflag()
            ),
        ));
    }

    // Parent and task are kept per pane on this machine, for `vega uac`,
    // `show` and the title fallback; both replace a stale record of a reused pane.
    // The UAC's sentinel in another worktree is a lane: `vega uac status`
    // labels the beads it creates (vega_herdr::lanes). `--no-parent` keeps
    // none of them: the agent reports by its own command.
    let mut lane = None;
    if let Some(me) = me().ok().filter(|_| !no_parent && reopening.is_none()) {
        save_parent(&pane, name, &me.pane);
        if machine().is_none() {
            let here = env::current_dir().unwrap_or_default();
            lane = lanes::record_if_uac(Path::new(&cwd), &here, &me.pane)?;
        }
    }
    save_task(&pane, lineage.task.as_deref());

    println!("spawned: {name} ({kind})");
    println!("pane: {pane}");
    println!("at: {place}");
    println!("cwd: {}", cell(&collapse_home(&cwd)));
    // Keys only: a value may be a secret.
    if !user_env.is_empty() {
        let keys: Vec<&str> = user_env
            .iter()
            .filter_map(|e| e.split('=').next())
            .collect();
        println!("env: {}", keys.join(" "));
    }
    // An --env BEADS_ACTOR replaced the lane's; the env line names it.
    if let Some(a) = actor.filter(|_| !user_env.iter().any(|e| e.starts_with("BEADS_ACTOR="))) {
        println!("beads_actor: {a}");
    }
    if let Some(w) = &tree {
        println!("worktree: {} ({})", cell(&w.branch), w.how);
    }
    if let Some(line) = &trust {
        println!("{line}");
    }
    if let Some(actor) = &lane {
        println!(
            "lane: {actor} (vega uac status labels its beads {})",
            lanes::LABEL
        );
    }
    println!("args: {}", cell(&agent_args.join(" ")));
    if let Ok((field, warning)) = capacity.join() {
        println!("{field}");
        if let Some(w) = warning {
            eprintln!("{w}");
        }
    }
    match prompt {
        Some(_) => deliver_brief(&pane, record.as_ref().expect("brief record"), &kept)?,
        None => println!("help: vega mux{} send {name} \"<task>\"", mflag()),
    }
    // Some harnesses publish their session only after the first prompt.
    reopen::capture(&mut lineage);
    reopen::save_best_effort(&lineage);
    Ok(pane)
}

/// Saves the brief of a spawn whose pane stays open without it. Returns the
/// message part (`; brief: not delivered`) and the hint part with the command
/// that delivers it later; both empty without a brief.
fn undelivered(pane: &str, record: Option<&KeptBrief>) -> (String, String) {
    let Some(record) = record else {
        return (String::new(), String::new());
    };
    let then = if keep_brief(pane, record) {
        format!(
            "; then deliver the brief: vega mux{} spawn --resume {pane}",
            mflag()
        )
    } else {
        format!(
            "; then deliver the brief: vega mux{} send {pane} --now --file <brief>",
            mflag()
        )
    };
    ("; brief: not delivered".into(), then)
}

/// Delivers a spawned agent's brief with the task footer and reports it. A
/// failure states whether the brief arrived and how to deliver it.
fn deliver_brief(pane: &str, record: &KeptBrief, kept: &str) -> Result<(), Failure> {
    let at = qualify(machine(), pane);
    let mode = mode_of(machine(), pane, &record.name);
    let (footer, _) = handoff(record.no_parent, mode);
    match wait_started(pane).and_then(|()| deliver(pane, &record.brief, footer)) {
        Ok(()) => {
            drop_kept_brief(pane);
            println!("prompt: taken");
            if record.no_parent {
                println!("help: no parent recorded: it reports by its own command; do not poll");
            } else if mode == Mode::Headed {
                println!("help: vegad wakes you when it reports; questions stay in its pane for the user; do not poll");
            } else {
                println!(
                    "help: vegad wakes you when it reports or needs you; see `vega uac`; do not poll"
                );
            }
            Ok(())
        }
        // Pasted, but the agent did not start: resending could duplicate it.
        Err(f) if f.code == "not_taken" => {
            keep_brief(pane, record);
            Err(Failure::new(
                &f.code,
                format!("{} (pane {at}{kept}; brief: pasted, not confirmed)", f.message),
                &format!(
                    "run `vega mux{m} show {pane}`: if the agent got the brief, do not resend; if not, vega mux{m} spawn --resume {pane}",
                    m = mflag()
                ),
            ))
        }
        Err(f) => {
            let (brief, then) = undelivered(pane, Some(record));
            Err(Failure::new(
                &f.code,
                format!("{} (pane {at} stays open{kept}{brief})", f.message),
                &format!("{}{then}", f.hint),
            ))
        }
    }
}

/// `spawn --resume`: delivers the brief a spawn saved when it stopped at a
/// dialog, once the user has answered it. Never touches the dialog itself.
fn resume_spawn(pane: &str) -> Result<(), Failure> {
    let record = kept_brief(pane).ok_or_else(|| {
        Failure::new(
            "no_brief",
            format!("no undelivered brief saved for pane {pane}"),
            "only a spawn that stopped before its brief saves one; send a task with `vega mux send`",
        )
    })?;
    let (_, dialog) = dialog::read(pane)?;
    let agent = herdr(&["agent", "get", pane])?;
    let name = record.name.clone();
    if !same_agent(&record, &agent["result"]["agent"]) {
        return Err(Failure::new(
            "pane_reused",
            format!("pane {} no longer runs {name}", qualify(machine(), pane)),
            "spawn the agent again with its brief",
        ));
    }
    if let Some(d) = dialog {
        return Err(Failure::new(
            "dialog_open",
            format!(
                "{name} still shows the {} dialog (brief: not delivered)",
                d.kind.as_str()
            ),
            &format!(
                "{}; then run this again",
                dialog_hint(&d.kind, &qualify(machine(), pane))
            ),
        ));
    }
    if machine().is_none() && !record.no_parent {
        vega_herdr::project::resolve_optional(
            Path::new(str_of(&agent["result"]["agent"], "cwd")),
            &vega_herdr::state::post_host(),
        )?;
    }
    let mut lane = None;
    if let Some(me) = me().ok().filter(|_| !record.no_parent) {
        save_parent(pane, &name, &me.pane);
        if machine().is_none() {
            let cwd = str_of(&agent["result"]["agent"], "cwd");
            let here = env::current_dir().unwrap_or_default();
            lane = lanes::record_if_uac(Path::new(cwd), &here, &me.pane)?;
        }
    }
    save_task(pane, task_label(&record.brief).as_deref());
    println!("resumed: {name}");
    println!("pane: {pane}");
    if let Some(actor) = &lane {
        println!(
            "lane: {actor} (vega uac status labels its beads {})",
            lanes::LABEL
        );
    }
    deliver_brief(pane, &record, "")
}

/// Whether `agent` (herdr's agent record) still runs the spawn `record` was
/// kept for. herdr drops the agent name while it sits at a startup screen
/// (codex's update prompt); then its kind and cwd must match.
fn same_agent(record: &KeptBrief, agent: &Value) -> bool {
    match agent["name"].as_str().filter(|n| !n.is_empty()) {
        Some(n) => n == record.name,
        None => {
            !record.kind.is_empty()
                && str_of(agent, "agent") == record.kind
                && same_dir(str_of(agent, "cwd"), &record.cwd)
        }
    }
}

/// Paths equal after resolving symlinks (herdr reports /private/tmp for
/// /tmp on macOS); unresolvable ones (remote machines) compare as text.
fn same_dir(a: &str, b: &str) -> bool {
    let real = |p: &str| std::fs::canonicalize(p).unwrap_or_else(|_| PathBuf::from(p));
    !a.is_empty() && real(a) == real(b)
}

/// Who answers a dialog vega stopped at. vega answers none of them. `pane`
/// is machine-qualified, as the user sees it.
fn dialog_hint(kind: &vega_herdr::dialog::Kind, pane: &str) -> String {
    use vega_herdr::dialog::Kind;
    match kind {
        Kind::Update => {
            format!("the user picks Skip or updates codex in pane {pane}; vega never answers it")
        }
        Kind::Trust | Kind::HooksTrust => {
            format!("tell the user to answer it in pane {pane}; vega never confirms trust")
        }
        _ => format!("tell the user to answer it in pane {pane}"),
    }
}

/// Claude Code's "Quick safety check: Is this a project you … trust?" and
/// similar folder trust prompts of other agents.
fn asks_folder_trust(screen: &str) -> bool {
    let s = screen.to_lowercase();
    s.contains("trust this folder") || s.contains("do you trust")
}

/// Codex's "Hooks need review" screen for new or changed project hooks.
fn asks_hooks_review(screen: &str) -> bool {
    screen.to_lowercase().contains("hooks need review")
}

/// Replaces a leading `~` with `home`.
fn expand_home(path: &str, home: &str) -> String {
    match path.strip_prefix('~') {
        Some(rest) if rest.is_empty() || rest.starts_with('/') => format!("{home}{rest}"),
        _ => path.to_string(),
    }
}

/// `$HOME` on a herdr machine, read over SSH from the machine's saved target.
/// How to remove the worktree of a closed workspace: `wt remove` inside it,
/// over ssh when the workspace was on another machine.
fn remove_hint(checkout: &str) -> String {
    match machine() {
        None => format!(
            "remove the worktree with: (cd {} && wt remove)",
            cell(checkout)
        ),
        Some(label) => {
            let target = ssh_target(label).unwrap_or_else(|| label.to_string());
            format!("remove the worktree with: ssh {target} 'cd {checkout} && wt remove'")
        }
    }
}

/// The ssh target herdr uses for a machine label.
fn ssh_target(label: &str) -> Option<String> {
    herdr_local(&["machine", "list", "--json"])
        .ok()?
        .as_array()?
        .iter()
        .find(|m| m["label"] == label)?["target"]
        .as_str()
        .map(String::from)
}

fn remote_home(label: &str) -> Result<String, Failure> {
    let list = herdr_local(&["machine", "list", "--json"])?;
    let target = list
        .as_array()
        .and_then(|ms| ms.iter().find(|m| m["label"] == label))
        .and_then(|m| m["target"].as_str())
        .ok_or_else(|| {
            Failure::new(
                "unknown_machine",
                format!("no herdr machine labelled {label}"),
                "run `herdr machine list` to see saved machines",
            )
        })?;
    let out = Command::new("ssh")
        .args(["-o", "BatchMode=yes", "-o", "ConnectTimeout=10", target])
        .arg("printf %s \"$HOME\"")
        .output()
        .map_err(|e| Failure::new("ssh_failed", e.to_string(), "install OpenSSH"))?;
    let home = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if !out.status.success() || home.is_empty() {
        return Err(Failure::new(
            "ssh_failed",
            format!("cannot read $HOME on {label} via ssh {target}"),
            "pass an absolute --cwd instead of ~/…",
        ));
    }
    Ok(home)
}

/// The last lines a pane showed, blank lines dropped and secrets masked:
/// what an agent printed before it exited at startup.
fn last_output(screen: &str) -> Vec<String> {
    const LINES: usize = 6;
    const WIDTH: usize = 200;
    let lines: Vec<&str> = screen
        .lines()
        .map(str::trim_end)
        .filter(|l| !l.trim().is_empty())
        .collect();
    lines[lines.len().saturating_sub(LINES)..]
        .iter()
        .map(|l| redact(&l.chars().take(WIDTH).collect::<String>()))
        .collect()
}

/// `; last output: a | b`, or nothing when the pane showed nothing.
fn output_tail(lines: &[String]) -> String {
    if lines.is_empty() {
        String::new()
    } else {
        format!("; last output: {}", lines.join(" | "))
    }
}

/// The --kind hint only fits a missing binary; otherwise the output says why.
fn startup_hint(lines: &[String]) -> &'static str {
    let missing = lines.iter().any(|l| {
        let l = l.to_lowercase();
        l.contains("command not found") || l.contains("no such file or directory")
    });
    if missing || lines.is_empty() {
        "check --kind against `herdr agent` and the extra args, then spawn again"
    } else {
        "fix the extra args from the agent's last output (e.g. pi: --provider X or --model provider/model), then spawn again"
    }
}

/// Masks what looks like a secret: the value of a `*key*=`/`*token*:`-style
/// field, a `Bearer` token, and any long run of letters and digits.
fn redact(line: &str) -> String {
    const MASK: &str = "[redacted]";
    let secretish = |name: &str| {
        let n = name.to_lowercase();
        ["key", "token", "secret", "password", "passwd", "auth"]
            .iter()
            .any(|w| n.contains(w))
    };
    let mut out = Vec::new();
    let mut bearer = false;
    for word in line.split(' ') {
        let masked = if bearer {
            MASK.to_string()
        } else if let Some((name, _)) = word
            .split_once(['=', ':'])
            .filter(|(n, v)| !v.is_empty() && secretish(n.trim_start_matches(['-', '"', '\''])))
        {
            let sep = &word[name.len()..name.len() + 1];
            format!("{name}{sep}{MASK}")
        } else {
            mask_runs(word, MASK)
        };
        // `Bearer <t>`, `api_key: <t>`: the next word is the secret.
        bearer = word.eq_ignore_ascii_case("bearer")
            || word
                .strip_suffix([':', '='])
                .is_some_and(|n| secretish(n.trim_start_matches(['-', '"', '\''])));
        out.push(masked);
    }
    out.join(" ")
}

/// Replaces runs of `[A-Za-z0-9_-]` of 20+ chars that mix letters and digits.
fn mask_runs(word: &str, mask: &str) -> String {
    let is_run = |c: char| c.is_ascii_alphanumeric() || c == '_' || c == '-';
    let mut out = String::new();
    let mut run = String::new();
    let flush = |run: &mut String, out: &mut String| {
        let mixed =
            run.chars().any(|c| c.is_ascii_digit()) && run.chars().any(|c| c.is_ascii_alphabetic());
        out.push_str(if run.len() >= 20 && mixed { mask } else { run });
        run.clear();
    };
    for c in word.chars() {
        if is_run(c) {
            run.push(c);
        } else {
            flush(&mut run, &mut out);
            out.push(c);
        }
    }
    flush(&mut run, &mut out);
    out
}

/// True if herdr detects no agent in the pane. A blocked agent (trust or
/// login prompt) is detected and keeps its pane for inspection.
fn no_agent(pane: &str) -> bool {
    herdr(&["pane", "get", pane]).is_ok_and(|v| v["result"]["pane"]["agent"].is_null())
}

fn valid_agent_name(name: &str) -> bool {
    let mut chars = name.chars();
    name.len() <= 24
        && chars.next().is_some_and(|c| c.is_ascii_lowercase())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// Splits a wide pane to the right and a narrow one down, as herdr's guide
/// recommends. A cell is about twice as tall as wide.
fn split_direction(layout: &Value, pane: &str) -> &'static str {
    let rect = layout["panes"]
        .as_array()
        .and_then(|ps| ps.iter().find(|p| p["pane_id"] == pane))
        .map(|p| &p["rect"]);
    match rect {
        Some(r) if r["width"].as_u64().unwrap_or(0) >= 2 * r["height"].as_u64().unwrap_or(0) => {
            "right"
        }
        _ => "down",
    }
}

/// The last line of the reply footer.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Footer {
    /// `send`: a peer replies only if it adds something, so agents do not loop.
    Peer,
    /// `spawn --prompt`: the spawner waits for the child's result.
    Task(Mode),
    /// `spawn --no-parent`: the origin line only; the agent reports by its
    /// own command.
    Plain,
}

/// Sends `text` plus the reply footer as a new prompt and confirms the target
/// took it.
fn deliver(target: &str, text: &str, footer: Footer) -> Result<(), Failure> {
    let Me {
        pane: me,
        agent: kind,
        cwd,
        ..
    } = me()?;
    if machine().is_none() && target == me {
        return Err(Failure::new(
            "self_send",
            format!("{target} is your own pane"),
            "run `vega mux` to list other agents",
        ));
    }

    // Across machines the reply goes through the broker (`vega post`), not
    // back over ssh: other machines may not reach this one (tailnet policy).
    let (origin, reply_cmd) = match machine() {
        Some(_) => {
            let host = local_host();
            (
                format!("{me} on {host}"),
                format!("vega post send --host {host} {me}"),
            )
        }
        None => (me.clone(), format!("vega mux send {me}")),
    };
    // The time is for the human reading the transcript later.
    let sent = local_time(None)
        .map(|t| format!(", {t}"))
        .unwrap_or_default();
    // An inbox-only target machine (corp-managed, `auth-config --inbox-only`)
    // cannot mail agents: its replies go into quarantine, a note for the human
    // (vega-lhc.83), so the footer says where they end up.
    let quarantined = machine().is_some_and(vega_herdr::state::is_inbox_only);
    let host = machine().map(|_| local_host());
    let footer = footer_text(footer, &me, &reply_cmd, host.as_deref(), quarantined);
    let body = format!("{text}\n\n---\n[vega-mux] from {origin} ({kind} in {cwd}){sent}{footer}");
    herdr(&["agent", "prompt", target, &body])?;
    confirm_taken(target, &body)?;
    if machine().is_none() {
        supervise(&me, target);
    }
    Ok(())
}

/// What a spawn's brief carries and whether the spawner becomes the agent's
/// parent (parent record, UAC lane): with `--no-parent`, neither the
/// done-report footer nor a parent.
fn handoff(no_parent: bool, mode: Mode) -> (Footer, bool) {
    if no_parent {
        (Footer::Plain, false)
    } else {
        (Footer::Task(mode), true)
    }
}

/// The footer after the origin line. `host`: this machine's name when the
/// target runs on another one.
fn footer_text(
    footer: Footer,
    me: &str,
    reply_cmd: &str,
    host: Option<&str>,
    quarantined: bool,
) -> String {
    match footer {
        Footer::Peer if quarantined => format!(" · reply only if it adds something: {reply_cmd} \"<reply>\" (your host is inbox-only: the reply goes into quarantine, a note for the human; I do not see it)"),
        Footer::Peer => format!(" · reply only if it adds something: {reply_cmd} \"<reply>\""),
        Footer::Plain => String::new(),
        Footer::Task(_) if quarantined => {
            let host = format!(" --host {}", host.unwrap_or_default());
            // Inbox-only mail cannot reach the parent; the human must help.
            let questions = task_questions(Mode::Headed, me, &host);
            format!(
                "\nYour host is inbox-only: your mail goes into quarantine, a note for the human; I never see it.\nWhen done, run once: vega post done --to {me}{host} --outcome succeeded|failed \"<one-line result>\" (the human reads it).\n{questions}\nFollow-ups: a bead only for work this task cannot do, always --deps discovered-from:<your bead>; without beads, name it in your report."
            )
        }
        // The parent's relay records the outcome and wakes the parent.
        Footer::Task(mode) => {
            let host = host.map(|h| format!(" --host {h}")).unwrap_or_default();
            let questions = task_questions(mode, me, &host);
            format!(
                "\nWhen done, run once: vega post done --to {me}{host} --outcome succeeded|failed \"<one-line result>\"\nThat is your final report; do not also reply. {questions}\nFollow-ups: a bead only for work this task cannot do, always --deps discovered-from:<your bead>; without beads, name it in your report.\nCI: never poll gh in a loop; vega ci watch <pr|branch> mails you the verdict."
            )
        }
    }
}

fn task_questions(mode: Mode, me: &str, host: &str) -> String {
    match mode {
        Mode::Headed => "Questions, decisions or blocked: ask the user in your pane.".into(),
        Mode::Headless => format!("Questions, decisions or blocked: ask me, never the user in your pane: vega post send{host} {me} \"<question>\", then wait for my answer."),
    }
}

/// For vegad (docs/design/vegad.md, Supervision): a message from a child to
/// its parent is a reply, so the child is not silent; a message from the
/// parent to its child is new work, which settles the child.
fn supervise(me: &str, target: &str) {
    let to = herdr(&["agent", "get", target])
        .ok()
        .and_then(|v| v["result"]["agent"]["pane_id"].as_str().map(String::from))
        .unwrap_or_else(|| target.to_string());
    for c in children().iter().filter(|c| c.machine.is_none()) {
        if c.pane == me && c.parent.pane == to {
            set_replied(None, me);
            set_silent(None, me, false);
        } else if c.pane == to && c.parent.pane == me {
            settle(None, &to);
        }
    }
}

/// Codex's status line while its session starts (`· Starting ·`); herdr
/// already reports it idle.
fn codex_starting(screen: &str) -> bool {
    let lines: Vec<&str> = screen.lines().filter(|l| !l.trim().is_empty()).collect();
    lines[lines.len().saturating_sub(CHROME_TAIL)..]
        .iter()
        .any(|l| l.contains(" · Starting · "))
}

/// Waits up to 30 s until a fresh agent has started its session. Codex takes
/// a paste while it starts but drops the Enter, and its switch from Starting
/// to Ready looks like a turn to herdr (vega-lhc.146.10). Only a screen
/// without Starting lets the brief go: a read error or the deadline fails.
fn wait_started(pane: &str) -> Result<(), Failure> {
    started_within(
        120,
        || herdr_text(&["pane", "read", pane, "--source", "visible"]),
        || std::thread::sleep(std::time::Duration::from_millis(250)),
    )
    .map_err(|f| {
        Failure::new(
            &f.code,
            format!("{pane} did not finish starting: {}", f.message),
            &format!("run `vega mux{} show {pane}`", mflag()),
        )
    })
}

fn started_within(
    tries: usize,
    mut read: impl FnMut() -> Result<String, Failure>,
    mut sleep: impl FnMut(),
) -> Result<(), Failure> {
    for _ in 0..tries {
        if !codex_starting(&read()?) {
            return Ok(());
        }
        sleep();
    }
    Err(Failure::new(
        "not_started",
        "its status line still says Starting",
        "",
    ))
}

/// The sent text still sits in the agent's input box: its last line is on
/// screen after the last input prompt (`›`, `❯`) and no prompt follows it.
/// A submitted text sits in the history, with the empty input prompt below.
/// Without an input prompt on screen (pi) it cannot tell: false.
fn staged(screen: &str, body: &str) -> bool {
    let Some(last) = body.lines().rev().map(str::trim).find(|l| !l.is_empty()) else {
        return false;
    };
    // The start of the line only: the input box wraps long lines.
    let snippet: String = last.chars().take(30).collect();
    let lines: Vec<&str> = screen.lines().collect();
    let Some(at) = lines.iter().rposition(|l| l.contains(&snippet)) else {
        return false;
    };
    let prompt = |l: &&str| {
        let t = l.trim_start();
        t.starts_with('›') || t.starts_with('❯')
    };
    lines[..=at].iter().any(prompt) && !lines[at + 1..].iter().any(prompt)
}

/// Claude Code can keep a multi-line bracketed paste staged in its input box
/// without submitting it, and codex flickers to working while it starts. If
/// the target does not start working, or its input box still holds the text,
/// press Enter only; never retype the text.
fn confirm_taken(target: &str, body: &str) -> Result<(), Failure> {
    const ATTEMPTS: usize = 3;
    for _ in 0..ATTEMPTS {
        match herdr(&[
            "agent",
            "wait",
            target,
            "--until",
            "working",
            "--timeout",
            "3000",
        ]) {
            Ok(_) => {
                let screen = herdr_text(&["pane", "read", target, "--source", "visible"])
                    .unwrap_or_default();
                if !staged(&screen, body) {
                    return Ok(());
                }
                herdr(&["agent", "send-keys", target, "enter"])?;
            }
            Err(f) if f.code == "timeout" => {
                herdr(&["agent", "send-keys", target, "enter"])?;
            }
            Err(f) => return Err(f),
        }
    }
    Err(Failure::new(
        "not_taken",
        format!("{target} did not start working after {ATTEMPTS} Enter retries"),
        "run `vega mux show <target>` to inspect its input box",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spawn_env_parses_and_builds_herdr_flags() {
        assert_eq!(
            parse_env("VEGA_GATE_ROLE=reviewer").unwrap(),
            "VEGA_GATE_ROLE=reviewer"
        );
        assert_eq!(parse_env("_X=a=b c").unwrap(), "_X=a=b c");
        assert_eq!(parse_env("EMPTY=").unwrap(), "EMPTY=");
        for bad in ["NOEQ", "=v", "1A=v", "A-B=v", "A B=v"] {
            assert!(parse_env(bad).is_err(), "{bad}");
        }
        let user = ["VEGA_GATE_ROLE=reviewer".to_string(), "FOO=bar".to_string()];
        let env = spawn_env("pi", Some("proteus/vega"), None, &user);
        assert_eq!(
            env_args(&env),
            [
                "--env",
                "BEADS_ACTOR=proteus/vega",
                "--env",
                "VEGA_GATE_ROLE=reviewer",
                "--env",
                "FOO=bar"
            ]
        );
        // An --env BEADS_ACTOR replaces the lane's.
        let own = ["BEADS_ACTOR=me".to_string()];
        assert_eq!(
            spawn_env("pi", Some("proteus/vega"), None, &own),
            ["BEADS_ACTOR=me"]
        );
        assert!(spawn_env("pi", None, None, &[]).is_empty());
    }

    #[test]
    fn codex_defaults_to_basic_ansi_unless_explicitly_overridden() {
        let env = spawn_env("codex", Some("proteus/vega"), None, &["FOO=bar".into()]);
        assert_eq!(
            env_args(&env),
            [
                "--env",
                "BEADS_ACTOR=proteus/vega",
                "--env",
                "FORCE_COLOR=1",
                "--env",
                "FOO=bar"
            ]
        );
        assert_eq!(spawn_env("codex", None, None, &[]), ["FORCE_COLOR=1"]);
        for value in ["0", "2", "3", ""] {
            let user = [format!("FORCE_COLOR={value}")];
            assert_eq!(spawn_env("codex", None, None, &user), user);
        }
        // Similar names do not suppress the default, and other kinds get no default.
        let user = ["FORCE_COLOR_EXTRA=3".into()];
        assert_eq!(
            spawn_env("codex", None, None, &user),
            ["FORCE_COLOR=1", "FORCE_COLOR_EXTRA=3"]
        );
        for kind in ["claude", "pi", "agy"] {
            assert!(spawn_env(kind, None, None, &[]).is_empty());
            let user = ["FORCE_COLOR=3".into()];
            assert_eq!(spawn_env(kind, None, None, &user), user);
        }
    }

    fn sock_args(
        kind: &str,
        socket: Option<&str>,
        user: &[String],
        extra: &[String],
    ) -> Vec<String> {
        spawn_args(kind, &[("SSH_AUTH_SOCK", socket)], user, extra)
    }

    #[test]
    fn codex_spawn_args_keep_herdr_identity_in_tool_shells() {
        let keep = [
            ("SSH_AUTH_SOCK", None),
            ("HERDR_PANE_ID", Some("wDR:p2")),
            ("HERDR_SOCKET_PATH", Some("/h/herdr.sock")),
        ];
        let n = default_args("codex").len();
        let args = spawn_args("codex", &keep, &[], &[]);
        assert_eq!(
            args[n..],
            [
                "-c",
                "shell_environment_policy.set.HERDR_PANE_ID=\"wDR:p2\"",
                "-c",
                "shell_environment_policy.set.HERDR_SOCKET_PATH=\"/h/herdr.sock\"",
            ]
        );
        // Explicit --env beats the spawned pane; a user -c comes last and wins in Codex.
        let user = ["HERDR_PANE_ID=w1:p1".into()];
        let extra = [
            "-c".into(),
            "shell_environment_policy.set.HERDR_PANE_ID=\"x\"".into(),
        ];
        let args = spawn_args("codex", &keep, &user, &extra);
        assert!(args.contains(&"shell_environment_policy.set.HERDR_PANE_ID=\"w1:p1\"".into()));
        assert_eq!(args[args.len() - 2..], extra);
        assert_eq!(
            spawn_args("claude", &keep, &[], &[]),
            default_args("claude")
        );
    }

    #[test]
    fn codex_spawn_args_keep_socket_in_tool_shells() {
        let defaults: Vec<String> = default_args("codex").iter().map(|s| (*s).into()).collect();
        assert_eq!(sock_args("codex", None, &[], &[]), defaults);
        for (socket, quoted) in [
            ("/tmp/caller agent.sock", r#""/tmp/caller agent.sock""#),
            ("", r#""""#),
            ("true", r#""true""#),
            ("a\"b\\c\nd", r#""a\"b\\c\nd""#),
            ("\u{7f}", r#""\u007f""#),
        ] {
            let args = sock_args("codex", Some(socket), &[], &[]);
            assert_eq!(&args[..defaults.len()], defaults);
            assert_eq!(args[defaults.len()], "-c");
            assert_eq!(
                args[defaults.len() + 1],
                format!("shell_environment_policy.set.SSH_AUTH_SOCK={quoted}")
            );
            assert_eq!(args.len(), defaults.len() + 2);
        }
        for value in ["/tmp/remote.sock", ""] {
            let user = [format!("SSH_AUTH_SOCK={value}")];
            for caller in [None, Some("/tmp/caller.sock")] {
                let args = sock_args("codex", caller, &user, &[]);
                assert_eq!(
                    args.last().unwrap(),
                    &format!(
                        "shell_environment_policy.set.SSH_AUTH_SOCK={}",
                        serde_json::to_string(value).unwrap()
                    )
                );
            }
        }
        // Duplicate --env follows herdr's last-value precedence.
        let user = ["SSH_AUTH_SOCK=/first".into(), "SSH_AUTH_SOCK=/last".into()];
        assert!(sock_args("codex", None, &user, &[])
            .last()
            .unwrap()
            .ends_with("=\"/last\""));
        let extra = [
            "-c".into(),
            "shell_environment_policy.set.SSH_AUTH_SOCK=\"/explicit\"".into(),
            "--config=shell_environment_policy.inherit=\"none\"".into(),
        ];
        let args = sock_args("codex", Some("/caller"), &user, &extra);
        assert_eq!(&args[args.len() - extra.len()..], extra);
        let similar = ["SSH_AUTH_SOCK_EXTRA=/ignored".into()];
        assert_eq!(sock_args("codex", None, &similar, &[]), defaults);
        for kind in ["claude", "pi", "agy"] {
            let expected: Vec<String> = default_args(kind)
                .iter()
                .map(|s| (*s).into())
                .chain(extra.iter().cloned())
                .collect();
            assert_eq!(sock_args(kind, Some("/caller"), &user, &extra), expected);
        }
    }

    #[test]
    fn spawn_env_forwards_caller_socket_unless_explicitly_overridden() {
        for kind in ["codex", "claude", "pi", "agy"] {
            for user in [vec![], vec!["FOO=bar".into()]] {
                let env = spawn_env(kind, None, Some("/tmp/caller agent.sock"), &user);
                assert!(env.contains(&"SSH_AUTH_SOCK=/tmp/caller agent.sock".into()));
                assert!(user.iter().all(|entry| env.contains(entry)));
                let flags = env_args(&env);
                assert!(flags
                    .windows(2)
                    .any(|pair| { pair == ["--env", "SSH_AUTH_SOCK=/tmp/caller agent.sock"] }));
            }
            for value in ["/tmp/user.sock", ""] {
                let user = [format!("SSH_AUTH_SOCK={value}")];
                for socket in [None, Some("/tmp/caller.sock")] {
                    let env = spawn_env(kind, None, socket, &user);
                    let sockets: Vec<_> = env
                        .iter()
                        .filter(|entry| entry.starts_with("SSH_AUTH_SOCK="))
                        .collect();
                    assert_eq!(sockets, [&user[0]]);
                }
            }
            let user = ["SSH_AUTH_SOCK_EXTRA=ignored".into()];
            let env = spawn_env(kind, None, Some("/tmp/caller.sock"), &user);
            assert!(env.contains(&"SSH_AUTH_SOCK=/tmp/caller.sock".into()));
            let env = spawn_env(kind, None, None, &[]);
            assert!(!env.iter().any(|entry| entry.starts_with("SSH_AUTH_SOCK=")));
            let env = spawn_env(kind, None, Some(""), &[]);
            assert!(env.contains(&"SSH_AUTH_SOCK=".into()));
        }
    }

    #[test]
    fn spawn_takes_repeatable_env_for_every_kind() {
        for kind in ["claude", "codex", "pi"] {
            let cli = Cli::try_parse_from([
                "vega-mux",
                "spawn",
                "rev",
                "--kind",
                kind,
                "--env",
                "VEGA_GATE_ROLE=reviewer",
                "--env",
                "FOO=bar",
            ])
            .unwrap();
            let Some(Cmd::Spawn { env, .. }) = cli.cmd else {
                panic!("not spawn")
            };
            assert_eq!(env, ["VEGA_GATE_ROLE=reviewer", "FOO=bar"]);
        }
        // codex inherits the pane env only without its shared daemon.
        assert!(default_args("codex").contains(&"--no-daemon"));
        assert!(
            Cli::try_parse_from(["vega-mux", "spawn", "rev", "--kind", "pi", "--env", "1=x"])
                .is_err()
        );
        assert!(
            Cli::try_parse_from(["vega-mux", "spawn", "--resume", "w1:p2", "--env", "A=b"])
                .is_err()
        );
    }

    #[test]
    fn skill_has_valid_frontmatter() {
        vega_plugin::assert_guide(SKILL, "mux");
    }
    use serde_json::json;

    #[test]
    fn close_takes_the_worktree_workspace_only_when_the_agent_is_alone() {
        let wt = json!({"worktree": {"is_linked_worktree": true}});
        let main = json!({"worktree": {"is_linked_worktree": false}});
        assert_eq!(closing(&wt, 0), Closing::Workspace);
        assert_eq!(closing(&wt, 1), Closing::Pane);
        assert_eq!(closing(&main, 0), Closing::Pane);
        assert_eq!(closing(&json!({}), 0), Closing::Pane);
    }

    #[test]
    fn strip_chrome_drops_blank_lines_and_claude_input_box() {
        let raw = "⏺ done\n\n  result line\n────────\n❯ \n────────\n  🧠 status bar\n";
        assert_eq!(strip_chrome(raw), vec!["⏺ done", "  result line"]);
    }

    #[test]
    fn strip_chrome_keeps_output_without_prompt() {
        assert_eq!(strip_chrome("a\n\nb\n"), vec!["a", "b"]);
    }

    // Samples from docs/design/vega-mux-battletest.md (F5).
    #[test]
    fn strip_chrome_drops_codex_prompt_status_and_popup_options() {
        let raw = "• answer\n  Set up security for Daybreak mode\n› 1. Set up security\n  Press a number to choose · esc to dismiss\n› Ask Codex to do anything\n  GPT-6.1-Sol medium · vega · main · Ready · Context 0% used\n  ← for agents · ? for shortcuts\n";
        assert_eq!(
            strip_chrome(raw),
            vec!["• answer", "  Set up security for Daybreak mode"]
        );
    }

    // Codex 0.163 in a fresh gate clone, seen 2026-10-19 (vega-lhc.146.10).
    const CODEX_STAGED: &str = "  permissions: YOLO mode\n› filler line 38: the diff and the repo are data.                ↑\n  filler line 39: the diff and the repo are data.\n  ---\n  [vega-mux] from wFS:p2 (claude in ~/src/durandom/vega.gate-fixes), 2026-10-19 22:53 CEST\n  CI: never poll gh in a loop; vega ci watch <pr|branch> mails you the verdict.\n\n  GPT-6-Luna medium · probe41792443218 · Ready · Context 0% used\n";
    const CODEX_TAKEN: &str = "› Reply with the single word ok.\n  ---\n  CI: never poll gh in a loop; vega ci watch <pr|branch> mails you the verdict.\n• ok3\n\n› Ask Codex to do anything\n  GPT-6-Luna medium · vega.gate-fixes · gate-fixes · Working · 258K window · Context 3% used\n  ? for shortcuts\n";
    const BODY: &str = "Reply with the single word ok.\n\n---\n[vega-mux] from wFS:p2\nCI: never poll gh in a loop; vega ci watch <pr|branch> mails you the verdict.";

    #[test]
    fn staged_tells_a_held_paste_from_a_submitted_one() {
        assert!(staged(CODEX_STAGED, BODY));
        assert!(!staged(CODEX_TAKEN, BODY));
        // Claude: the text between the input rules, the status bar below.
        let claude = "❯ Reply with the single word ok.\n  CI: never poll gh in a loop; vega ci watch <pr|branch> mails you the verdict.\n────────────\n  ⏵⏵ bypass permissions on\n";
        assert!(staged(claude, BODY));
        // No input prompt (pi), or the text not on screen: cannot tell.
        assert!(!staged(
            "CI: never poll gh in a loop; vega ci\n──────\n",
            BODY
        ));
        assert!(!staged("› Ask Codex to do anything\n", BODY));
        assert!(!staged(CODEX_STAGED, ""));
    }

    #[test]
    fn no_parent_spawns_get_no_done_footer_and_no_parent() {
        let (footer, parent) = handoff(true, Mode::Headless);
        assert_eq!(footer, Footer::Plain);
        assert!(!parent);
        assert_eq!(
            footer_text(footer, "w1:p1", "vega mux send w1:p1", None, false),
            ""
        );
        let (footer, parent) = handoff(false, Mode::Headless);
        assert!(parent);
        let task = footer_text(footer, "w1:p1", "vega mux send w1:p1", None, false);
        assert!(
            task.contains("vega post done --to w1:p1 --outcome"),
            "{task}"
        );
        let remote = footer_text(footer, "w1:p1", "x", Some("proteus"), false);
        assert!(
            remote.contains("vega post done --to w1:p1 --host proteus"),
            "{remote}"
        );
        // A kept brief carries the choice to `spawn --resume`.
        let kept = KeptBrief {
            name: "gate-x".into(),
            brief: "review".into(),
            no_parent: true,
            ..Default::default()
        };
        assert_eq!(
            handoff(kept.no_parent, Mode::Headed),
            (Footer::Plain, false)
        );
    }

    #[test]
    fn only_a_screen_past_starting_lets_the_brief_go() {
        let starting = "  GPT-6-Luna medium · x · Starting · Context 0% used\n".to_string();
        let mut screens = vec![CODEX_STAGED.to_string(), starting.clone()];
        assert!(started_within(3, || Ok(screens.pop().unwrap()), || {}).is_ok());
        let f = started_within(3, || Ok(starting.clone()), || {}).unwrap_err();
        assert_eq!(f.code, "not_started");
        let f = started_within(3, || Err(Failure::new("herdr", "down", "")), || {}).unwrap_err();
        assert_eq!(f.code, "herdr");
    }

    #[test]
    fn codex_starting_reads_the_status_line() {
        assert!(codex_starting(
            "› Reply\n  GPT-6-Luna medium · probe61792443279 · Starting · Context 0% used\n"
        ));
        assert!(!codex_starting(CODEX_STAGED));
        assert!(!codex_starting(CODEX_TAKEN));
    }

    #[test]
    fn strip_chrome_drops_pi_input_box_and_status() {
        let raw = "pong\n──────────────────\n\n──────────────────\n~/src/durandom/vega (main)\n$0.000 (sub) 0.0%/500k (auto)   (xai) grok-4.7 • medium\n◆ 1 checkpoint\n";
        assert_eq!(strip_chrome(raw), vec!["pong"]);
    }

    #[test]
    fn strip_chrome_only_looks_at_the_tail() {
        let mut raw = String::from("──────────\nkept\n");
        for i in 0..CHROME_TAIL {
            raw.push_str(&format!("line {i}\n"));
        }
        assert_eq!(strip_chrome(&raw).len(), CHROME_TAIL + 2);
    }

    #[test]
    fn expand_home_only_touches_a_leading_tilde() {
        assert_eq!(expand_home("~/src/x", "/home/m"), "/home/m/src/x");
        assert_eq!(expand_home("~", "/home/m"), "/home/m");
        assert_eq!(expand_home("/abs/~x", "/home/m"), "/abs/~x");
        assert_eq!(expand_home("~other/x", "/home/m"), "~other/x");
    }

    #[test]
    fn asks_folder_trust_matches_claude_dialog() {
        let screen = " Quick safety check: Is this a project you created or one you trust?\n ❯ No, exit\n   Yes, I trust this folder\n";
        assert!(asks_folder_trust(screen));
        assert!(!asks_folder_trust("❯ ready"));
    }

    #[test]
    fn asks_hooks_review_matches_codex_dialog() {
        let screen = "  Hooks need review\n  7 hooks are new or changed.\n› 1. Review hooks\n";
        assert!(asks_hooks_review(screen));
        assert!(!asks_folder_trust(screen));
        assert!(!asks_hooks_review("› Ask Codex to do anything"));
    }

    #[test]
    fn asks_folder_trust_matches_codex_dialog() {
        assert!(asks_folder_trust(
            "  Folder access\n  Trust this folder? Codex can read"
        ));
    }

    #[test]
    fn clean_title_strips_terminal_title_noise() {
        let cwd = "/Users/x/src/durandom/vega";
        assert_eq!(clean_title("vega", cwd), "-");
        assert_eq!(clean_title("π - vega", cwd), "-");
        assert_eq!(clean_title("[ ! ] Action Required | vega", cwd), "-");
        assert_eq!(clean_title("Fix auth bug", cwd), "Fix auth bug");
        assert_eq!(clean_title("", cwd), "-");
        assert_eq!(clean_title("vega ⠹", cwd), "-");
    }

    #[test]
    fn validate_rejects_empty_target() {
        let send = |t: &str| Cmd::Send {
            target: t.into(),
            text: Some("hi".into()),
            file: None,
            now: false,
        };
        assert_eq!(validate(&send("")).unwrap().message, "target is empty");
        assert!(validate(&send("w3V:p1")).is_none());
    }

    #[test]
    fn read_text_takes_arg_or_file_and_rejects_empty() {
        assert_eq!(
            read_text(Some("hi"), None, "message").unwrap().as_deref(),
            Some("hi")
        );
        assert_eq!(read_text(None, None, "prompt").unwrap(), None);
        assert_eq!(
            read_text(Some(" \n"), None, "message").unwrap_err().message,
            "message text is empty"
        );
        let f = std::env::temp_dir().join(format!("vega-mux-test-{}", std::process::id()));
        std::fs::write(&f, "it's \"quoted\" $x\n").unwrap();
        let got = read_text(None, Some(&f), "prompt").unwrap();
        std::fs::remove_file(&f).unwrap();
        assert_eq!(got.as_deref(), Some("it's \"quoted\" $x\n"));
        let missing = PathBuf::from("/nonexistent/vega-mux");
        assert_eq!(
            read_text(None, Some(&missing), "prompt").unwrap_err().code,
            "usage"
        );
    }

    #[test]
    fn startup_failure_shows_the_agents_last_output() {
        // A fake pi that prints an error and exits, as pi does for an
        // ambiguous --model; the pane shows its shell prompt afterwards.
        let dir = std::env::temp_dir().join(format!("vega-mux-fake-pi-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let fake = dir.join("pi");
        std::fs::write(
            &fake,
            "#!/bin/sh\necho 'Starting pi'\necho 'Error: model \"grok-4.7\" is ambiguous: xai, openrouter. Pass --provider or provider/model.' >&2\nexit 1\n",
        )
        .unwrap();
        let out = std::process::Command::new("sh")
            .arg("-c")
            .arg(format!(
                "sh {} --approve --model grok-4.7 2>&1",
                fake.display()
            ))
            .output()
            .unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(!out.status.success());
        let screen = format!(
            "$ pi --approve --model grok-4.7\n{}\n\n$ ",
            String::from_utf8_lossy(&out.stdout)
        );
        let lines = last_output(&screen);
        assert_eq!(lines.last().unwrap(), "$");
        let tail = output_tail(&lines);
        assert!(tail.starts_with("; last output: $ pi --approve"), "{tail}");
        assert!(tail.contains("is ambiguous: xai, openrouter"), "{tail}");
        assert!(startup_hint(&lines).contains("--provider X"));

        // A missing binary keeps the --kind hint; an empty screen too.
        let missing = last_output("$ pi --approve\nzsh: command not found: pi\n$ ");
        assert!(startup_hint(&missing).starts_with("check --kind"));
        assert_eq!(output_tail(&last_output("\n  \n")), "");
        assert!(startup_hint(&[]).starts_with("check --kind"));
    }

    #[test]
    fn last_output_keeps_the_tail_and_masks_secrets() {
        let screen: String = (1..=10).map(|i| format!("line {i}\n")).collect();
        assert_eq!(last_output(&screen).first().unwrap(), "line 5");
        assert_eq!(last_output(&"x".repeat(500))[0].len(), 200);
        assert_eq!(
            redact("XAI_API_KEY=xai-abc123 --api-key=s3cret token: short"),
            "XAI_API_KEY=[redacted] --api-key=[redacted] token: [redacted]"
        );
        assert_eq!(
            redact("Authorization: Bearer abc.def"),
            "Authorization: [redacted] [redacted]"
        );
        assert_eq!(
            redact("401 for sk-proj-Ab12Cd34Ef56Gh78Ij90 at /Users/me/src"),
            "401 for [redacted] at /Users/me/src"
        );
        // Model names and paths stay readable.
        assert_eq!(
            redact("model xai/grok-4.7 in ~/src/durandom/vega.pi-provider"),
            "model xai/grok-4.7 in ~/src/durandom/vega.pi-provider"
        );
    }

    #[test]
    fn task_footer_routes_questions_by_mode() {
        for quarantined in [false, true] {
            for (mode, question) in [
                (Mode::Headed, "ask the user in your pane."),
                (
                    Mode::Headless,
                    "ask me, never the user in your pane: vega post send --host proteus w1:p1",
                ),
            ] {
                let (footer, _) = handoff(false, mode);
                let text = footer_text(footer, "w1:p1", "x", Some("proteus"), quarantined);
                if quarantined {
                    assert!(text.contains("ask the user in your pane."), "{text}");
                } else {
                    assert!(text.contains(question), "{text}");
                }
                assert_eq!(
                    text.contains("never the user"),
                    mode == Mode::Headless && !quarantined
                );
            }
        }
    }

    #[test]
    fn spawn_records_headed_by_default_and_headless_on_request() {
        // Isolate state in a subprocess so parallel tests keep their own env.
        if env::var_os("VEGA_MUX_MODE_TEST").is_none() {
            let dir = env::temp_dir().join(format!("vega-mux-mode-{}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            let output = Command::new(env::current_exe().unwrap())
                .args([
                    "--exact",
                    "tests::spawn_records_headed_by_default_and_headless_on_request",
                    "--nocapture",
                ])
                .env("VEGA_MUX_MODE_TEST", "1")
                .env("XDG_STATE_HOME", &dir)
                .output()
                .unwrap();
            std::fs::remove_dir_all(dir).unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stdout)
            );
            return;
        }
        let pane = format!("mux-mode-test-{}", std::process::id());
        let test_dir = PathBuf::from(env::var_os("XDG_STATE_HOME").unwrap());
        let bin = test_dir.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let herdr = bin.join("herdr");
        std::fs::write(&herdr, r#"#!/bin/sh
case "$1 $2" in
  "agent get") printf '{"result":{"agent":{"pane_id":"%s","name":"fix","agent":"codex","agent_status":"idle","cwd":"/checkout"}}}\n' "$3" ;;
  "pane list") printf '%s\n' '{"result":{"panes":[{"pane_id":"mode-parent","agent":"codex","workspace_id":"test","cwd":"/checkout"}]}}' ;;
  "pane process-info") printf '{"result":{"process_info":{"shell_pid":%s}}}\n' "$VEGA_MUX_TEST_PID" ;;
  "pane read") printf 'Ready\n›\n' ;;
  "agent prompt") printf '%s' "$4" > "$VEGA_MUX_TEST_PROMPT"; printf '%s\n' '{"result":{}}' ;;
  "agent wait") printf '%s\n' '{"result":{}}' ;;
  *) printf 'unexpected fake herdr call: %s\n' "$*" >&2; exit 1 ;;
esac
"#).unwrap();
        // Identity preflight inspects the owner-local cwd, so use an existing
        // non-repository directory rather than the old /checkout placeholder.
        let script = std::fs::read_to_string(&herdr)
            .unwrap()
            .replace("/checkout", test_dir.to_str().unwrap());
        std::fs::write(&herdr, script).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&herdr, std::fs::Permissions::from_mode(0o755)).unwrap();
        let mut paths = vec![bin];
        paths.extend(env::split_paths(&env::var_os("PATH").unwrap()));
        env::set_var("PATH", env::join_paths(paths).unwrap());
        env::set_var("HERDR_PANE_ID", "mode-parent");
        env::set_var("VEGA_MUX_TEST_PID", std::process::id().to_string());
        let prompt_file = test_dir.join("prompt");
        env::set_var("VEGA_MUX_TEST_PROMPT", &prompt_file);
        for (flags, expected) in [(vec![], Mode::Headed), (vec!["--headless"], Mode::Headless)] {
            let mut args = vec!["vega-mux", "spawn", "fix", "--kind", "codex"];
            args.extend(flags);
            let cli = Cli::try_parse_from(args).unwrap();
            let Some(Cmd::Spawn { headless, .. }) = cli.cmd else {
                panic!("not spawn")
            };
            // Exercise the actual startup boundary. A startup dialog must see
            // the record already, and failing startup must not discard it.
            let result = start_spawn(&pane, "fix", spawn_mode(headless), || {
                assert_eq!(mode_of(None, &pane, "fix"), expected);
                Err(Failure::new("trust_required", "dialog", "answer in pane"))
            });
            assert_eq!(result.unwrap_err().code, "trust_required");
            assert_eq!(mode_of(None, &pane, "fix"), expected);
            let unnamed =
                json!({"pane_id": pane, "name": "", "agent": "codex", "cwd": "/checkout"});
            // No brief (including spawn without --prompt): never write "-".
            for request in [None, Some(ModeArg::Headless), Some(ModeArg::Headed)] {
                assert_eq!(
                    agent_mode(&unnamed, request).unwrap_err().code,
                    "agent_not_ready"
                );
                assert_eq!(mode_of(None, &pane, "fix"), expected);
            }
            let record = KeptBrief {
                name: "fix".into(),
                kind: "codex".into(),
                cwd: "/checkout".into(),
                brief: "task".into(),
                ..Default::default()
            };
            assert!(keep_brief(&pane, &record));
            let mut agent =
                json!({"pane_id": pane, "name": "", "agent": "codex", "cwd": "/checkout"});
            // Both show and mode resolve the same startup identity; a mode
            // update must not replace the saved name with herdr's placeholder.
            let name = mode_agent_name(&agent);
            assert_eq!(name, "fix");
            assert_eq!(mode_of(None, &pane, &name), expected);
            let requested = if expected == Mode::Headed {
                ModeArg::Headed
            } else {
                ModeArg::Headless
            };
            assert_eq!(
                agent_mode(&agent, Some(requested)).unwrap(),
                ("fix".into(), expected)
            );
            // The brief delivery shared by spawn and resume uses this record.
            let resumed = kept_brief(&pane).unwrap();
            assert!(same_agent(&resumed, &agent));
            let (footer, _) = handoff(false, mode_of(None, &pane, &resumed.name));
            assert_eq!(footer, Footer::Task(expected));
            resume_spawn(&pane).unwrap();
            assert_eq!(parent_of(&pane, "fix").unwrap().pane, "mode-parent");
            assert_eq!(mode_of(None, &pane, "fix"), expected);
            assert!(kept_brief(&pane).is_none());
            let prompt = std::fs::read_to_string(&prompt_file).unwrap();
            assert_eq!(
                prompt.contains("never the user"),
                expected == Mode::Headless
            );
            // Keep a fresh brief to exercise rejection of mismatched identity.
            assert!(keep_brief(&pane, &record));
            agent["name"] = json!("replacement");
            assert_eq!(mode_agent_name(&agent), "replacement");
            agent["name"] = json!("");
            agent["agent"] = json!("claude");
            assert_eq!(mode_agent_name(&agent), "-");
            assert!(agent_mode(&agent, Some(ModeArg::Headless)).is_err());
            assert_eq!(mode_of(None, &pane, "fix"), expected);
            drop_kept_brief(&pane);
            std::fs::remove_file(state_file("parents", &pane).unwrap()).unwrap();
            assert!(has_records(&pane)); // includes mode-only --no-parent panes
            assert!(!close_failed_spawn(&pane, || Err(Failure::new(
                "io",
                "close failed",
                "retry"
            ))));
            assert_eq!(mode_of(None, &pane, "fix"), expected);
            assert!(close_failed_spawn(&pane, || Ok(Value::Null)));
            assert!(!has_records(&pane));
        }
        assert!(!state_file("modes", &pane).unwrap().exists());
        assert_eq!(mode_of(None, &pane, "fix"), Mode::Headless);
    }

    #[test]
    fn spawn_flags_parse_and_conflict() {
        let parse = |args: &[&str]| Cli::try_parse_from([&["vega mux"], args].concat());
        assert!(parse(&[
            "spawn",
            "a",
            "--kind",
            "pi",
            "--worktree",
            "b",
            "--base",
            "origin/main"
        ])
        .is_ok());
        assert!(parse(&[
            "spawn",
            "a",
            "--kind",
            "pi",
            "--worktree",
            "b",
            "--at",
            "tab"
        ])
        .is_err());
        assert!(parse(&["spawn", "a", "--kind", "pi", "--base", "main"]).is_err());
        assert!(parse(&[
            "spawn",
            "a",
            "--kind",
            "pi",
            "--prompt",
            "x",
            "--prompt-file",
            "f"
        ])
        .is_err());
        assert!(parse(&["spawn", "--resume", "w1:p2"]).is_ok());
        assert!(parse(&["spawn", "a", "--resume", "w1:p2"]).is_err());
        assert!(parse(&["spawn", "--resume", "w1:p2", "--prompt", "x"]).is_err());
        assert!(parse(&["spawn", "a", "--kind", "codex", "--no-parent"]).is_ok());
        assert!(parse(&["spawn", "--resume", "w1:p2", "--no-parent"]).is_err());
        assert!(parse(&["spawn", "--resume", "w1:p2", "--headless"]).is_err());
        assert!(parse(&["spawn", "a"]).is_err());
        assert!(parse(&["send", "a", "--file", "f"]).is_ok());
        assert!(parse(&["send", "a", "x", "--file", "f"]).is_err());
        assert!(parse(&["send", "a"]).is_err());
    }

    #[test]
    fn agent_table_has_kind_and_name_with_dash_for_unnamed() {
        let a = [
            json!({"pane_id": "w1:p2", "agent": "codex", "name": "rev", "agent_status": "working",
                   "cwd": "/srv/x", "terminal_title_stripped": "Review PR"}),
            json!({"pane_id": "w1:p3", "agent": "claude", "agent_status": "idle",
                   "cwd": "/srv/y", "terminal_title_stripped": "y"}),
        ];
        let refs: Vec<&Value> = a.iter().collect();
        assert_eq!(
            agent_table(&refs),
            vec![
                "agents[2]{pane,kind,name,status,cwd,title}:",
                "  w1:p2,codex,rev,working,/srv/x,Review PR",
                "  w1:p3,claude,-,idle,/srv/y,-",
            ]
        );
    }

    #[test]
    fn agent_table_drops_name_column_when_no_agent_has_one() {
        let a = [
            json!({"pane_id": "w1:p3", "agent": "pi", "name": "", "agent_status": "idle",
                        "cwd": "/srv/y", "terminal_title_stripped": "Fix bug"}),
        ];
        let refs: Vec<&Value> = a.iter().collect();
        assert_eq!(
            agent_table(&refs),
            vec![
                "agents[1]{pane,kind,status,cwd,title}:",
                "  w1:p3,pi,idle,/srv/y,Fix bug",
            ]
        );
    }

    #[test]
    fn title_falls_back_to_the_saved_task() {
        let codex = json!({"name": "rev", "cwd": "/srv/api", "terminal_title_stripped": "api ⠹"});
        assert_eq!(
            title_of(&codex, Some("Review the auth PR")),
            "Review the auth PR"
        );
        assert_eq!(title_of(&codex, Some("3")), "-");
        assert_eq!(title_of(&codex, Some("rev")), "-");
        assert_eq!(title_of(&codex, None), "-");
        // A real terminal title (Claude's summary) wins over the saved task.
        let claude = json!({"cwd": "/srv/api", "terminal_title_stripped": "Fix login bug"});
        assert_eq!(title_of(&claude, Some("Old task")), "Fix login bug");
    }

    #[test]
    fn task_label_is_the_first_non_empty_line() {
        assert_eq!(
            task_label("\n  Run the tests.\nThen report.").as_deref(),
            Some("Run the tests.")
        );
        assert_eq!(task_label(" \n"), None);
    }

    #[test]
    fn command_of_takes_the_group_leader_and_strips_login_dash() {
        let info = json!({"foreground_process_group_id": 20, "foreground_processes": [
            {"pid": 21, "cmdline": "caffeinate -i", "argv0": "caffeinate"},
            {"pid": 20, "cmdline": "ssh ganymede", "argv0": "ssh"},
        ]});
        assert_eq!(command_of(&info), "ssh ganymede");
        let shell = json!({"foreground_process_group_id": 5,
            "foreground_processes": [{"pid": 5, "cmdline": "-zsh"}]});
        assert_eq!(command_of(&shell), "zsh");
        assert_eq!(command_of(&Value::Null), "-");
    }

    #[test]
    fn pane_and_workspace_tables() {
        let rows = vec![(
            json!({"pane_id": "w3V:p1E", "cwd": "/srv/a,b"}),
            "btop".to_string(),
        )];
        assert_eq!(
            pane_table(&rows),
            vec!["panes[1]{pane,command,cwd}:", "  w3V:p1E,btop,\"/srv/a,b\""]
        );
        assert_eq!(pane_table(&[]), vec!["panes: 0 panes without an agent"]);
        let ws = [json!({"workspace_id": "w3V", "label": "vega"})];
        assert_eq!(
            workspace_table(&ws),
            vec!["workspaces[1]{id,label}:", "  w3V,vega"]
        );
    }

    #[test]
    fn shorten_adds_ellipsis_only_when_needed() {
        assert_eq!(shorten("abc", 5), "abc");
        assert_eq!(shorten("abcdefg", 5), "abcd…");
    }

    #[test]
    fn agent_names_follow_herdr_rule() {
        assert!(valid_agent_name("rev"));
        assert!(valid_agent_name("link-parsing"));
        assert!(!valid_agent_name("api_2"));
        assert!(!valid_agent_name("own-bead-sk-jay0-6-92-and-more"));
        assert!(!valid_agent_name("Rev"));
        assert!(!valid_agent_name("2api"));
        assert!(!valid_agent_name(""));
        assert!(!valid_agent_name(&"a".repeat(33)));
    }

    #[test]
    fn split_direction_by_pane_shape() {
        let layout = json!({"panes": [
            {"pane_id": "w1:p1", "rect": {"width": 176, "height": 78}},
            {"pane_id": "w1:p2", "rect": {"width": 88, "height": 78}},
        ]});
        assert_eq!(split_direction(&layout, "w1:p1"), "right");
        assert_eq!(split_direction(&layout, "w1:p2"), "down");
        assert_eq!(split_direction(&layout, "w1:p9"), "down");
    }

    #[test]
    fn codex_spawns_without_daemon_or_update_check() {
        assert_eq!(default_args("codex")[0], "--no-daemon");
        assert!(default_args("codex")
            .windows(2)
            .any(|w| w == ["-c", "check_for_update_on_startup=false"]));
    }

    #[test]
    fn resume_matches_a_pane_that_lost_its_agent_name() {
        let record = KeptBrief {
            name: "rbr-unit-audit".into(),
            kind: "codex".into(),
            cwd: "/srv/rally".into(),
            brief: "audit".into(),
            no_parent: false,
        };
        // herdr at codex's update screen: kind and cwd, no name.
        let nameless = json!({"agent": "codex", "agent_status": "unknown", "cwd": "/srv/rally"});
        assert!(same_agent(&record, &nameless));
        assert!(same_agent(
            &record,
            &json!({"agent": "codex", "name": "", "cwd": "/srv/rally"})
        ));
        assert!(same_agent(
            &record,
            &json!({"agent": "codex", "name": "rbr-unit-audit", "cwd": "/x"})
        ));
        // Another agent, kind or folder: the pane was reused.
        assert!(!same_agent(
            &record,
            &json!({"agent": "codex", "name": "other", "cwd": "/srv/rally"})
        ));
        assert!(!same_agent(
            &record,
            &json!({"agent": "claude", "cwd": "/srv/rally"})
        ));
        assert!(!same_agent(
            &record,
            &json!({"agent": "codex", "cwd": "/srv/other"})
        ));
        // Records from before kind and cwd were kept need the name.
        let old = KeptBrief {
            kind: String::new(),
            cwd: String::new(),
            ..record
        };
        assert!(!same_agent(&old, &nameless));
        assert!(default_args("unknown").is_empty());
    }
}
