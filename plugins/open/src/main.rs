//! `vega open`: open things from a herdr pane in the right app, VS Code
//! first.
//!
//! herdr flow: one keybinding invokes the `selection` plugin action, which
//! only collects inputs (herdr's selected text, the pane id and cwd) and
//! opens the `menu` popup. The popup builds the home menu (see `home.rs`):
//! the selection if it resolves, the workspace, the repo's git diff.
//!
//! herdr's default `copy_on_select` clears the selection on mouse release,
//! so the clipboard stands in for it, but only while its text is visible on
//! the pane; a stale clipboard never shows up.
//!
//! A relative path that exists neither under the cwd nor the git root is
//! matched as a suffix against the repo's files. Several matches get a file
//! list first, ranked by what is visible on the pane.
//!
//! A herdr pane id (`w1:p2`, `io:w1:p2`) jumps to that pane (see `pane.rs`).

mod choices;
mod home;
mod menu;
mod mounts;
mod rank;
mod request;
mod target;

use std::env;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};

use clap::{Parser, Subcommand};
use serde_json::Value;

use vega_herdr::pane;

use choices::{choices_for, Choice, Facts};
use request::Request;
use target::Target;

const PLUGIN_ID: &str = "vega.open";

/// Cap on files offered in the menu: hotkeys 1-9, ranked best first.
const MAX_CANDIDATES: usize = 9;

/// Outcome of turning selected text into files.
enum Found {
    One(Target),
    /// Several repo files end with the selected relative path.
    Many {
        root: PathBuf,
        suffix: String,
        rels: Vec<String>,
        line: Option<u32>,
        col: Option<u32>,
    },
    /// A herdr pane id; several hits when a bare id exists on several
    /// other machines.
    Pane(Vec<pane::Hit>),
}

