//! `vega skills`: the index of the plugins' agent guides and one guide by
//! name; `vega skills lib` manages the skills library.
//!
//! Each plugin prints its guide with `vega-<name> --skill`, in SKILL.md form
//! (frontmatter plus body). The index shows only the frontmatter descriptions,
//! so an agent loads the one guide it needs, not all of them. Library skills
//! (docs/design/skills-management.md) come from a vega-owned clone; skills.sh
//! (`skills`) serves and installs them.

mod library;

use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::thread;
use std::time::{Duration, Instant};
use vega_plugin::{find_on_path, parse_frontmatter, plugins, PREFIX};

use library::{collapse_home, Skill};

/// A guide is a constant in the plugin; anything slower is a broken plugin.
/// `VEGA_SKILL_TIMEOUT_MS` overrides it (tests under load).
fn timeout() -> Duration {
    std::env::var("VEGA_SKILL_TIMEOUT_MS")
        .ok()
        .and_then(|s| s.parse().ok())
        .map_or(Duration::from_secs(5), Duration::from_millis)
}

/// Library commands that were top-level before `vega skills lib`.
const MOVED: [&str; 5] = ["install", "remove", "list", "sync", "doctor"];

const HELP: &str = "\
Usage: vega skills [NAME]
       vega skills lib [COMMAND]

Without NAME: the index of the vega plugin guides (no bodies).
With NAME: that plugin's guide (`vega-NAME --skill`).

  lib   the skills library: index, show, list, install, remove, sync,
        doctor, add, update; run `vega skills lib --help`

Examples:
  vega skills            # which guides exist
  vega skills mux        # the vega mux guide
  vega skills lib        # which library skills exist
";

const LIB_HELP: &str = include_str!("../help/lib.md");

const SHOW_HELP: &str = "\
Usage: vega skills lib show NAME

Prints a library skill: a header naming the skill's own dir in the library
clone (absolute, links resolved; also for skills vendored under
.agents/skills), then its SKILL.md unchanged. Relative paths in the skill
(scripts/, references/) resolve against that dir; it is read-only, keep state
in the project. Installs nothing. Clones the library on first use, never pulls.

Examples:
  vega skills lib show caveman
";

const INSTALL_HELP: &str = include_str!("../help/install.md");

const REMOVE_HELP: &str = "\
Usage: vega skills lib remove NAME... [-g]

Uninstalls skills with skills.sh: from this project (git root), or with -g
globally, also dropping them from ~/.config/vega/skills-global.

Examples:
  vega skills lib remove beads
  vega skills lib remove caveman -g
";

const SYNC_HELP: &str = "\
Usage: vega skills lib sync

Clones the library from ~/.config/vega/skills-remote, or fast-forwards the clone,
then installs every skill in ~/.config/vega/skills-global globally at the library
commit; up-to-date ones are left alone. `vega deploy apply` runs it on the target.
A new machine needs both files in ~/.config/vega; they are not synced by vega.

Examples:
  vega skills lib sync
";

const DOCTOR_HELP: &str = include_str!("../help/doctor.md");

const LIST_HELP: &str = include_str!("../help/list.md");

const ADD_HELP: &str = "\
Usage: vega skills lib add OWNER -s NAME...

Vendors skills from their owner repo into the library clone: runs `skills add
OWNER --copy -s NAME --agent universal -y` there (lands in .agents/skills,
recorded in its skills-lock.json), retrying with --full-depth when the skill is
not found at the top levels, then commits. Never pushes: push the printed
commit with `git -C <clone> push`. Refused on a dirty clone; on a failure the
clone is reset to its commit. OWNER: owner/repo on GitHub, a git URL or a local
path.

Examples:
  vega skills lib add kunchenguid/axi -s axi
  vega skills lib add durandom/discord -s discord-search
";

const UPDATE_HELP: &str = "\
Usage: vega skills lib update [NAME...]

Re-vendors skills from the owner recorded in the library clone's
skills-lock.json (all vendored skills without NAME), as `lib add` does, and
commits the changes. Fixes `library_stale`. Never pushes: push the printed
commit with `git -C <clone> push`. Refused on a dirty clone. Skills the library
owns itself (skills/NAME) are not vendored and cannot be updated here.

Examples:
  vega skills lib update discord-search
  vega skills lib update
";

const SKILL: &str = include_str!("../SKILL.md");

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    match args.as_slice() {
        [] => index(),
        [a] if a == "-h" || a == "--help" => help(HELP),
        [a] if a == "-v" || a == "-V" || a == "--version" => {
            println!("{}", vega_build::version_line!());
            ExitCode::SUCCESS
        }
        [a] if a == "--skill" => help(SKILL),
        [lib, rest @ ..] if lib == "lib" => lib_main(rest),
        [cmd, ..] if MOVED.contains(&cmd.as_str()) => {
            print_failure(
                "usage",
                &format!("`vega skills {cmd}` moved to `vega skills lib {cmd}`"),
                &format!("run `vega skills lib {}`", args.join(" ")),
            );
            ExitCode::from(2)
        }
        [a] if a.starts_with('-') => usage(&format!("unknown flag {a}")),
        [name] => guide(name.strip_prefix(PREFIX).unwrap_or(name)),
        _ => usage("expected at most one NAME"),
    }
}

fn help(text: &str) -> ExitCode {
    print!("{text}");
    ExitCode::SUCCESS
}

/// `vega skills lib ...`
fn lib_main(args: &[String]) -> ExitCode {
    let Some((cmd, rest)) = args.split_first() else {
        return lib_index();
    };
    let sub_help = match cmd.as_str() {
        "-h" | "--help" => return help(LIB_HELP),
        "show" => SHOW_HELP,
        "list" => LIST_HELP,
        "install" => INSTALL_HELP,
        "remove" => REMOVE_HELP,
        "sync" => SYNC_HELP,
        "doctor" => DOCTOR_HELP,
        "add" => ADD_HELP,
        "update" => UPDATE_HELP,
        _ => return usage_for(&format!("unknown lib command {cmd}"), LIB_HELP),
    };
    if rest.iter().any(|a| a == "-h" || a == "--help") {
        return help(sub_help);
    }
    match cmd.as_str() {
        "install" | "remove" => match names_and_global(rest) {
            Ok((names, global)) if cmd == "install" => finish(install(&names, global)),
            Ok((names, global)) => finish(remove(&names, global)),
            Err(m) => usage_for(&m, sub_help),
        },
        "show" => match rest {
            [name] if !name.starts_with('-') => finish(show(name)),
            _ => usage_for("show takes one skill NAME", sub_help),
        },
        "add" => match owner_and_names(rest) {
            Ok((owner, names)) => finish(vendor(Some(&owner), &names)),
            Err(m) => usage_for(&m, sub_help),
        },
        "update" => match rest.iter().find(|a| a.starts_with('-')) {
            Some(f) => usage_for(&format!("unknown flag {f}"), sub_help),
            None => finish(vendor(None, rest)),
        },
        "doctor" if rest == ["--fix"] => finish(doctor(true)),
        "list" if rest == ["-g"] || rest == ["--global"] => finish(list(true)),
        _ if !rest.is_empty() => usage_for(&format!("{cmd} takes no arguments"), sub_help),
        "sync" => finish(sync()),
        "list" => finish(list(false)),
        _ => finish(doctor(false)),
    }
}

