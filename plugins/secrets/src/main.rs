//! `vega secrets`: API keys and credentials for tools and agents
//! (docs/design/secrets.md). The Slayer keeps the originals in 1Password;
//! hosts get age ciphertext through chezmoi and decrypt on demand with their
//! own identity. Values never reach stdout, argv or a log.

mod adopt;

use std::env;
use std::fs;
use std::io::{IsTerminal, Read};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};

use clap::error::ErrorKind;
use clap::{CommandFactory, Parser, Subcommand};
use vega_secrets::{valid_name, Entry, Error, Here, Manifest, Secret, Store};

#[derive(Parser)]
#[command(
    name = "vega secrets",
    bin_name = "vega secrets",
    disable_version_flag = true,
    about = "Secrets for tools and agents: inventory, env injection, set and rekey",
    after_help = "Examples:\n  vega secrets                                   # this host's secrets, no values\n  vega secrets run --for lanista -- lanista fetch\n  vega secrets set artificial-analysis           # the Slayer, on the admin host"
)]
struct Cli {
    /// Print the agent guide for this plugin, SKILL.md form (read by `vega skills`)
    #[arg(long)]
    skill: bool,
    /// Print the bare version
    #[arg(short = 'V', short_alias = 'v', long)]
    version: bool,
    /// Home view: also compare with 1Password items tagged `vega` (admin host)
    #[arg(long)]
    op: bool,
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Run a command with the secrets for one tool added to its env
    #[command(
        after_help = "Only secrets whose `for` lists the tool are injected. A secret this host cannot decrypt is skipped with a note on stderr; the command still runs. The exit code is the command's.\n\nExamples:\n  vega secrets run --for lanista -- lanista fetch"
    )]
    Run {
        /// The tool whose secrets to inject (a name in the secrets' `for`)
        #[arg(long = "for", value_name = "TOOL")]
        tool: String,
        /// The command and its arguments, after `--`
        #[arg(trailing_var_arg = true, required = true, num_args = 1..)]
        cmd: Vec<String>,
    },
    /// Store or rotate a secret (the Slayer, admin host only)
    #[command(
        after_help = "The value comes from `op read <op ref>` if the secret has one, else a hidden prompt on the terminal, else stdin when it is a pipe. Never from an argument. Writes the ciphertext and the manifest, then `chezmoi add`s both; commit and push the chezmoi source yourself.\n\nExamples:\n  vega secrets set artificial-analysis --env ARTIFICIAL_ANALYSIS_KEY --for lanista --op 'op://Private/Artificial Analysis/credential'\n  vega secrets set artificial-analysis           # rotate: re-read from 1Password"
    )]
    Set {
        name: String,
        /// The env var the value is injected as
        #[arg(long)]
        env: Option<String>,
        /// Tools that may receive it, comma-separated
        #[arg(long = "for", value_name = "TOOLS", value_delimiter = ',')]
        tools: Option<Vec<String>>,
        /// Hosts that may decrypt it, comma-separated (admin hosts always can)
        #[arg(long, value_delimiter = ',')]
        hosts: Option<Vec<String>>,
        /// 1Password reference to read the value from (op://vault/item/field)
        #[arg(long = "op", value_name = "REF")]
        op_ref: Option<String>,
    },
    /// List secret-like variables in a checkout's .envrc and .env files (names only)
    #[command(
        after_help = "Shows each variable's name, how the file holds it (literal, op, ref, empty), whether git tracks the file and whether vega secrets already has it for the tool. Never a value.\n\nExamples:\n  vega secrets scan                      # this checkout, tool = directory name\n  vega secrets scan ~/src/x --for x"
    )]
    Scan {
        /// The checkout to scan (default: the current directory)
        dir: Option<PathBuf>,
        /// The tool (direnv consumer) name; default: the directory name
        #[arg(long = "for", value_name = "TOOL")]
        tool: Option<String>,
        /// How many directory levels to descend
        #[arg(long, default_value_t = 3)]
        depth: usize,
    },
    /// Move variables from an env file into vega secrets for one tool (admin host only)
    #[command(
        after_help = "For each VAR: reads the value from the file (or its op:// ref) without printing it. Same value as an existing secret: adds the tool to its `for` (under another env name also an alias line in the .envrc). Otherwise a new secret <name> (or <name>-<tool>) backed by the 1Password item <name>-api, created when missing (API Credential, tag vega). Then removes the lines from the file and puts `use vega <tool>` into the .envrc next to it; the Slayer runs `direnv allow` (trust prompt). One adopt at a time (lock).\n\nExamples:\n  vega secrets adopt .envrc OPENAI_API_KEY ZAI_API_KEY --for spellkave\n  vega secrets adopt .env CI_TOKEN --for sidekick --name github-ci"
    )]
    Adopt {
        /// The .envrc or .env file holding the variables
        file: PathBuf,
        /// The variables to adopt
        #[arg(required = true, num_args = 1..)]
        vars: Vec<String>,
        /// The tool (direnv consumer) that gets them: `use vega <tool>`
        #[arg(long = "for", value_name = "TOOL")]
        tool: String,
        /// Secret name instead of the derived one (one VAR only)
        #[arg(long)]
        name: Option<String>,
        /// 1Password vault for new items
        #[arg(long, default_value = "Private")]
        vault: String,
    },
    /// Re-encrypt every secret to its current hosts (admin host only)
    Rekey,
    /// This host's age identity, and adding hosts to the manifest
    Host {
        #[command(subcommand)]
        cmd: HostCmd,
    },
}