#[derive(Parser)]
#[command(
    name = "vega open",
    version = env!("VEGA_VERSION"),
    about = "Open selections, the workspace or the git diff from a herdr pane, VS Code first",
    after_help = "Examples:\n  vega open resolve 'src/main.rs:12'          # show the target and fitting apps\n  vega open resolve README.md --cwd ~/src/x\n  vega open resolve io:w1:p2                  # a herdr pane, here or on another machine\n\nIn herdr: press the key bound to `vega.open.selection`; select a path first to open it."
)]
struct Cli {
    /// Print the agent guide for this plugin, SKILL.md form (read by `vega skills`)
    #[arg(long)]
    skill: bool,
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

const SKILL: &str = include_str!("../SKILL.md");

#[derive(Subcommand)]
enum Cmd {
    /// Resolve text to a file or herdr pane and list what can open it (no side effects)
    Resolve {
        /// Text as selected on screen, e.g. `src/main.rs:12:5`
        text: String,
        /// Directory relative paths resolve against (default: current dir)
        #[arg(long)]
        cwd: Option<PathBuf>,
    },
    /// herdr plugin action entrypoint (reads HERDR_PLUGIN_CONTEXT_JSON)
    #[command(hide = true)]
    Action,
    /// herdr popup entrypoint (reads VEGA_OPEN_CWD/PANE/SELECTED)
    #[command(hide = true)]
    Menu,
    /// vegad entrypoint: run a request from a remote herdr pane (JSON on stdin)
    #[command(hide = true)]
    RemoteOpen {
        /// Host the request came from, as verified by vegad
        #[arg(long)]
        from: String,
    },
}

struct Failure {
    code: &'static str,
    message: String,
    hint: &'static str,
}

impl Failure {
    fn new(code: &'static str, message: impl Into<String>, hint: &'static str) -> Self {
        Self {
            code,
            message: message.into(),
            hint,
        }
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    if cli.skill {
        print!("{SKILL}");
        return ExitCode::SUCCESS;
    }
    let result = match cli.cmd {
        None => {
            home();
            Ok(())
        }
        Some(Cmd::Resolve { text, cwd }) => resolve_cmd(&text, cwd),
        Some(Cmd::Action) => action().map_err(notify),
        Some(Cmd::Menu) => menu_cmd(),
        Some(Cmd::RemoteOpen { from }) => return remote_open_cmd(&from),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(f) => {
            println!("error{{code,message,hint}}:");
            println!("  {},{},{}", f.code, cell(&f.message), cell(f.hint));
            ExitCode::from(1)
        }
    }
}

fn home() {
    println!("description: Open selections, the workspace or the git diff from a herdr pane, VS Code first");
    println!("menu: s selection (file/dir), w workspace, d git diff (when the repo has changes)");
    println!("rules: file -> VS Code (+ git diff if changed, + MarkEdit for *.md); dir -> VS Code Explorer, Finder");
    println!("help: run `vega open resolve <text>` to test resolution; in herdr bind `vega.open.selection` to a key");
}

fn resolve_cmd(text: &str, cwd: Option<PathBuf>) -> Result<(), Failure> {
    let cwd = match cwd {
        Some(c) => c,
        None => {
            env::current_dir().map_err(|e| Failure::new("no_cwd", e.to_string(), "pass --cwd"))?
        }
    };
    match find(text, &cwd)? {
        Found::Pane(hits) => {
            println!("panes[{}]{{pane,machine}}:", hits.len());
            for h in &hits {
                let machine = match h {
                    pane::Hit::Local(_) => "this machine",
                    pane::Hit::Remote { machine, .. } => machine,
                };
                println!("  {},{}", h.display(), machine);
            }
            println!("choices[1]: herdr: jump to pane");
            if hits.iter().any(|h| matches!(h, pane::Hit::Remote { .. })) {
                println!("help: {REMOTE_HINT}");
            }
        }
        Found::One(t) => {
            let names: Vec<&str> = choices_for(&t, &facts(&t))
                .iter()
                .map(|c| c.label())
                .collect();
            println!("target: {}", t.display());
            println!("choices[{}]: {}", names.len(), names.join(","));
        }
        Found::Many { root, rels, .. } => {
            println!("root: {}", root.display());
            println!("candidates[{}]:", rels.len());
            for r in &rels {
                println!("  {r}");
            }
            println!("help: in herdr the menu ranks these by what the pane shows");
        }
    }
    Ok(())
}

/// herdr switches machines only on user input (upstream: vega-8m7).
const REMOTE_HINT: &str =
    "herdr focuses the pane on its machine; switch to that machine in herdr yourself";

/// A file or folder first; a selection that is none but holds a herdr pane
/// id resolves to the pane.
fn find(text: &str, cwd: &Path) -> Result<Found, Failure> {
    let file = locate(text, cwd);
    if file.is_ok() {
        return file;
    }
    let Some(r) = pane::parse(text) else {
        return file;
    };
    pane::resolve(
        &r,
        &vega_herdr::local_host(),
        &pane::machines(),
        pane::exists,
    )
    .map(Found::Pane)
    .map_err(|(code, message)| {
        Failure::new(
            code,
            message,
            "w1:p2 is looked up here, then on the herdr machines; io:w1:p2 names the machine",
        )
    })
}

fn locate(text: &str, cwd: &Path) -> Result<Found, Failure> {
    let home = env::var_os("HOME").map(PathBuf::from);
    let root = repo_root(cwd);
    let r = match target::find(text, cwd, root.as_deref(), home.as_deref()) {
        Ok(t) => return Ok(Found::One(t)),
        Err(None) => {
            return Err(Failure::new(
                "not_a_path",
                format!("no file path in {:?}", short(text)),
                "select a path like src/main.rs:12",
            ))
        }
        Err(Some(r)) => r,
    };
    let not_found = || {
        Failure::new(
            "not_found",
            format!("{} does not exist (cwd {})", r.path, cwd.display()),
            "relative paths resolve against the pane cwd, the git root, then repo files ending in it",
        )
    };
    let relative = !(r.path.starts_with('/') || r.path.starts_with('~') || r.path.contains(".."));
    let Some(root) = root.filter(|_| relative) else {
        return Err(not_found());
    };
    let mut rels = rank::suffix_matches(&r.path, &repo_files(&root));
    match rels.len() {
        0 => Err(not_found()),
        1 => Ok(Found::One(Target {
            path: root.join(rels.remove(0)),
            line: r.line,
            col: r.col,
        })),
        _ => Ok(Found::Many {
            root,
            suffix: r.path,
            rels,
            line: r.line,
            col: r.col,
        }),
    }
}

fn action() -> Result<(), Failure> {
    let ctx: Value = env::var("HERDR_PLUGIN_CONTEXT_JSON")
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or(Value::Null);
    let cwd = ctx["focused_pane_cwd"]
        .as_str()
        .or(ctx["workspace_cwd"].as_str())
        .map(PathBuf::from)
        .or_else(|| env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("/"));
    let mut env = vec![format!("VEGA_OPEN_CWD={}", cwd.display())];
    if let Some(pane) = ctx["focused_pane_id"].as_str() {
        env.push(format!("VEGA_OPEN_PANE={pane}"));
    }
    // Only set when herdr kept the selection (`ui.copy_on_select = false`).
    if let Some(text) = ctx["selected_text"]
        .as_str()
        .filter(|s| !s.trim().is_empty())
    {
        env.push(format!("VEGA_OPEN_SELECTED={text}"));
    }
    open_popup(&env)
}

/// Open the `menu` popup pane; `env` (KEY=VALUE) carries the inputs.
fn open_popup(env: &[String]) -> Result<(), Failure> {
    let mut args: Vec<String> = [
        "plugin",
        "pane",
        "open",
        "--plugin",
        PLUGIN_ID,
        "--entrypoint",
        "menu",
        "--focus",
    ]
    .map(String::from)
    .to_vec();
    for kv in env {
        args.extend(["--env".into(), kv.clone()]);
    }
    run(&herdr_bin(), &args, "menu_failed")
}

/// The popup has no caller to report to, so failures show in the popup.
fn menu_cmd() -> Result<(), Failure> {
    let result = home_menu();
    if let Err(f) = &result {
        let hint = format!("hint: {}", f.hint);
        let _ = menu::message(
            "vega open: cannot open",
            &[&f.message, &hint, "", "press any key"],
        );
    }
    result
}

fn home_menu() -> Result<(), Failure> {
    let cwd = env::var_os("VEGA_OPEN_CWD")
        .map(PathBuf::from)
        .or_else(|| env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("/"));
    let screen = env::var("VEGA_OPEN_PANE")
        .ok()
        .and_then(|p| pane_screen(&p))
        .unwrap_or_default();
    let text = env::var("VEGA_OPEN_SELECTED")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| clipboard().and_then(|c| home::fresh_clipboard(&c, &screen)));