fn names_and_global(args: &[String]) -> Result<(Vec<String>, bool), String> {
    let mut names = Vec::new();
    let mut global = false;
    for a in args {
        match a.as_str() {
            "-g" | "--global" => global = true,
            f if f.starts_with('-') => return Err(format!("unknown flag {f}")),
            n => names.push(n.to_string()),
        }
    }
    if names.is_empty() {
        return Err("no skill NAME given".into());
    }
    Ok((names, global))
}

/// `OWNER -s NAME...`; `-s` may repeat.
fn owner_and_names(args: &[String]) -> Result<(String, Vec<String>), String> {
    let mut owner = None;
    let mut names = Vec::new();
    let mut in_names = false;
    for a in args {
        match a.as_str() {
            "-s" | "--skill" => in_names = true,
            f if f.starts_with('-') => return Err(format!("unknown flag {f}")),
            n if in_names => names.push(n.to_string()),
            o if owner.is_none() => owner = Some(o.to_string()),
            x => return Err(format!("unexpected argument {x}; skill names follow -s")),
        }
    }
    let owner = owner.ok_or("no OWNER given")?;
    if names.is_empty() {
        return Err("no skill NAME given after -s".into());
    }
    Ok((owner, names))
}

struct Failure {
    code: &'static str,
    message: String,
    hint: String,
}

fn fail(code: &'static str, message: impl Into<String>, hint: impl Into<String>) -> Failure {
    Failure {
        code,
        message: message.into(),
        hint: hint.into(),
    }
}

fn finish(result: Result<ExitCode, Failure>) -> ExitCode {
    match result {
        Ok(code) => code,
        Err(f) => {
            print_failure(f.code, &f.message, &f.hint);
            ExitCode::from(if f.code == "usage" { 2 } else { 1 })
        }
    }
}

/// The plugins with an agent guide slot: every `vega-*` on PATH.
fn guide_plugins() -> Vec<String> {
    plugins().into_iter().collect()
}

/// The library state the index reads; never touches the network.
enum Library {
    NotConfigured,
    NotCloned(String),
    Ready {
        remote: String,
        clone: PathBuf,
        skills: BTreeMap<String, Skill>,
    },
}

fn library() -> Library {
    let Some(remote) = library::remote() else {
        return Library::NotConfigured;
    };
    match library::clone_dir().filter(|c| c.join(".git").exists()) {
        Some(clone) => Library::Ready {
            remote,
            skills: library::skills(&clone),
            clone,
        },
        None => Library::NotCloned(remote),
    }
}

fn index() -> ExitCode {
    let bin = env::current_exe()
        .map(|p| collapse_home(&p.display().to_string()))
        .unwrap_or_else(|_| "unknown".into());
    println!("bin: {bin}");
    println!("description: Index of the vega plugin guides; `vega skills <name>` prints one");
    let names = guide_plugins();
    if names.is_empty() {
        println!("plugins: 0 vega plugins found on PATH");
    } else {
        println!("plugins[{}]{{name,description}}:", names.len());
        for name in &names {
            let description = match load(name) {
                Ok(text) => match parse_frontmatter(&text) {
                    Ok(fm) => fm.description,
                    Err(e) => format!("invalid guide ({e}); run `vega {name} --help`"),
                },
                Err(e) => format!("{e}; run `vega {name} --help`"),
            };
            println!("  {name},{}", cell(&description));
        }
    }
    println!("help[3]:");
    println!("  Run `vega skills <name>` for one full guide");
    println!("  Run `vega skills lib` for the skills library");
    println!("  `vega <name>` shows live state, `vega <name> --help` the reference; errors print error{{code,message,hint}}, exit 1 runtime, exit 2 usage");
    ExitCode::SUCCESS
}

/// `vega skills lib`: the library skills and where each is installed.
fn lib_index() -> ExitCode {
    let mut help = Vec::new();
    match library() {
        Library::NotConfigured => {
            println!("library: not configured");
            help.push(format!("Set the library: {}", library::set_remote_hint()));
        }
        Library::NotCloned(remote) => {
            println!("library: {} (not cloned)", cell(&remote));
            help.push("Run `vega skills lib sync` to clone it".into());
        }
        Library::Ready {
            remote,
            clone,
            skills,
        } => {
            let at = library::head(&clone).unwrap_or_else(|| "?".into());
            println!("library: {} @ {at}", cell(&remote));
            println!(
                "clone: {}",
                cell(&collapse_home(&clone.display().to_string()))
            );
            if skills.is_empty() {
                println!("skills: 0 skills in the library");
            } else {
                let root = project_root();
                println!("skills[{}]{{name,installed,description}}:", skills.len());
                for s in skills.values() {
                    println!(
                        "  {},{},{}",
                        s.name,
                        installed_where(&s.name, root.as_deref()),
                        cell(&s.description)
                    );
                }
                help.push("`vega skills lib show <name>` prints one without installing".into());
                help.push("`vega skills lib install <name> [-g]` installs one".into());
            }
        }
    }
    help.push("`vega skills lib --help` lists every library command".into());
    println!("help[{}]:", help.len());
    for h in help {
        println!("  {h}");
    }
    ExitCode::SUCCESS
}

/// `project`, `global`, `both` or `no`.
fn installed_where(name: &str, project: Option<&Path>) -> &'static str {
    let p = project.is_some_and(|r| is_installed(r, name));
    let g = home().is_some_and(|h| is_installed(&h, name));
    match (p, g) {
        (true, true) => "both",
        (true, false) => "project",
        (false, true) => "global",
        (false, false) => "no",
    }
}

fn is_installed(base: &Path, name: &str) -> bool {
    exists_or_link(&base.join(".agents/skills").join(name))
        || exists_or_link(&base.join(".claude/skills").join(name))
}

fn exists_or_link(p: &Path) -> bool {
    fs::symlink_metadata(p).is_ok()
}

fn home() -> Option<PathBuf> {
    env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .map(PathBuf::from)
}