#[derive(Subcommand)]
enum HostCmd {
    /// Create this host's identity (idempotent) and print its public key
    #[command(
        after_help = "Examples:\n  vega secrets host init           # a new host; prints the line for the admin host\n  vega secrets host init --admin   # the first host: start the manifest with it as admin"
    )]
    Init {
        /// Start a new manifest with this host as admin
        #[arg(long)]
        admin: bool,
    },
    /// Add a host's public key to the manifest (admin host only)
    #[command(
        after_help = "Examples:\n  vega secrets host add ganymede age1... --secret titan-dolt"
    )]
    Add {
        host: String,
        /// The host's public key, from `vega secrets host init` on it
        recipient: String,
        /// Also let the host decrypt this secret (repeatable); rekeys it
        #[arg(long = "secret")]
        secrets: Vec<String>,
    },
}

const SKILL: &str = include_str!("../SKILL.md");

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
            print_failure(&Error::new(
                "usage",
                message.trim_start_matches("error: "),
                "valid usage below",
            ));
            let mut cmd = Cli::command();
            cmd.build();
            let sub = env::args().nth(1).filter(|a| !a.starts_with('-'));
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
        print!("{SKILL}");
        return ExitCode::SUCCESS;
    }
    let result = Store::from_env().and_then(|store| match cli.cmd {
        None => home(&store, cli.op),
        Some(Cmd::Run { tool, cmd }) => run(&store, &tool, &cmd),
        Some(Cmd::Set {
            name,
            env,
            tools,
            hosts,
            op_ref,
        }) => set(&store, &name, env, tools, hosts, op_ref),
        Some(Cmd::Scan { dir, tool, depth }) => scan(&store, dir, tool, depth),
        Some(Cmd::Adopt {
            file,
            vars,
            tool,
            name,
            vault,
        }) => adopt_cmd(&store, &file, &vars, &tool, name.as_deref(), vault),
        Some(Cmd::Rekey) => rekey(&store),
        Some(Cmd::Host {
            cmd: HostCmd::Init { admin },
        }) => host_init(&store, admin),
        Some(Cmd::Host {
            cmd:
                HostCmd::Add {
                    host,
                    recipient,
                    secrets,
                },
        }) => host_add(&store, &host, &recipient, &secrets),
    });
    match result {
        Ok(code) => code,
        Err(f) => {
            print_failure(&f);
            ExitCode::from(1)
        }
    }
}

fn print_failure(f: &Error) {
    println!("error{{code,message,hint}}:");
    println!("  {},{},{}", f.code, cell(&f.message), cell(&f.hint));
}