    // A selection that does not resolve is reported in the title, so the
    // workspace and diff rows stay usable.
    let mut title = "vega open".to_string();
    let found = match text.as_deref().map(|t| (t, find(t, &cwd))) {
        Some((_, Ok(found))) => Some(found),
        Some((t, Err(f))) => {
            title = format!("vega open · {:?}: {}", short(t), f.code);
            None
        }
        None => None,
    };
    let repo = repo_root(&cwd);
    let changed = repo.as_deref().map_or(0, count_changes);
    let rows = home::rows(
        found.as_ref().map(|f| f.label(repo.as_deref())).as_deref(),
        repo.as_deref(),
        &cwd,
        changed,
    );
    let items: Vec<(char, &str)> = rows.iter().map(|r| (r.key, r.label.as_str())).collect();
    let Some(i) = menu::pick(&title, &items).map_err(tty_error)? else {
        return Ok(());
    };
    match rows[i].entry {
        home::Entry::Selection => match found {
            Some(found) => open_found(found, &screen),
            None => Ok(()),
        },
        home::Entry::Workspace => execute(Request::Workspace {
            dir: repo.unwrap_or(cwd),
        }),
        home::Entry::Diff => match repo {
            Some(repo) => execute(Request::Scm { repo }),
            None => Ok(()),
        },
    }
}

impl Found {
    /// Menu label: the target relative to the repo, or the ambiguous path.
    fn label(&self, repo: Option<&Path>) -> String {
        match self {
            Found::One(t) => {
                let shown = repo
                    .and_then(|r| t.path.strip_prefix(r).ok())
                    .map(|rel| {
                        Target {
                            path: rel.to_path_buf(),
                            ..t.clone()
                        }
                        .display()
                    })
                    .unwrap_or_else(|| t.display());
                short(&shown)
            }
            Found::Many { suffix, rels, .. } => format!("{suffix} ({} matches)", rels.len()),
            Found::Pane(hits) => match hits.as_slice() {
                [one] => format!("jump to pane {}", one.display()),
                _ => format!(
                    "jump to pane {} ({} machines)",
                    pane_of(&hits[0]),
                    hits.len()
                ),
            },
        }
    }
}

/// Pick the file (if ambiguous), then the app (if several fit), then open.
fn open_found(found: Found, screen: &str) -> Result<(), Failure> {
    let t = match found {
        Found::Pane(hits) => return jump(hits),
        Found::One(t) => t,
        Found::Many {
            root,
            suffix,
            rels,
            line,
            col,
        } => {
            // Ranked best first, so the preselected first row is the best guess.
            let mut ranked = rank::rank(rels, &suffix, screen, &root);
            ranked.truncate(MAX_CANDIDATES);
            let labels: Vec<&str> = ranked.iter().map(String::as_str).collect();
            let Some(i) = menu::pick("Which file?", &menu::numbered(&labels)).map_err(tty_error)?
            else {
                return Ok(());
            };
            Target {
                path: root.join(&ranked[i]),
                line,
                col,
            }
        }
    };
    let options = choices_for(&t, &facts(&t));
    if let [only] = options.as_slice() {
        return launch(*only, &t);
    }
    let labels: Vec<&str> = options.iter().map(|c| c.label()).collect();
    let title = format!("Open {}", short(&t.display()));
    match menu::pick(&title, &menu::numbered(&labels)).map_err(tty_error)? {
        Some(i) => launch(options[i], &t),
        None => Ok(()),
    }
}

fn pane_of(hit: &pane::Hit) -> &str {
    match hit {
        pane::Hit::Local(p) | pane::Hit::Remote { pane: p, .. } => p,
    }
}

/// Pick the machine (if the id exists on several), then focus the pane.
fn jump(hits: Vec<pane::Hit>) -> Result<(), Failure> {
    let hit = match hits.as_slice() {
        [one] => one,
        _ => {
            let labels: Vec<String> = hits.iter().map(pane::Hit::display).collect();
            let labels: Vec<&str> = labels.iter().map(String::as_str).collect();
            let Some(i) =
                menu::pick("Which machine?", &menu::numbered(&labels)).map_err(tty_error)?
            else {
                return Ok(());
            };
            &hits[i]
        }
    };
    pane::focus(hit).map_err(|e| {
        Failure::new(
            "focus_failed",
            format!("{}: {e}", hit.display()),
            "check that the pane still exists (herdr pane list)",
        )
    })?;
    if let pane::Hit::Remote { machine, .. } = hit {
        let done = format!("focused {} on {machine}", hit.display());
        let _ = menu::message("vega open", &[&done, REMOTE_HINT, "", "press any key"]);
    }
    Ok(())
}

fn tty_error(e: std::io::Error) -> Failure {
    Failure::new("tty_error", e.to_string(), "run inside a herdr popup")
}

fn launch(choice: Choice, t: &Target) -> Result<(), Failure> {
    execute(Request::Open {
        choice,
        target: t.clone(),
        repo: facts(t).repo,
    })
}

/// Run the pick here, or send it to the user's machine when this host has
/// an `open-target` (a remote herdr host whose panes the user views from
/// there).
fn execute(req: Request) -> Result<(), Failure> {
    match mounts::open_target() {
        Some(host) => send_remote(&host, &req),
        None => run_all(req.commands()),
    }
}

/// Max payload vegad forwards.
const MAX_PAYLOAD: usize = 4096;

/// `vega post cmd open --to <host>`: vegad there runs `remote-open`.
fn send_remote(host: &str, req: &Request) -> Result<(), Failure> {
    let unavailable = |msg: String| {
        Failure::new(
            "remote_unavailable",
            msg,
            "needs vega post cmd (vegad command channel) here and vegad on the target",
        )
    };
    let bin = env::var_os("HOME")
        .map(|h| Path::new(&h).join(".local/bin/vega-post"))
        .filter(|p| p.exists())
        .unwrap_or_else(|| PathBuf::from("vega-post"));
    let mut child = Command::new(&bin)
        .args(["cmd", "open", "--to", host])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| unavailable(format!("{}: {e}", bin.display())))?;
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(req.to_json().to_string().as_bytes());
    }
    let out = child
        .wait_with_output()
        .map_err(|e| unavailable(e.to_string()))?;
    if out.status.success() {
        return Ok(());
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    // AXI error: a `error{code,message,hint}:` header, then one row.
    let row = stdout
        .lines()
        .skip_while(|l| !l.starts_with("error{"))
        .nth(1)
        .map(str::trim)
        .map(String::from);
    match (out.status.code(), row) {
        (Some(1), Some(row)) => Err(Failure::new(
            "remote_failed",
            format!("{host}: {row}"),
            "see the message from the target host",
        )),
        _ => Err(unavailable(format!(
            "vega post cmd open --to {host} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ))),
    }
}

/// One stdout line either way: vegad replies with the first line only.
fn remote_open_cmd(from: &str) -> ExitCode {
    match remote_open(from) {
        Ok(shown) => {
            println!("opened: {shown}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            println!("error: {}", e.replace('\n', " "));
            ExitCode::from(1)
        }
    }
}

fn remote_open(from: &str) -> Result<String, String> {
    if from.is_empty() {
        return Err("empty --from".into());
    }
    let mut buf = String::new();
    std::io::stdin()
        .take(MAX_PAYLOAD as u64 + 1)
        .read_to_string(&mut buf)
        .map_err(|e| format!("stdin: {e}"))?;
    if buf.len() > MAX_PAYLOAD {
        return Err(format!("payload over {MAX_PAYLOAD} bytes"));
    }
    let v = serde_json::from_str(&buf).map_err(|e| format!("bad JSON: {e}"))?;
    let mounts = mounts::load()?;
    let req = Request::from_json(&v)?.map_paths(|p| mounts::map(&mounts, from, p))?;
    run_all(req.commands()).map_err(|f| format!("{}: {}", f.code, f.message))?;
    Ok(req.display())
}

fn run_all(commands: Vec<Vec<String>>) -> Result<(), Failure> {
    for argv in commands {
        run(&argv[0], &argv[1..], "launch_failed")?;
    }
    Ok(())
}

/// Entries in `git status`, untracked included: Source Control lists them.
fn count_changes(repo: &Path) -> usize {
    Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["status", "--porcelain"])
        .output()
        .map_or(0, |out| {
            String::from_utf8_lossy(&out.stdout).lines().count()
        })
}

/// Git facts for the target: its repo root and whether it differs from HEAD.
fn facts(t: &Target) -> Facts {
    let dir = if t.is_dir() {
        t.path.as_path()
    } else {
        t.path.parent().unwrap_or(Path::new("/"))
    };
    let repo = repo_root(dir);
    let changed = !t.is_dir() && repo.as_deref().is_some_and(|r| is_changed(r, &t.path));
    Facts { repo, changed }
}

/// Tracked and modified or staged. Untracked files have no diff to show.
fn is_changed(repo: &Path, file: &Path) -> bool {
    Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["status", "--porcelain", "--"])
        .arg(file)
        .output()
        .is_ok_and(|out| {
            String::from_utf8_lossy(&out.stdout)
                .lines()
                .any(|l| !l.is_empty() && !l.starts_with("??"))
        })
}

fn run(program: &str, args: &[String], code: &'static str) -> Result<(), Failure> {
    let out = Command::new(program).args(args).output().map_err(|e| {
        Failure::new(
            code,
            format!("{program}: {e}"),
            "check that the program exists",
        )
    })?;
    if out.status.success() {
        Ok(())
    } else {
        Err(Failure::new(
            code,
            String::from_utf8_lossy(&out.stderr).trim().to_string(),
            "see the message above",
        ))
    }
}

fn clipboard() -> Option<String> {
    let out = Command::new("/usr/bin/pbpaste").output().ok()?;
    Some(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn repo_root(cwd: &Path) -> Option<PathBuf> {
    let out = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .ok()?;
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (out.status.success() && !s.is_empty()).then(|| PathBuf::from(s))
}

/// Repo files, tracked and untracked-but-not-ignored, relative to `root`.
fn repo_files(root: &Path) -> Vec<String> {
    let Ok(out) = Command::new("git")
        .arg("-C")
        .arg(root)
        .args([
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
        ])
        .output()
    else {
        return Vec::new();
    };
    out.stdout
        .split(|b| *b == 0)
        .filter(|f| !f.is_empty())
        .map(|f| String::from_utf8_lossy(f).into_owned())
        .collect()
}

/// The visible text of a pane, for ranking candidates.
fn pane_screen(pane_id: &str) -> Option<String> {
    let out = Command::new(herdr_bin())
        .args([
            "pane", "read", pane_id, "--source", "visible", "--format", "text",
        ])
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// The popup could not open, so there is nowhere in herdr to show the
/// error but a notification.
fn notify(f: Failure) -> Failure {
    let _ = Command::new(herdr_bin())
        .args([
            "notification",
            "show",
            "vega open",
            "--body",
            &format!("{} ({})", f.message, f.hint),
        ])
        .output();
    f
}

fn herdr_bin() -> String {
    env::var("HERDR_BIN_PATH").unwrap_or_else(|_| "herdr".into())
}

fn short(s: &str) -> String {
    let s = s.trim();
    if s.chars().count() > 60 {
        format!("{}…", s.chars().take(59).collect::<String>())
    } else {
        s.to_string()
    }
}

fn cell(s: &str) -> String {
    if s.contains([',', '"', '\n']) {
        format!("\"{}\"", s.replace('"', "\"\"").replace('\n', " "))
    } else {
        s.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skill_has_valid_frontmatter() {
        vega_plugin::assert_guide(SKILL, "open");
    }
}