/// The git root of the working directory, if inside a repo.
fn project_root() -> Option<PathBuf> {
    let cwd = env::current_dir().ok()?;
    library::git(&cwd, &["rev-parse", "--show-toplevel"])
        .ok()
        .map(PathBuf::from)
}

fn guide(name: &str) -> ExitCode {
    let names = guide_plugins();
    if names.iter().any(|n| n == name) {
        return match load(name) {
            Ok(text) => {
                print!("{text}");
                if !text.ends_with('\n') {
                    println!();
                }
                ExitCode::SUCCESS
            }
            Err(e) => {
                print_failure("no_guide", &e, &format!("run `vega {name} --help`"));
                ExitCode::from(1)
            }
        };
    }
    let valid = if names.is_empty() {
        "none: no vega plugins on PATH".to_string()
    } else {
        names.join(" ")
    };
    let in_library =
        matches!(library(), Library::Ready { skills, .. } if skills.contains_key(name));
    let hint = if in_library {
        format!("{name} is a library skill: run `vega skills lib show {name}`")
    } else {
        "run `vega skills` for the guides, `vega skills lib` for library skills".into()
    };
    print_failure(
        "unknown_guide",
        &format!("no plugin guide named {name}; valid names: {valid}"),
        &hint,
    );
    ExitCode::from(2)
}

/// Prints a library skill straight from the clone: a header naming the
/// skill's own dir (absolute, links resolved), then SKILL.md as is. No copy,
/// so relative paths stay valid for the whole workflow.
fn show(name: &str) -> Result<ExitCode, Failure> {
    let (remote, clone, skills) = ready_library()?;
    let Some(skill) = skills.get(name) else {
        return Err(unknown_skills(&skills, &[name]));
    };
    let dir = skill
        .dir
        .canonicalize()
        .unwrap_or_else(|_| skill.dir.clone());
    let file = dir.join("SKILL.md");
    let text = fs::read_to_string(&file).map_err(|e| {
        fail(
            "read_failed",
            format!("{}: {e}", file.display()),
            "run `vega skills lib doctor`",
        )
    })?;
    let commit = library::head(&clone).unwrap_or_else(|| "unknown".into());
    println!("Base directory for this skill: {}", dir.display());
    println!(
        "Relative paths in this skill (scripts/, references/, ...) resolve against it. Read-only:"
    );
    println!("keep state in the project, not in this dir.");
    println!("library: {remote} @ {commit}");
    println!();
    print!("{text}");
    if !text.ends_with('\n') {
        println!();
    }
    Ok(ExitCode::SUCCESS)
}

fn unknown_skills(skills: &BTreeMap<String, Skill>, unknown: &[&str]) -> Failure {
    let valid: Vec<&str> = skills.keys().map(String::as_str).collect();
    fail(
        "unknown_skill",
        format!(
            "not in the library: {}; valid names: {}",
            unknown.join(" "),
            if valid.is_empty() {
                "none".into()
            } else {
                valid.join(" ")
            }
        ),
        "run `vega skills lib` for the index; `vega skills lib sync` pulls new library commits",
    )
}

/// `lib add` (with `owner`) and `lib update`: vendors skills into the clone
/// with `skills add OWNER --copy`, then commits. Never pushes.
fn vendor(owner: Option<&str>, names: &[String]) -> Result<ExitCode, Failure> {
    let (_, clone, _) = ready_library()?;
    let shown = collapse_home(&clone.display().to_string());
    let dirty = library::git(&clone, &["status", "--porcelain"])
        .map_err(|e| fail("git_failed", format!("{shown}: {e}"), "check the clone"))?;
    if !dirty.is_empty() {
        return Err(fail(
            "dirty_clone",
            format!("the library clone {shown} has uncommitted changes"),
            format!("commit or discard them first: git -C {shown} status"),
        ));
    }
    let lock = library::owners(&clone);
    // (name, owner) pairs; an owner path given relative to the caller's cwd
    // is made absolute, since skills.sh runs in the clone.
    let todo: Vec<(String, String)> = match owner {
        Some(o) => {
            let p = Path::new(o);
            let o = if p.exists() {
                p.canonicalize()
                    .map_or(o.to_string(), |c| c.display().to_string())
            } else {
                o.to_string()
            };
            names.iter().map(|n| (n.clone(), o.clone())).collect()
        }
        None if names.is_empty() => lock.into_iter().collect(),
        None => {
            let missing: Vec<&str> = names
                .iter()
                .filter(|n| !lock.contains_key(*n))
                .map(String::as_str)
                .collect();
            if !missing.is_empty() {
                return Err(fail(
                    "not_vendored",
                    format!(
                        "no owner recorded in the library's skills-lock.json for: {}",
                        missing.join(" ")
                    ),
                    "a skill the library owns (skills/NAME) is edited in the clone; add a new one with `vega skills lib add OWNER -s NAME`",
                ));
            }
            names.iter().map(|n| (n.clone(), lock[n].clone())).collect()
        }
    };
    if todo.is_empty() {
        println!("vendored: 0 skills in {shown}/skills-lock.json");
        return Ok(ExitCode::SUCCESS);
    }
    let engine = engine()?;
    let mut rows = Vec::new();
    for (name, owner) in &todo {
        let dir = Path::new(".agents/skills").join(name);
        let existed = clone.join(&dir).exists();
        let mut args = vec!["add", owner.as_str(), "--copy", "-s", name.as_str()];
        args.extend(["--agent", "universal", "-y"]);
        let run = engine.run(&clone, &args).or_else(|_| {
            args.push("--full-depth");
            engine.run(&clone, &args)
        });
        if let Err(f) = run {
            // Undo this run's changes; the clone was clean before.
            let _ = library::git(&clone, &["reset", "-q", "--hard", "HEAD"]);
            let _ = library::git(&clone, &["clean", "-qfd"]);
            return Err(fail(
                f.code,
                format!("{name} from {owner}: {}", f.message),
                format!(
                    "check the owner and skill name (`skills add {owner} --list`); {shown} is reset"
                ),
            ));
        }
        let changed = library::git(
            &clone,
            &["status", "--porcelain", "--", &dir.display().to_string()],
        )
        .is_ok_and(|s| !s.is_empty());
        let status = match (existed, changed) {
            (false, _) => "added",
            (true, true) => "updated",
            (true, false) => "unchanged",
        };
        rows.push((name.clone(), owner.clone(), status));
    }
    library::git(&clone, &["add", "-A"]).map_err(|e| {
        fail(
            "git_failed",
            format!("git add in {shown}: {e}"),
            "check the clone",
        )
    })?;
    let staged = library::git(&clone, &["diff", "--cached", "--name-only"]).unwrap_or_default();
    println!("vendored[{}]{{name,owner,status}}:", rows.len());
    for (n, o, s) in &rows {
        println!("  {n},{},{s}", cell(o));
    }
    if staged.is_empty() {
        println!("commit: none (no changes)");
        return Ok(ExitCode::SUCCESS);
    }
    let changed: Vec<&str> = rows
        .iter()
        .filter(|r| r.2 != "unchanged")
        .map(|r| r.0.as_str())
        .collect();
    let changed = if changed.is_empty() {
        "skills-lock.json".to_string()
    } else {
        changed.join(", ")
    };
    let subject = match owner {
        Some(o) => format!("Add {changed} from {}", library::slug(o)),
        None => format!("Update {changed} from their owners"),
    };
    library::git(&clone, &["commit", "-q", "-m", &subject]).map_err(|e| {
        fail(
            "git_failed",
            format!("commit in {shown}: {e}"),
            format!("the changes are staged; commit by hand: git -C {shown} commit"),
        )
    })?;
    let at = library::head(&clone).unwrap_or_else(|| "?".into());
    println!("commit: {at} {}", cell(&subject));
    println!("help: not pushed; publish it: git -C {shown} push");
    Ok(ExitCode::SUCCESS)
}