fn home(store: &Store, op: bool) -> Result<ExitCode, Error> {
    let m = store.manifest()?;
    let admin = store.is_admin(&m);
    let identity = match store.identity() {
        Ok(_) => "ok",
        Err(e) if e.code == "no_identity" => "missing",
        Err(_) => "bad",
    };
    println!("description: Secrets for tools and agents; values are never printed");
    println!(
        "host: {}{}  identity: {identity}",
        store.host,
        if admin { " (admin)" } else { "" }
    );
    let entries = m.entries();
    let manifest = tilde(&store.manifest_path());
    if !store.manifest_path().exists() {
        println!("manifest: none ({manifest})");
    } else {
        println!("manifest: {manifest}");
    }
    let rows = rows(store, &m, &entries);
    if rows.is_empty() {
        println!("secrets: none");
    } else {
        println!(
            "secrets[{}]{{name,env,for,hosts,here,rotated}}:",
            rows.len()
        );
        for (r, _) in &rows {
            println!("  {r}");
        }
    }
    if op {
        op_compare(admin, &entries)?;
    }
    println!("help: {}", home_hint(store, &m, identity, &rows));
    Ok(ExitCode::SUCCESS)
}

fn rows(store: &Store, m: &Manifest, entries: &[Entry]) -> Vec<(String, Here)> {
    entries
        .iter()
        .map(|e| {
            let here = store.here(m, e);
            let row = format!(
                "{},{},{},{},{},{}",
                e.name,
                cell(&e.env),
                cell(&join(&e.tools)),
                cell(&join(&m.recipients_of(e))),
                here.label(),
                e.rotated.as_deref().unwrap_or("-")
            );
            (row, here)
        })
        .collect()
}

fn home_hint(store: &Store, m: &Manifest, identity: &str, rows: &[(String, Here)]) -> String {
    let admin = store.is_admin(m);
    if identity == "missing" {
        return if m.admin().is_empty() {
            "vega secrets host init --admin   # the Slayer, on the admin host (proteus)".into()
        } else {
            "vega secrets host init   # then the Slayer adds this host on the admin host".into()
        };
    }
    if m.entries().is_empty() {
        return "the Slayer: vega secrets set <name> --env <VAR> --for <tool> --op <op://...>"
            .into();
    }
    let bad = |h: Here| rows.iter().any(|(_, x)| *x == h);
    match (admin, bad(Here::Missing), bad(Here::Undecryptable)) {
        (true, true, _) => "the Slayer: vega secrets set <name>   # for each missing one".into(),
        (true, _, true) => "the Slayer: vega secrets rekey".into(),
        (false, true, _) | (false, _, true) => {
            "chezmoi update; if it persists, ask the Slayer to set or rekey on the admin host"
                .into()
        }
        _ => "vega secrets run --for <tool> -- <cmd>".into(),
    }
}

/// Lists 1Password items tagged `vega` (metadata only) against the manifest's
/// op refs.
fn op_compare(admin: bool, entries: &[Entry]) -> Result<(), Error> {
    if !admin {
        return Err(Error::new(
            "not_admin",
            "--op runs on the admin host only",
            "vega secrets   # without --op",
        ));
    }
    let out = Command::new("op")
        .args(["item", "list", "--tags", "vega", "--format", "json"])
        .stderr(Stdio::inherit())
        .output()
        .map_err(|_| {
            Error::new(
                "op_missing",
                "op is not installed",
                "brew install 1password-cli",
            )
        })?;
    if !out.status.success() {
        return Err(Error::new(
            "op_failed",
            "op item list failed (see stderr)",
            "op signin, then retry",
        ));
    }
    let items = op_titles(&String::from_utf8_lossy(&out.stdout));
    let refs: Vec<(String, String)> = entries
        .iter()
        .filter_map(|e| Some((e.name.clone(), op_item(e.op.as_deref()?)?)))
        .collect();
    let mut lines = Vec::new();
    for t in &items {
        match refs.iter().find(|(_, i)| i == t) {
            Some((name, _)) => lines.push(format!("{},{name},tracked", cell(t))),
            None => lines.push(format!("{},-,untracked", cell(t))),
        }
    }
    for (name, item) in &refs {
        if !items.contains(item) {
            lines.push(format!("{},{name},not_tagged_or_gone", cell(item)));
        }
    }
    println!("op[{}]{{item,secret,state}}:", lines.len());
    for l in lines {
        println!("  {l}");
    }
    Ok(())
}

/// Item titles from `op item list --format json`, without a JSON crate: each
/// item object carries `"title": "..."`.
fn op_titles(json: &str) -> Vec<String> {
    let mut v = Vec::new();
    let mut rest = json;
    while let Some(i) = rest.find("\"title\"") {
        rest = &rest[i + 7..];
        let Some(q) = rest.find('"') else { break };
        rest = &rest[q + 1..];
        let mut title = String::new();
        let mut chars = rest.char_indices();
        let mut end = rest.len();
        while let Some((j, c)) = chars.next() {
            match c {
                '\\' => {
                    if let Some((_, n)) = chars.next() {
                        title.push(n);
                    }
                }
                '"' => {
                    end = j + 1;
                    break;
                }
                c => title.push(c),
            }
        }
        rest = &rest[end..];
        v.push(title);
    }
    v
}

/// The item part of `op://vault/item/field`.
fn op_item(r: &str) -> Option<String> {
    let mut parts = r.strip_prefix("op://")?.split('/');
    parts.next()?;
    parts.next().map(str::to_owned)
}

fn run(store: &Store, tool: &str, cmd: &[String]) -> Result<ExitCode, Error> {
    let (vars, skipped) = store.env_for(tool)?;
    for e in &skipped {
        eprintln!(
            "vega secrets: skipped ({}): {}; {}",
            e.code, e.message, e.hint
        );
    }
    let mut c = Command::new(&cmd[0]);
    c.args(&cmd[1..]);
    for (k, v) in &vars {
        c.env(k, v.expose());
    }
    let err = c.exec();
    Err(Error::new(
        "exec_failed",
        format!("{}: {err}", cmd[0]),
        "check the command name and PATH",
    ))
}

fn scan(
    store: &Store,
    dir: Option<PathBuf>,
    tool: Option<String>,
    depth: usize,
) -> Result<ExitCode, Error> {
    let dir = dir.unwrap_or_else(|| PathBuf::from("."));
    let dir = dir.canonicalize().map_err(|e| {
        Error::new(
            "bad_dir",
            format!("{}: {e}", dir.display()),
            "pass an existing directory",
        )
    })?;
    let tool = tool.unwrap_or_else(|| default_tool(&dir));
    let entries = store.manifest()?.entries();
    let (mut rows, mut uses, mut todo, mut leaked) = (Vec::new(), Vec::new(), false, false);
    for f in adopt::env_files(&dir, depth) {
        let text = fs::read_to_string(&f).unwrap_or_default();
        let rel = f.strip_prefix(&dir).unwrap_or(&f).display().to_string();
        if f.file_name().is_some_and(|n| n == ".envrc") && adopt::has_use_vega(&text, &tool) {
            uses.push(rel.clone());
        }
        let git = if tracked(&f) { "tracked" } else { "no" };
        for v in adopt::scan_text(&f, &text) {
            let in_vega = entries
                .iter()
                .any(|e| e.env == v.name && e.tools.contains(&tool));
            // In vega already: adopt still removes the copy from the file.
            todo |= matches!(v.class, adopt::Class::Literal | adopt::Class::Op);
            leaked |= v.class == adopt::Class::Literal && git == "tracked";
            rows.push(format!(
                "{},{},{},{git},{}",
                cell(&rel),
                v.name,
                v.class.label(),
                if in_vega { "yes" } else { "no" }
            ));
        }
    }
    println!("dir: {}  tool: {tool}", tilde(&dir));
    if rows.is_empty() {
        println!("vars: none");
    } else {
        println!("vars[{}]{{file,var,value,git,vega}}:", rows.len());
        for r in &rows {
            println!("  {r}");
        }
    }
    println!(
        "use_vega: {}",
        if uses.is_empty() {
            "none".into()
        } else {
            join(&uses)
        }
    );
    if leaked {
        println!("warning: a tracked file holds a literal; git history keeps it, ask the Slayer to rotate it");
    }
    if todo {
        println!("help: vega secrets adopt <file> <VAR>... --for {tool}   # on the admin host");
    } else {
        println!("help: nothing to adopt");
    }
    Ok(ExitCode::SUCCESS)
}

/// The default tool name for a checkout: its directory name, lowercased.
fn default_tool(dir: &Path) -> String {
    dir.file_name()
        .map(|n| n.to_string_lossy().to_lowercase())
        .unwrap_or_default()
}