/// skills.sh: `VEGA_SKILLS_ENGINE`, else `skills` on PATH, else `npx -y skills`.
struct Engine {
    program: PathBuf,
    prefix: Vec<&'static str>,
}

fn engine() -> Result<Engine, Failure> {
    if let Some(p) = env::var_os("VEGA_SKILLS_ENGINE").filter(|p| !p.is_empty()) {
        return Ok(Engine {
            program: PathBuf::from(p),
            prefix: vec![],
        });
    }
    if let Some(p) = find_on_path("skills") {
        return Ok(Engine {
            program: p,
            prefix: vec![],
        });
    }
    if let Some(p) = find_on_path("npx") {
        return Ok(Engine {
            program: p,
            prefix: vec!["-y", "skills"],
        });
    }
    Err(fail(
        "no_engine",
        "skills.sh not found: neither `skills` nor `npx` on PATH",
        "install it: `brew install skills` or `npm install -g skills`",
    ))
}

impl Engine {
    /// Runs skills.sh non-interactively, telemetry off; returns stdout.
    fn run(&self, cwd: &Path, args: &[&str]) -> Result<String, Failure> {
        let out = Command::new(&self.program)
            .args(&self.prefix)
            .args(args)
            .current_dir(cwd)
            .env("DO_NOT_TRACK", "1")
            .env("NO_COLOR", "1")
            .stdin(Stdio::null())
            .output()
            .map_err(|e| {
                fail(
                    "engine_failed",
                    format!("{} did not start ({e})", self.program.display()),
                    "check `skills --version`",
                )
            })?;
        let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
        if out.status.success() {
            return Ok(stdout);
        }
        let all = format!("{stdout}\n{}", String::from_utf8_lossy(&out.stderr));
        Err(fail(
            "engine_failed",
            format!("`skills {}` failed: {}", args[0], last_lines(&all, 3)),
            "run `vega skills lib doctor`; check `skills --version`",
        ))
    }
}

/// The last `n` lines with text, ANSI codes and box drawing removed.
fn last_lines(s: &str, n: usize) -> String {
    let clean: Vec<String> = s
        .lines()
        .map(strip_ansi)
        .map(|l| {
            l.trim_matches(|c: char| c.is_whitespace() || "│◇■●└┌├╮╯─".contains(c))
                .to_string()
        })
        .filter(|l| !l.is_empty())
        .collect();
    clean[clean.len().saturating_sub(n)..].join(" / ")
}

fn strip_ansi(s: &str) -> String {
    let mut out = String::new();
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            for c in chars.by_ref() {
                if c.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// The configured library, cloned if missing (never pulled).
fn ready_library() -> Result<(String, PathBuf, BTreeMap<String, Skill>), Failure> {
    let remote = library::remote().ok_or_else(|| {
        fail(
            "not_configured",
            "no skills library is set",
            library::set_remote_hint(),
        )
    })?;
    let clone = library::clone_dir().ok_or_else(|| {
        fail(
            "no_home",
            "neither XDG_DATA_HOME nor HOME is set",
            "set HOME",
        )
    })?;
    library::fetch(&remote, &clone, false).map_err(|e| {
        fail(
            "clone_failed",
            format!("cloning {remote}: {e}"),
            "check the URL in ~/.config/vega/skills-remote and git access",
        )
    })?;
    let skills = library::skills(&clone);
    Ok((remote, clone, skills))
}

/// Where a scope's skill dirs live: the git root, or HOME with -g.
fn scope_base(global: bool) -> Result<PathBuf, Failure> {
    if global {
        return home().ok_or_else(|| fail("no_home", "HOME is not set", "set HOME"));
    }
    project_root().ok_or_else(|| {
        fail(
            "not_in_project",
            "the working directory is not in a git repo",
            "cd into the project, or add -g for a global install",
        )
    })
}

fn scope_name(global: bool) -> &'static str {
    if global {
        "global"
    } else {
        "project"
    }
}

fn install(names: &[String], global: bool) -> Result<ExitCode, Failure> {
    let base = scope_base(global)?;
    if let Some(n) = names.iter().find(|n| !global && owned(&base, n)) {
        return Err(fail(
            "owned_here",
            format!(
                "{n} is owned by this repo (.agents/skills/{n} links to skills/{n}); installing would replace the live source with the library copy"
            ),
            format!("keep it: edit skills/{n} here; `vega skills lib doctor` reports a stale library copy"),
        ));
    }
    let (remote, clone, skills) = ready_library()?;
    let unknown: Vec<&str> = names
        .iter()
        .filter(|n| !skills.contains_key(*n))
        .map(String::as_str)
        .collect();
    if !unknown.is_empty() {
        return Err(unknown_skills(&skills, &unknown));
    }
    add(&skills, &base, names, global)?;
    if !global {
        pin_lock(&base, &remote, &clone, &skills, names)?;
    }
    let at = library::head(&clone).unwrap_or_else(|| "?".into());
    println!("library: {at}");
    let failed = report_installed(&base, &skills, names, global);
    if global {
        let mut set = library::global_set();
        set.extend(names.iter().cloned());
        library::write_global_set(&set)
            .map_err(|e| fail("config_failed", e, "check ~/.config/vega permissions"))?;
    } else {
        println!("help: commit .agents/skills, .claude/skills and skills-lock.json");
    }
    Ok(if failed {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    })
}

/// `skills add <dir> --skill NAME` per skill, for Claude Code and the
/// `.agents/skills` readers. Each skill's own dir is the source: skills.sh does
/// not look into a clone's hidden `.agents/skills`, where vendored skills live.
fn add(
    skills: &BTreeMap<String, Skill>,
    base: &Path,
    names: &[String],
    global: bool,
) -> Result<(), Failure> {
    let engine = engine()?;
    for n in names {
        let dir = skills[n.as_str()].dir.display().to_string();
        let mut args = vec!["add", dir.as_str(), "--skill", n.as_str()];
        args.extend(["--agent", "claude-code", "codex", "-y"]);
        if global {
            args.push("-g");
        }
        engine.run(base, &args)?;
    }
    Ok(())
}

/// Points the `skills-lock.json` entries of `names` at the library remote.
/// skills.sh records the clone as a path relative to the project, valid only
/// on this machine at this checkout depth; the remote plus the skill's path
/// in the library is the same on every machine.
fn pin_lock(
    base: &Path,
    remote: &str,
    clone: &Path,
    skills: &BTreeMap<String, Skill>,
    names: &[String],
) -> Result<(), Failure> {
    let file = base.join("skills-lock.json");
    let broken = |e: String| {
        fail(
            "lock_failed",
            format!("{}: {e}", file.display()),
            "check skills-lock.json; `vega skills lib install` rewrites the entries",
        )
    };
    let Ok(text) = fs::read_to_string(&file) else {
        return Ok(());
    };
    let mut lock: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| broken(e.to_string()))?;
    let source = library::slug(remote);
    let github = source != remote;
    for n in names {
        let Some(entry) = lock["skills"].get_mut(n).and_then(|e| e.as_object_mut()) else {
            continue;
        };
        let dir = &skills[n.as_str()].dir;
        let rel = dir.strip_prefix(clone).unwrap_or(dir).join("SKILL.md");
        entry.insert("source".into(), source.clone().into());
        entry.insert(
            "sourceType".into(),
            if github { "github" } else { "git" }.into(),
        );
        if github {
            entry.remove("sourceUrl");
        } else {
            entry.insert("sourceUrl".into(), remote.into());
        }
        entry.insert("skillPath".into(), rel.display().to_string().into());
    }
    let out = serde_json::to_string_pretty(&lock).map_err(|e| broken(e.to_string()))? + "\n";
    if out != text {
        fs::write(&file, out).map_err(|e| broken(e.to_string()))?;
    }
    Ok(())
}