fn tracked(f: &Path) -> bool {
    let (Some(dir), Some(name)) = (f.parent(), f.file_name()) else {
        return false;
    };
    Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["ls-files", "--error-unmatch", "--"])
        .arg(name)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// The raw value of the last assignment of `var` (the one the shell keeps).
fn find_raw<'a>(text: &'a str, var: &str) -> Option<&'a str> {
    text.lines()
        .filter(|l| !l.trim_start().starts_with('#'))
        .filter_map(adopt::parse_line)
        .filter(|(n, _)| *n == var)
        .map(|(_, v)| v)
        .next_back()
}

/// One variable's value and, when the file read it from 1Password, its ref.
fn file_value(
    op: &adopt::Op,
    text: &str,
    var: &str,
    origin: &str,
) -> Result<(Secret, Option<String>), Error> {
    let raw = find_raw(text, var).ok_or_else(|| {
        Error::new(
            "not_found",
            format!("{var} is not assigned in {origin}"),
            "vega secrets scan   # lists the variables",
        )
    })?;
    match adopt::classify(raw) {
        adopt::Class::Literal => Ok((Secret::new(adopt::unquote(raw)), None)),
        adopt::Class::Op => {
            let r = adopt::op_ref(raw).ok_or_else(|| {
                Error::new(
                    "not_literal",
                    format!("{var} runs op without an op:// ref"),
                    "adopt it by hand: vega secrets set",
                )
            })?;
            Ok((adopt::Vault::read(op, &r)?, Some(r)))
        }
        c => Err(Error::new(
            "not_literal",
            format!("{var} is {} in {origin}, not a value", c.label()),
            "nothing to adopt; leave the line or remove it by hand",
        )),
    }
}

fn adopt_cmd(
    store: &Store,
    file: &Path,
    vars: &[String],
    tool: &str,
    name: Option<&str>,
    vault: String,
) -> Result<ExitCode, Error> {
    if name.is_some() && vars.len() != 1 {
        return Err(Error::new(
            "usage",
            "--name takes exactly one VAR",
            "vega secrets adopt <file> <VAR> --for <tool> --name <name>",
        ));
    }
    if !valid_name(tool) {
        return Err(Error::new(
            "bad_tool",
            format!("{tool}: lowercase letters, digits and hyphens only"),
            "e.g. --for spellkave",
        ));
    }
    let bad_file = |e: std::io::Error| {
        Error::new(
            "bad_file",
            format!("{}: {e}", file.display()),
            "vega secrets scan   # lists the env files",
        )
    };
    let file = file.canonicalize().map_err(bad_file)?;
    let _lock = adopt::Lock::acquire(store)?;
    let mut m = store.manifest()?;
    require_admin(store, &m)?;
    let text = fs::read_to_string(&file).map_err(bad_file)?;
    let op = adopt::Op { vault };
    let origin = tilde(&file);
    let (mut done, mut failed) = (Vec::new(), None);
    for var in vars {
        let res = file_value(&op, &text, var, &origin).and_then(|(value, op_ref)| {
            adopt::adopt_value(store, &mut m, &op, var, tool, &value, name, op_ref, &origin)
        });
        match res {
            Ok(a) => {
                store.save_manifest(&m)?;
                done.push(a);
            }
            Err(e) => {
                failed = Some(e);
                break;
            }
        }
    }
    if !done.is_empty() {
        let names: Vec<String> = done.iter().map(|a| a.var.clone()).collect();
        // A value vega holds under another env name: the .envrc maps it.
        let aliases: Vec<String> = done.iter().filter_map(adopt::Adopted::alias).collect();
        let with_aliases = if aliases.is_empty() {
            String::new()
        } else {
            format!(
                "; alias {}",
                done.iter()
                    .filter(|a| a.env.is_some())
                    .map(|a| a.var.as_str())
                    .collect::<Vec<_>>()
                    .join("+")
            )
        };
        let envrc = file.file_name().is_some_and(|n| n == ".envrc");
        adopt::write_keep_mode(
            &file,
            &adopt::rewrite(&text, &names, envrc.then_some(tool), &aliases),
        )?;
        let mut files = vec![format!(
            "{},removed {}{}",
            cell(&origin),
            names.join("+"),
            if envrc {
                format!("; use vega {tool}{with_aliases}")
            } else {
                String::new()
            }
        )];
        let dir = file.parent().unwrap_or(Path::new("."));
        if !envrc {
            let rc = dir.join(".envrc");
            let old = fs::read_to_string(&rc).unwrap_or_default();
            let new = adopt::rewrite(&old, &[], Some(tool), &aliases);
            if new != old {
                adopt::write_keep_mode(&rc, &new)?;
                files.push(format!(
                    "{},use vega {tool}{with_aliases}",
                    cell(&tilde(&rc))
                ));
            }
        }
        println!("adopted[{}]{{var,secret,state,for}}:", done.len());
        for a in &done {
            println!("  {},{},{},{}", a.var, a.secret, a.state, join(&a.tools));
        }
        println!("files[{}]{{file,change}}:", files.len());
        for f in &files {
            println!("  {f}");
        }
        let mut paths: Vec<PathBuf> = done.iter().map(|a| store.secret_path(&a.secret)).collect();
        paths.dedup();
        paths.push(store.manifest_path());
        finish(&paths);
        println!(
            "next: the Slayer runs direnv allow {}   # trust prompt",
            tilde(dir)
        );
    }
    match failed {
        Some(e) => Err(e),
        None => Ok(ExitCode::SUCCESS),
    }
}

fn require_admin(store: &Store, m: &Manifest) -> Result<(), Error> {
    if store.is_admin(m) {
        return Ok(());
    }
    let admins = m.admin();
    Err(Error::new(
        "not_admin",
        format!(
            "{} is not an admin host (admin: {})",
            store.host,
            if admins.is_empty() {
                "none".into()
            } else {
                join(&admins)
            }
        ),
        if admins.is_empty() {
            "vega secrets host init --admin   # on the admin host".to_string()
        } else {
            format!("run it on {}", admins[0])
        },
    ))
}

fn set(
    store: &Store,
    name: &str,
    env: Option<String>,
    tools: Option<Vec<String>>,
    hosts: Option<Vec<String>>,
    op_ref: Option<String>,
) -> Result<ExitCode, Error> {
    let mut m = store.manifest()?;
    require_admin(store, &m)?;
    if !valid_name(name) {
        return Err(Error::new(
            "bad_name",
            format!("{name}: lowercase letters, digits and hyphens only"),
            "e.g. vega secrets set titan-dolt",
        ));
    }
    let existing = m.entry(name);
    let mut e = existing.clone().unwrap_or_else(|| Entry {
        name: name.into(),
        ..Entry::default()
    });
    if let Some(v) = env {
        e.env = v;
    }
    if let Some(v) = tools {
        e.tools = v;
    }
    if let Some(v) = hosts {
        e.hosts = v;
    }
    if op_ref.is_some() {
        e.op = op_ref;
    }
    if e.env.is_empty() {
        return Err(Error::new(
            "incomplete",
            format!("{name} is new: it needs --env (and --for, or no tool gets it yet)"),
            format!("vega secrets set {name} --env <VAR> --for <tool> [--op op://...]"),
        ));
    }
    let value = read_value(&e)?;
    e.encrypted_to = store.encrypt(&m, &e, &value)?;
    e.rotated = Some(vega_secrets::today());
    m.set_entry(&e);
    store.save_manifest(&m)?;
    println!(
        "{}: {name} env={} for={} hosts={}",
        if existing.is_some() { "rotated" } else { "set" },
        e.env,
        join(&e.tools),
        join(&e.encrypted_to)
    );
    finish(&[store.secret_path(name), store.manifest_path()]);
    Ok(ExitCode::SUCCESS)
}