/// Prints where each skill landed; true when one is missing, unlinked or
/// differs from the library copy.
fn report_installed(
    base: &Path,
    skills: &BTreeMap<String, Skill>,
    names: &[String],
    global: bool,
) -> bool {
    let mut failed = false;
    println!("installed[{}]{{name,scope,path,status}}:", names.len());
    for n in names {
        let dir = base.join(".agents/skills").join(n);
        let status = if !dir.join("SKILL.md").is_file() {
            "missing"
        } else if !exists_or_link(&base.join(".claude/skills").join(n)) {
            "no_claude_link"
        } else if !library::same_tree(&dir, &skills[n.as_str()].dir) {
            "drift"
        } else {
            "ok"
        };
        failed |= status != "ok";
        println!(
            "  {n},{},{},{status}",
            scope_name(global),
            cell(&collapse_home(&dir.display().to_string()))
        );
    }
    if failed {
        println!(
            "help: run `vega skills lib doctor`; drift means skills.sh left an older copy in place"
        );
    }
    failed
}

fn remove(names: &[String], global: bool) -> Result<ExitCode, Failure> {
    let base = scope_base(global)?;
    let (present, absent): (Vec<&String>, Vec<&String>) =
        names.iter().partition(|n| is_installed(&base, n));
    let mut set = library::global_set();
    let in_set = |n: &String| set.contains(n);
    if present.is_empty() && !(global && absent.iter().any(|n| in_set(n))) {
        return Err(fail(
            "not_installed",
            format!(
                "not installed in {} scope: {}",
                scope_name(global),
                names.join(" ")
            ),
            "run `vega skills lib` to see where each skill is installed",
        ));
    }
    if !present.is_empty() {
        let mut args = vec!["remove"];
        args.extend(present.iter().map(|s| s.as_str()));
        args.push("-y");
        if global {
            args.push("-g");
        }
        engine()?.run(&base, &args)?;
    }
    if global {
        set.retain(|n| !names.contains(n));
        library::write_global_set(&set)
            .map_err(|e| fail("config_failed", e, "check ~/.config/vega permissions"))?;
    }
    let mut failed = false;
    println!("removed[{}]{{name,scope,status}}:", names.len());
    for n in names {
        let status = if is_installed(&base, n) {
            failed = true;
            "still_installed"
        } else if present.contains(&n) {
            "removed"
        } else {
            "not_installed"
        };
        println!("  {n},{},{status}", scope_name(global));
    }
    Ok(if failed {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    })
}

fn sync() -> Result<ExitCode, Failure> {
    let Some(remote) = library::remote() else {
        println!("library: not configured");
        println!("help[2]:");
        println!("  Set the library: {}", library::set_remote_hint());
        println!("  A new machine needs ~/.config/vega/skills-remote and skills-global; vega does not sync them");
        return Ok(ExitCode::SUCCESS);
    };
    let clone = library::clone_dir().ok_or_else(|| {
        fail(
            "no_home",
            "neither XDG_DATA_HOME nor HOME is set",
            "set HOME",
        )
    })?;
    let action = library::fetch(&remote, &clone, true).map_err(|e| {
        fail(
            "fetch_failed",
            format!("library {remote}: {e}"),
            format!(
                "fix the clone at {} (git status) or the URL in ~/.config/vega/skills-remote",
                collapse_home(&clone.display().to_string())
            ),
        )
    })?;
    let at = library::head(&clone).unwrap_or_else(|| "?".into());
    println!("library: {} @ {at} ({action})", cell(&remote));
    let set = library::global_set();
    if set.is_empty() {
        println!("global: 0 skills in ~/.config/vega/skills-global");
        println!("help: `vega skills lib install <name> -g` adds one");
        return Ok(ExitCode::SUCCESS);
    }
    let skills = library::skills(&clone);
    let home = home().ok_or_else(|| fail("no_home", "HOME is not set", "set HOME"))?;
    let mut status: Vec<(String, String)> = Vec::new();
    let mut todo = Vec::new();
    for n in &set {
        let s = match skills.get(n) {
            None => "not_in_library",
            Some(skill) if current(&home, skill) => "current",
            Some(_) if is_installed(&home, n) => {
                todo.push(n.clone());
                "updated"
            }
            Some(_) => {
                todo.push(n.clone());
                "installed"
            }
        };
        status.push((n.clone(), s.into()));
    }
    let mut engine_error = None;
    if !todo.is_empty() {
        if let Err(f) = add(&skills, &home, &todo, true) {
            engine_error = Some(f);
        }
    }
    let mut failed = false;
    println!("global[{}]{{name,status}}:", status.len());
    for (n, s) in &mut status {
        if (s == "installed" || s == "updated") && !current(&home, &skills[n.as_str()]) {
            *s = "failed".into();
        }
        failed |= s == "failed" || s == "not_in_library";
        println!("  {n},{s}");
    }
    if let Some(f) = engine_error {
        print_failure(f.code, &f.message, &f.hint);
    }
    if status.iter().any(|(_, s)| s == "not_in_library") {
        println!("help: drop missing names with `vega skills lib remove <name> -g`");
    }
    Ok(if failed {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    })
}

/// Installed at `base`, identical to the library copy, with a Claude link.
fn current(base: &Path, skill: &Skill) -> bool {
    let dir = base.join(".agents/skills").join(&skill.name);
    dir.join("SKILL.md").is_file()
        && base.join(".claude/skills").join(&skill.name).exists()
        && library::same_tree(&dir, &skill.dir)
}

struct Finding {
    scope: String,
    name: String,
    issue: &'static str,
    fix: String,
}

/// Agent-specific skill dirs read besides `.agents/skills`, relative to the
/// scope base: (global, project). Entries starting with `.` (codex `.system`)
/// are skipped. A new agent is one line here.
const AGENT_SKILL_DIRS: &[(&str, &str)] = &[
    (".pi/agent/skills", ".pi/skills"),
    (".codex/skills", ".codex/skills"),
];

fn doctor(fix: bool) -> Result<ExitCode, Failure> {
    let mut removed: Vec<(String, String, PathBuf)> = Vec::new();
    let mut findings = Vec::new();
    let mut found = |scope: &str, name: &str, issue: &'static str, fix: String| {
        findings.push(Finding {
            scope: scope.into(),
            name: name.into(),
            issue,
            fix,
        })
    };
    let mut skills = BTreeMap::new();
    match library::remote() {
        None => {
            println!("library: not configured");
            found("library", "-", "not_configured", library::set_remote_hint());
        }
        Some(remote) => {
            println!("library: {}", cell(&remote));
            match library::clone_dir().filter(|c| c.join(".git").exists()) {
                None => found("library", "-", "not_cloned", "vega skills lib sync".into()),
                Some(clone) => {
                    let shown = collapse_home(&clone.display().to_string());
                    if library::origin(&clone).as_deref() != Some(remote.as_str()) {
                        found(
                            "library",
                            "-",
                            "origin_differs",
                            "vega skills lib sync".into(),
                        );
                    }
                    if library::git(&clone, &["status", "--porcelain"]).is_ok_and(|s| !s.is_empty())
                    {
                        found(
                            "library",
                            "-",
                            "uncommitted_changes",
                            format!("commit or discard in {shown}"),
                        );
                    }
                    skills = library::skills(&clone);
                }
            }
        }
    }
    let home = home();
    let mut scopes: Vec<(&str, PathBuf)> = Vec::new();
    if let Some(root) = project_root().filter(|r| Some(r) != home.as_ref()) {
        scopes.push(("project", root));
    }
    if let Some(h) = &home {
        scopes.push(("global", h.clone()));
    }
    let checked: Vec<String> = scopes
        .iter()
        .map(|(s, p)| format!("{s} {}", collapse_home(&p.display().to_string())))
        .collect();
    println!("checked: {}", cell(&checked.join("; ")));
    for (scope, base) in &scopes {
        findings.extend(check_scope(scope, base, &skills, fix, &mut removed));
        if *scope == "global" {
            for n in library::global_set() {
                if !is_installed(base, &n) {
                    findings.push(Finding {
                        scope: scope.to_string(),
                        name: n,
                        issue: "not_installed",
                        fix: "vega skills lib sync".into(),
                    });
                }
            }
        }
    }
    if !removed.is_empty() {
        println!("removed[{}]{{scope,name,path}}:", removed.len());
        for (scope, name, path) in &removed {
            let shown = collapse_home(&path.display().to_string());
            println!("  {scope},{name},{}", cell(&shown));
        }
    }
    if findings.is_empty() {
        println!("findings: 0 issues");
        return Ok(ExitCode::SUCCESS);
    }
    println!("findings[{}]{{scope,name,issue,fix}}:", findings.len());
    for f in &findings {
        println!("  {},{},{},{}", f.scope, f.name, f.issue, cell(&f.fix));
    }
    Ok(ExitCode::from(1))
}

/// A scope's agent dirs relative to its base, in display order.
fn scope_dirs(global: bool) -> Vec<&'static str> {
    let mut v = vec![".agents/skills", ".claude/skills"];
    v.extend(
        AGENT_SKILL_DIRS
            .iter()
            .map(|(g, p)| if global { *g } else { *p }),
    );
    v
}