/// The value: `op read`, else a hidden prompt, else a piped stdin.
fn read_value(e: &Entry) -> Result<Secret, Error> {
    let raw = if let Some(r) = &e.op {
        let out = Command::new("op")
            .args(["read", "--no-newline", r])
            .stdin(Stdio::inherit())
            .stderr(Stdio::inherit())
            .output()
            .map_err(|_| {
                Error::new(
                    "op_missing",
                    "op is not installed",
                    "brew install 1password-cli",
                )
            })?;
        if !out.status.success() {
            return Err(Error::new(
                "op_failed",
                format!("op read {r} failed (see stderr)"),
                "op signin; check the reference with op item get",
            ));
        }
        String::from_utf8(out.stdout)
            .map_err(|_| Error::new("not_utf8", "the op value is not UTF-8", "check the item"))?
    } else if std::io::stdin().is_terminal() {
        rpassword::prompt_password(format!("value for {} (hidden): ", e.name)).map_err(|_| {
            Error::new(
                "no_tty",
                "cannot read a hidden value from the terminal",
                "pipe it in, or pass --op op://...",
            )
        })?
    } else {
        let mut s = String::new();
        std::io::stdin()
            .read_to_string(&mut s)
            .map_err(|_| Error::new("no_tty", "stdin is not readable", "pass --op op://..."))?;
        s
    };
    let value = Secret::new(raw);
    let trimmed = value.expose().trim_end_matches(['\n', '\r']);
    if trimmed.is_empty() {
        return Err(Error::new(
            "empty_value",
            format!("the value for {} is empty", e.name),
            "check the 1Password item or what was typed",
        ));
    }
    Ok(Secret::new(trimmed.to_string()))
}

fn rekey(store: &Store) -> Result<ExitCode, Error> {
    let mut m = store.manifest()?;
    require_admin(store, &m)?;
    let names: Vec<String> = m.entries().into_iter().map(|e| e.name).collect();
    let rows = rekey_names(store, &mut m, &names)?;
    store.save_manifest(&m)?;
    print_rekeyed(&rows);
    finish(&changed_paths(store, &rows));
    Ok(ExitCode::SUCCESS)
}

/// Re-encrypts the named secrets to their current recipients; returns
/// `(name, state)` rows (`rekeyed host+host` or `missing`).
fn rekey_names(
    store: &Store,
    m: &mut Manifest,
    names: &[String],
) -> Result<Vec<(String, String)>, Error> {
    let mut rows = Vec::new();
    for name in names {
        let Some(mut e) = m.entry(name) else {
            return Err(Error::new(
                "unknown_secret",
                format!("no secret {name} in the manifest"),
                "vega secrets   # lists them",
            ));
        };
        if !store.secret_path(name).exists() {
            rows.push((name.clone(), "missing".into()));
            continue;
        }
        let value = store.decrypt(name)?;
        e.encrypted_to = store.encrypt(m, &e, &value)?;
        m.set_entry(&e);
        rows.push((name.clone(), format!("rekeyed {}", join(&e.encrypted_to))));
    }
    Ok(rows)
}

fn print_rekeyed(rows: &[(String, String)]) {
    println!("secrets[{}]{{name,state}}:", rows.len());
    for (n, s) in rows {
        println!("  {n},{s}");
    }
}

fn changed_paths(store: &Store, rows: &[(String, String)]) -> Vec<PathBuf> {
    rows.iter()
        .filter(|(_, s)| s.starts_with("rekeyed"))
        .map(|(n, _)| store.secret_path(n))
        .chain(std::iter::once(store.manifest_path()))
        .collect()
}

fn host_init(store: &Store, admin: bool) -> Result<ExitCode, Error> {
    let mut m = store.manifest()?;
    if admin && !m.admin().is_empty() && !store.is_admin(&m) {
        return Err(Error::new(
            "admin_exists",
            format!("the manifest already has admin {}", join(&m.admin())),
            "vega secrets host init   # without --admin",
        ));
    }
    let (public, created) = store.host_init()?;
    println!(
        "identity: {} {}",
        if created { "created" } else { "exists" },
        tilde(&store.key_path())
    );
    println!("public: {public}");
    let known = m
        .hosts()
        .iter()
        .any(|(h, r)| *h == store.host && *r == public);
    if admin || store.is_admin(&m) {
        if !store.is_admin(&m) {
            let mut a = m.admin();
            a.push(store.host.clone());
            m.set_admin(&a);
        }
        if !known {
            m.set_host(&store.host, &public);
        }
        store.save_manifest(&m)?;
        println!("manifest: {} admin, recipient recorded", store.host);
        finish(&[store.manifest_path()]);
    } else if known {
        println!("manifest: recipient recorded");
        println!("help: vega secrets");
    } else {
        println!(
            "help: the Slayer, on the admin host: vega secrets host add {} {public} --secret <name>",
            store.host
        );
    }
    Ok(ExitCode::SUCCESS)
}