/// skills.sh lock entries of a scope, name to source: the project's
/// `skills-lock.json`, or the global `~/.agents/.skill-lock.json`.
fn lock_sources(base: &Path, global: bool) -> BTreeMap<String, String> {
    let file = base.join(if global {
        ".agents/.skill-lock.json"
    } else {
        "skills-lock.json"
    });
    let Some(json) = fs::read_to_string(file)
        .ok()
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
    else {
        return BTreeMap::new();
    };
    json["skills"]
        .as_object()
        .map(|m| {
            m.iter()
                .map(|(name, e)| {
                    let src = e["source"].as_str().unwrap_or("?");
                    (name.clone(), src.to_string())
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Where a link points, absolute when it can be resolved, `~` collapsed.
fn link_target(entry: &Path) -> String {
    let Ok(t) = fs::read_link(entry) else {
        return "?".into();
    };
    let abs = match entry.parent() {
        Some(parent) if t.is_relative() => parent.join(&t),
        _ => t,
    };
    collapse_home(&abs.canonicalize().unwrap_or(abs).display().to_string())
}

/// Whether `.agents/skills/<name>` links into the repo's own `skills/<name>`:
/// the repo at `base` owns the skill (vega-skills README).
fn owned(base: &Path, name: &str) -> bool {
    let entry = base.join(".agents/skills").join(name);
    let own = base.join("skills").join(name);
    fs::symlink_metadata(&entry).is_ok_and(|m| m.is_symlink())
        && own.join("SKILL.md").is_file()
        && entry.canonicalize().ok() == own.canonicalize().ok()
}

/// The source skills.sh takes for the git repo holding `dir`: `owner/repo`
/// for GitHub, other remotes as they are, `<owner/repo>` without a remote.
fn owner_repo(dir: &Path) -> String {
    library::origin(dir).map_or_else(|| "<owner/repo>".into(), |url| library::slug(&url))
}

/// The command that refreshes the library copy of a skill `base` owns:
/// `lib update` when the library vendors it, else `lib add` from this repo.
fn refresh_library(base: &Path, skill: &Skill) -> String {
    let vendored =
        library::clone_dir().is_some_and(|c| skill.dir.starts_with(c.join(".agents/skills")));
    if vendored {
        format!("vega skills lib update {}", skill.name)
    } else {
        add_to_library(&owner_repo(base), &skill.name)
    }
}

/// The command that vendors `owner`'s skill into the library.
fn add_to_library(owner: &str, name: &str) -> String {
    format!("vega skills lib add {owner} -s {name}")
}

struct Row {
    name: String,
    origin: &'static str,
    source: String,
    dirs: String,
    status: String,
    action: String,
}

fn list(global: bool) -> Result<ExitCode, Failure> {
    let base = scope_base(global)?;
    let scope = scope_name(global);
    let g = if global { " -g" } else { "" };
    println!(
        "scope: {scope} {}",
        cell(&collapse_home(&base.display().to_string()))
    );
    let mut help = Vec::new();
    let (skills, clone, remote) = match library() {
        Library::NotConfigured => {
            println!("library: not configured");
            help.push(format!(
                "Without the library no skill is `library`; set it: {}",
                library::set_remote_hint()
            ));
            (BTreeMap::new(), None, None)
        }
        Library::NotCloned(remote) => {
            println!("library: {} (not cloned)", cell(&remote));
            help.push("Run `vega skills lib sync` to clone the library".into());
            (BTreeMap::new(), None, None)
        }
        Library::Ready {
            remote,
            clone,
            skills,
        } => {
            let at = library::head(&clone).unwrap_or_else(|| "?".into());
            println!("library: {} @ {at}", cell(&remote));
            (skills, Some(clone), Some(remote))
        }
    };
    // The remote as written or as `pin_lock` records it, or a path into the
    // clone (skills.sh writes it relative to the project).
    let from_library = |src: &str| {
        remote
            .as_deref()
            .is_some_and(|r| r == src || library::slug(r) == src)
            || clone.as_ref().is_some_and(|c| {
                let p = base.join(src);
                let p = p.canonicalize().unwrap_or(p);
                p.starts_with(c.canonicalize().unwrap_or_else(|_| c.clone()))
            })
    };
    let agents = base.join(".agents/skills");
    let rels = scope_dirs(global);
    // An agent dir that is (a link to) .agents/skills holds nothing of its own.
    let dirs: Vec<(&str, PathBuf)> = rels
        .iter()
        .map(|r| (*r, base.join(r)))
        .filter(|(r, d)| {
            *r == ".agents/skills" || d.canonicalize().ok() != agents.canonicalize().ok()
        })
        .collect();
    let mut held: BTreeMap<String, Vec<&str>> = BTreeMap::new();
    for (rel, dir) in &dirs {
        for name in dir_names(dir) {
            if global && *rel == ".claude/skills" && name == "synced" {
                continue;
            }
            held.entry(name).or_default().push(rel);
        }
    }
    let mut issues: BTreeMap<String, Vec<Finding>> = BTreeMap::new();
    for f in check_scope(scope, &base, &skills, false, &mut Vec::new()) {
        issues.entry(f.name.clone()).or_default().push(f);
    }
    let lock = lock_sources(&base, global);
    let mut rows = Vec::new();
    for (name, in_dirs) in held {
        let entry = base.join(in_dirs[0]).join(&name);
        let is_link = fs::symlink_metadata(&entry).is_ok_and(|m| m.is_symlink());
        let upstream = lock.get(&name).filter(|src| !from_library(src));
        let (origin, source) = if is_link && !entry.exists() {
            ("broken", link_target(&entry))
        } else if !global && owned(&base, &name) {
            ("own", format!("skills/{name}"))
        } else if let Some(src) = upstream {
            ("upstream", src.clone())
        } else if is_link {
            ("link", link_target(&entry))
        } else if skills.contains_key(&name) || lock.contains_key(&name) {
            ("library", "-".into())
        } else {
            ("local", "-".into())
        };
        let found = issues.remove(&name).unwrap_or_default();
        let status = if !found.is_empty() {
            found.iter().map(|f| f.issue).collect::<Vec<_>>().join("+")
        } else if origin == "broken" {
            "broken_link".into()
        } else {
            "ok".into()
        };
        let action = match (found.first(), origin) {
            (Some(f), _) => f.fix.clone(),
            (None, "broken") => format!("rm {}", collapse_home(&entry.display().to_string())),
            (None, "library") => "keep".into(),
            // The global stub install-cli.sh links; it lives in the vega repo.
            (None, "link") if global && name == "vega" => "keep".into(),
            (None, "own") if skills.contains_key(&name) => "keep".into(),
            (None, "own") => add_to_library(&owner_repo(&base), &name),
            (None, _) if skills.contains_key(&name) => format!("vega skills lib install {name}{g}"),
            (None, "upstream") => add_to_library(&source, &name),
            (None, "link") => {
                let target = entry.canonicalize().unwrap_or_else(|_| entry.clone());
                add_to_library(&owner_repo(&target), &name)
            }
            (None, _) => add_to_library("<owner/repo>", &name),
        };
        rows.push(Row {
            name,
            origin,
            source,
            dirs: in_dirs.join(" "),
            status,
            action,
        });
    }
    let synced = base.join(".claude/skills/synced");
    if global && synced.is_dir() {
        rows.push(Row {
            name: "synced".into(),
            origin: "claude.ai",
            source: format!("{} synced", dir_names(&synced).len()),
            dirs: ".claude/skills".into(),
            status: "ok".into(),
            action: "keep".into(),
        });
    }
    if rows.is_empty() {
        println!("skills: 0 skills in {}", rels.join(" "));
    } else {
        println!(
            "skills[{}]{{name,origin,source,dirs,status,action}}:",
            rows.len()
        );
        for r in &rows {
            println!(
                "  {},{},{},{},{},{}",
                r.name,
                r.origin,
                cell(&r.source),
                r.dirs,
                r.status,
                cell(&r.action)
            );
        }
    }
    if !global {
        help.push("`vega skills lib list -g` shows the global dirs".into());
    }
    help.push("`vega skills lib doctor` checks the library too; `vega skills lib list --help` explains the columns".into());
    println!("help[{}]:", help.len());
    for h in help {
        println!("  {h}");
    }
    Ok(ExitCode::SUCCESS)
}

/// The per-scope checks of doctor, shared with list: Claude links, drift
/// against the library, broken links, shadowing entries in agent dirs. With
/// `fix`, safe shadows are removed and recorded in `removed`, not reported.
fn check_scope(
    scope: &str,
    base: &Path,
    skills: &BTreeMap<String, Skill>,
    fix: bool,
    removed: &mut Vec<(String, String, PathBuf)>,
) -> Vec<Finding> {
    let mut findings = Vec::new();
    let mut found = |scope: &str, name: &str, issue: &'static str, fix: String| {
        findings.push(Finding {
            scope: scope.into(),
            name: name.into(),
            issue,
            fix,
        })
    };
    let g = if scope == "global" { " -g" } else { "" };
    let agents = base.join(".agents/skills");
    let claude = base.join(".claude/skills");
    for name in dir_names(&agents) {
        let link = claude.join(&name);
        match fs::symlink_metadata(&link) {
            Err(_) => found(
                scope,
                &name,
                "missing_claude_link",
                format!(
                    "ln -s ../../.agents/skills/{name} {}",
                    collapse_home(&link.display().to_string())
                ),
            ),
            Ok(_) if !link.exists() => found(
                scope,
                &name,
                "broken_link",
                format!("vega skills lib install {name}{g}"),
            ),
            Ok(_) => {}
        }
        if let Some(skill) = skills.get(&name) {
            let same = library::same_tree(&agents.join(&name), &skill.dir);
            if scope == "project" && owned(base, &name) {
                if !same {
                    found(scope, &name, "library_stale", refresh_library(base, skill));
                }
            } else if !same {
                found(
                    scope,
                    &name,
                    "drift",
                    format!("vega skills lib install {name}{g} (resets to the library)"),
                );
            }
        }
    }
    for name in dir_names(&claude) {
        let link = claude.join(&name);
        let dangling = fs::symlink_metadata(&link).is_ok() && !link.exists();
        if dangling && !agents.join(&name).exists() {
            found(
                scope,
                &name,
                "broken_link",
                format!("rm {}", collapse_home(&link.display().to_string())),
            );
        }
    }
    for (global_rel, project_rel) in AGENT_SKILL_DIRS {
        let dir = base.join(if scope == "global" {
            global_rel
        } else {
            project_rel
        });
        // A dir that is (a link to) .agents/skills itself shadows nothing.
        if dir.canonicalize().ok() == agents.canonicalize().ok() {
            continue;
        }
        for name in dir_names(&dir) {
            let own = agents.join(&name);
            if !exists_or_link(&own) {
                continue;
            }
            let entry = dir.join(&name);
            let shown = collapse_home(&entry.display().to_string());
            let is_link = fs::symlink_metadata(&entry).is_ok_and(|m| m.is_symlink());
            let safe = is_link
                || (entry.is_dir()
                    && own.join("SKILL.md").is_file()
                    && library::same_tree(&entry, &own));
            if !safe {
                let own_shown = collapse_home(&own.display().to_string());
                found(
                    scope,
                    &name,
                    "shadow_differs",
                    format!("diff -r {shown} {own_shown}; keep what you need, then rm -r {shown}"),
                );
            } else if !fix {
                found(
                    scope,
                    &name,
                    "shadow",
                    "vega skills lib doctor --fix".into(),
                );
            } else {
                let gone = if is_link {
                    fs::remove_file(&entry)
                } else {
                    fs::remove_dir_all(&entry)
                };
                match gone {
                    Ok(()) => removed.push((scope.to_string(), name, entry)),
                    Err(e) => found(
                        scope,
                        &name,
                        "shadow",
                        format!("rm -r {shown} (--fix failed: {e})"),
                    ),
                }
            }
        }
    }
    findings
}

/// Entry names in a dir (links included), sorted; none if it does not exist.
fn dir_names(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .filter_map(|e| e.file_name().to_str().map(str::to_string))
                .filter(|n| !n.starts_with('.'))
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

/// Runs `vega-<name> --skill` and returns its stdout, or why there is no guide.
fn load(name: &str) -> Result<String, String> {
    if name == "skills" {
        return Ok(SKILL.to_string());
    }
    let bin = format!("{PREFIX}{name}");
    let path = find_on_path(&bin).ok_or_else(|| format!("no guide: {bin} not on PATH"))?;
    let mut child = Command::new(path)
        .arg("--skill")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("no guide: {bin} did not start ({e})"))?;
    // Read on a thread so a large guide cannot block the child on a full pipe.
    let mut stdout = child.stdout.take().expect("stdout is piped");
    let reader = thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stdout.read_to_end(&mut buf);
        buf
    });
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if start.elapsed() < timeout() => thread::sleep(Duration::from_millis(10)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("no guide: `{bin} --skill` timed out"));
            }
            Err(e) => return Err(format!("no guide: {bin} failed ({e})")),
        }
    };
    let out = reader.join().unwrap_or_default();
    if !status.success() {
        let code = status
            .code()
            .map_or("a signal".to_string(), |c| c.to_string());
        return Err(format!("no guide: `{bin} --skill` exited {code}"));
    }
    if out.is_empty() {
        return Err(format!("no guide: `{bin} --skill` printed nothing"));
    }
    Ok(String::from_utf8_lossy(&out).into_owned())
}

fn usage(message: &str) -> ExitCode {
    print_failure(
        "usage",
        message,
        "run `vega skills` or `vega skills <name>`",
    );
    ExitCode::from(2)
}

fn usage_for(message: &str, help: &str) -> ExitCode {
    print_failure("usage", message, "valid usage below");
    print!("{help}");
    ExitCode::from(2)
}

fn print_failure(code: &str, message: &str, hint: &str) {
    println!("error{{code,message,hint}}:");
    println!("  {code},{},{}", cell(message), cell(hint));
}

/// Quotes a TOON cell when it contains a delimiter or quote.
fn cell(s: &str) -> String {
    if s.contains([',', '"', '\n']) {
        format!("\"{}\"", s.replace('"', "\\\"").replace('\n', " "))
    } else {
        s.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skill_has_valid_frontmatter() {
        vega_plugin::assert_guide(SKILL, "skills");
    }

    #[test]
    fn last_lines_strips_decoration() {
        let s = "\u{1b}[1m│ ok\u{1b}[0m\n│\n■  No matching skills found for: nope\n";
        assert_eq!(last_lines(s, 2), "ok / No matching skills found for: nope");
    }
}