fn host_add(
    store: &Store,
    host: &str,
    recipient: &str,
    secrets: &[String],
) -> Result<ExitCode, Error> {
    let mut m = store.manifest()?;
    require_admin(store, &m)?;
    if !vega_secrets::valid_recipient(recipient) {
        return Err(Error::new(
            "bad_recipient",
            "not an age X25519 public key (age1...)",
            format!("copy the `public:` line of vega secrets host init on {host}"),
        ));
    }
    m.set_host(host, recipient);
    for name in secrets {
        let mut e = m.entry(name).ok_or_else(|| {
            Error::new(
                "unknown_secret",
                format!("no secret {name} in the manifest"),
                "vega secrets   # lists them",
            )
        })?;
        if !e.hosts.iter().any(|h| h == host) {
            e.hosts.push(host.to_string());
            m.set_entry(&e);
        }
    }
    let rows = rekey_names(store, &mut m, secrets)?;
    store.save_manifest(&m)?;
    println!("host: {host} added");
    print_rekeyed(&rows);
    finish(&changed_paths(store, &rows));
    Ok(ExitCode::SUCCESS)
}

/// Copies changed files into the chezmoi source (`chezmoi add`; ciphertext
/// stays ciphertext) and prints the commit hint.
fn finish(paths: &[PathBuf]) {
    let ok = Command::new("chezmoi")
        .arg("add")
        .args(paths)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .status()
        .is_ok_and(|s| s.success());
    if ok {
        println!("chezmoi: added {}", paths.len());
        println!("help: commit and push the chezmoi source (chezmoi git -- status)");
    } else {
        println!("chezmoi: not added");
        println!(
            "help: chezmoi add {}; then commit and push the chezmoi source",
            paths.iter().map(|p| tilde(p)).collect::<Vec<_>>().join(" ")
        );
    }
}

fn join(v: &[String]) -> String {
    if v.is_empty() {
        "-".into()
    } else {
        v.join("+")
    }
}

fn tilde(path: &Path) -> String {
    let p = path.display().to_string();
    match env::var("HOME") {
        Ok(home) if !home.is_empty() && p.starts_with(&home) => format!("~{}", &p[home.len()..]),
        _ => p,
    }
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
        vega_plugin::assert_guide(SKILL, "secrets");
    }

    #[test]
    fn op_titles_and_items() {
        let json = r#"[{"id":"a","title":"Artificial Analysis","tags":["vega"]},{"id":"b","title":"titan \"dolt\""}]"#;
        assert_eq!(op_titles(json), ["Artificial Analysis", "titan \"dolt\""]);
        assert_eq!(op_titles("[]"), Vec::<String>::new());
        assert_eq!(
            op_item("op://Private/Artificial Analysis/credential").as_deref(),
            Some("Artificial Analysis")
        );
        assert_eq!(op_item("Private/x"), None);
    }

    #[test]
    fn rows_show_state_without_values() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store {
            config: dir.path().join("c"),
            state: dir.path().join("s"),
            host: "proteus".into(),
        };
        let (pk, _) = store.host_init().unwrap();
        let mut m = Manifest::default();
        m.set_admin(&["proteus".into()]);
        m.set_host("proteus", &pk);
        let mut e = Entry {
            name: "aa".into(),
            env: "ARTIFICIAL_ANALYSIS_KEY".into(),
            tools: vec!["lanista".into()],
            ..Entry::default()
        };
        m.set_entry(&e);
        let r = rows(&store, &m, &m.entries());
        assert_eq!(
            r[0].0,
            "aa,ARTIFICIAL_ANALYSIS_KEY,lanista,proteus,missing,-"
        );
        e.encrypted_to = store
            .encrypt(&m, &e, &Secret::new("sekrit".into()))
            .unwrap();
        e.rotated = Some("2026-10-11".into());
        m.set_entry(&e);
        let r = rows(&store, &m, &m.entries());
        assert_eq!(
            r[0].0,
            "aa,ARTIFICIAL_ANALYSIS_KEY,lanista,proteus,ok,2026-10-11"
        );
        assert!(!r[0].0.contains("sekrit"));
        assert_eq!(
            home_hint(&store, &m, "ok", &r),
            "vega secrets run --for <tool> -- <cmd>"
        );
    }
}
