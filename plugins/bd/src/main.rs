//! `vega bd`: beads sync (docs/design/beads-sync.md).
//!
//! `vega bd init` provisions the current project: bd in server mode, stealth,
//! cloned from its database on the configured Dolt remote, and registered for
//! vegad, which pulls on an interval and pushes after local writes.
//! `vega bd status` shows the sync state vegad recorded; `vega bd resume`
//! restarts a project that stopped on a conflict. Status also shows vegad's
//! git sync of the same projects (docs/design/git-sync.md).
//! Endpoints: `VEGA_BD_REMOTE` or `~/.config/vega/bd.remote`; SQL creation uses
//! `VEGA_BD_SQL` or `~/.config/vega/bd.sql`. Environment wins; no compiled endpoint.

use std::env;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::time::Duration;

use clap::error::ErrorKind;
use clap::{CommandFactory, Parser, Subcommand};
use vega_bd_sync::adopt::{self, Mode, Plan, PushError};
use vega_bd_sync::bd::{self, Create, Probe};
use vega_bd_sync::store::{self, Project};
use vega_bd_sync::{doctor, git, remote_base, sql_endpoint, unix_now, SQL_USER};
use vega_herdr::{cell, collapse_home, print_failure, Failure};

const SKILL: &str = include_str!("../SKILL.md");

#[derive(Parser)]
#[command(
    name = "vega bd",
    bin_name = "vega bd",
    disable_version_flag = true,
    about = "Beads sync: provision a project's bd database on the Dolt remote; vegad keeps it synced",
    after_help = "Without a command: the same as `vega bd status`.\n\nExamples:\n  vega bd init                 # provision this project\n  vega bd status               # sync state of every provisioned project\n  vega bd resume               # after resolving a conflict\n  vega bd resume --project VEGA   # by unique database or project ID; use a local path for multiple clones"
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
    /// Provision this project: bd server mode, stealth, cloned from the remote, synced by vegad
    #[command(
        after_help = "Defaults come from the repo name (the git common dir): database REPO (upper case), prefix repo.\nThe remote is VEGA_BD_REMOTE or ~/.config/vega/bd.remote (required); the URL is <remote>/<database>.\nA missing database is created over SQL (VEGA_BD_SQL or ~/.config/vega/bd.sql (required for creation)) with the vega secret titan-dolt.\nIdempotent: a project already provisioned with the same remote is only registered again.\n--adopt: move this project's existing local database (embedded or server mode) to the remote; it keeps its prefix and history, .beads is backed up under $XDG_STATE_HOME/vega/bd/backup.\n\nExamples:\n  vega bd init\n  vega bd init --database SPELLKAVE --prefix sk\n  vega bd init --adopt              # existing beads → the remote, server mode"
    )]
    Init {
        /// Issue prefix [default: the repo name]
        #[arg(long)]
        prefix: Option<String>,
        /// Dolt database (ASCII letters, digits, underscores, hyphens) [default: repo name upper case, other characters become underscores]
        #[arg(long)]
        database: Option<String>,
        /// Move this project's existing local beads database (embedded or server mode) to the remote
        #[arg(long)]
        adopt: bool,
    },
    /// Sync state of every provisioned project on this host, local checks, and git sync
    #[command(
        after_help = "Tables: projects (beads sync), problems, doctor, git (vegad's fetch and fast-forward of the default branch per repo), git_problems, worktrees_behind.\n\nExamples:\n  vega bd status"
    )]
    Status,
    /// Sync a stopped project again (after resolving its conflict)
    Resume {
        /// A unique project ID/database, or a local location from `vega bd status`; `~` expanded, a trailing `/`, the dir without `.git`/`.bare` or a checkout dir work too [default: this checkout's]
        #[arg(long)]
        project: Option<String>,
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
    let result = match cli.cmd {
        None | Some(Cmd::Status) => status(),
        Some(Cmd::Init {
            prefix,
            database,
            adopt,
        }) => init(prefix, database, adopt),
        Some(Cmd::Resume { project }) => resume(project),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(f) => {
            print_failure(&f);
            ExitCode::from(if f.code == "usage" { 2 } else { 1 })
        }
    }
}

fn git(dir: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .current_dir(dir)
        .args(args)
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (out.status.success() && !s.is_empty()).then_some(s)
}

/// The repo's name from its git common dir: `x/.git`, `x/.bare` → `x`;
/// `x.git` → `x`.
fn repo_name(common: &Path) -> String {
    let last = common
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    if last == ".git" || last == ".bare" {
        common
            .parent()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or(last)
    } else {
        last.trim_end_matches(".git").to_string()
    }
}

/// `spell-kave` → `SPELL_KAVE`: a Dolt database name.
fn default_database(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect()
}

/// `spell-kave` → `spell-kave`, `Foo Bar` → `foobar`: bd's issue prefix.
fn default_prefix(name: &str) -> String {
    name.chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// The real bd: `BD_REAL`, else the first `bd` on PATH outside vega's shim
/// dir. A dotfiles shim found that way is harmless: vegad sets BEADS_ACTOR,
/// which such a shim passes through.
fn real_bd() -> Option<PathBuf> {
    if let Some(p) = env::var_os("BD_REAL").filter(|s| !s.is_empty()) {
        return Some(PathBuf::from(p));
    }
    let shims = env::var_os("HOME").map(|h| Path::new(&h).join(".local/share/vega/shims/bin"));
    env::split_paths(&env::var_os("PATH")?)
        .filter(|d| Some(d) != shims.as_ref())
        .map(|d| d.join("bd"))
        .find(|p| p.is_file())
}

fn which(bin: &str) -> Option<PathBuf> {
    env::split_paths(&env::var_os("PATH")?)
        .map(|d| d.join(bin))
        .find(|p| p.is_file())
}

fn run_bd(bd_bin: &Path, dir: &Path, args: &[&str], secs: u64) -> bd::Run {
    let mut c = Command::new(bd_bin);
    c.current_dir(dir).args(args);
    bd::run(c, Duration::from_secs(secs))
}

/// bd's data dir here (`bd info --json`: database_path), if a beads
/// database resolves from `dir`.
fn data_dir(bd_bin: &Path, dir: &Path) -> Option<PathBuf> {
    Some(PathBuf::from(info(bd_bin, dir)?["database_path"].as_str()?))
}

/// `bd info --json` here.
fn info(bd_bin: &Path, dir: &Path) -> Option<serde_json::Value> {
    let r = run_bd(bd_bin, dir, &["info", "--json"], 30);
    if !r.ok {
        return None;
    }
    let start = r.out.find('{')?;
    serde_json::from_str(&r.out[start..]).ok()
}

/// The URL of bd's `origin` Dolt remote here.
fn origin_url(bd_bin: &Path, dir: &Path) -> Option<String> {
    let r = run_bd(bd_bin, dir, &["dolt", "remote", "list", "--json"], 30);
    let start = r.out.find('[')?;
    let v: serde_json::Value = serde_json::from_str(&r.out[start..]).ok()?;
    v.as_array()?
        .iter()
        .find(|r| r["name"] == "origin")
        .and_then(|r| r["url"].as_str().map(String::from))
}

fn init(prefix: Option<String>, database: Option<String>, adopt: bool) -> Result<(), Failure> {
    let cwd = env::current_dir()
        .map_err(|e| Failure::new("no_cwd", e.to_string(), "cd into a checkout"))?;
    let not_git = || {
        Failure::new(
            "not_a_repo",
            "not inside a git checkout",
            "cd into the project's checkout, then vega bd init",
        )
    };
    let top = PathBuf::from(git(&cwd, &["rev-parse", "--show-toplevel"]).ok_or_else(not_git)?);
    let resolved = vega_herdr::project::resolve(&top, &vega_herdr::state::post_host())?;
    let key = resolved.id.to_string();
    let common = resolved.location.common_dir;
    let bd_bin = real_bd().ok_or_else(|| {
        Failure::new(
            "bd_missing",
            "no bd binary on PATH",
            "install bd (brew install beads), or set BD_REAL",
        )
    })?;
    let name = repo_name(&common);
    let database = database.unwrap_or_else(|| default_database(&name));
    if !bd::valid_database(&database) {
        return Err(Failure::new(
            "usage",
            format!("database `{database}` is not a plain name"),
            "pass --database with ASCII letters, digits, underscores and hyphens",
        ));
    }
    let remote = remote_base();
    if remote.is_empty() {
        return Err(Failure::new(
            "remote_unconfigured",
            "no Dolt remote configured; beads stay local",
            "set VEGA_BD_REMOTE or ~/.config/vega/bd.remote before vega bd init",
        ));
    }
    let url = format!("{remote}/{database}");
    let p = Target {
        key,
        common,
        top,
        bd_bin,
        database,
        url,
    };

    if let Some(data) = data_dir(&p.bd_bin, &p.top) {
        let origin = origin_url(&p.bd_bin, &p.top);
        let server = adopt::local(&data).is_ok_and(|l| l.mode == Mode::Server);
        // Already provisioned here? Then only (re-)register it.
        if origin.as_deref() == Some(p.url.as_str()) && (server || !adopt) {
            register(&p, &data)?;
            println!("provisioned: {} (already; registered for sync)", p.key);
            print_after(&p.database, &p.url, &data);
            return Ok(());
        }
        if !adopt {
            return Err(Failure::new(
                "already_initialized",
                format!(
                    "this project already has a beads database at {} with remote {}",
                    collapse_home(&data.display().to_string()),
                    origin.as_deref().unwrap_or("none")
                ),
                "vega bd init --adopt moves it to the remote: pushes its history, backs up .beads, converts it to server mode (vega skills bd)",
            ));
        }
        return adopt_existing(&p, prefix, data, origin);
    }
    if adopt {
        return Err(Failure::new(
            "nothing_to_adopt",
            "this project has no beads database to adopt",
            "vega bd init provisions it fresh (clones from the remote)",
        ));
    }

    let prefix = prefix.unwrap_or_else(|| default_prefix(&name));
    let dolt = need_dolt()?;
    let created = ensure_database(&dolt, &p.url, &p.database)?;
    bd_init(&p, &prefix).map_err(|m| {
        Failure::new(
            "bd_init_failed",
            m,
            "see bd init's message; bd doctor helps",
        )
    })?;
    let (data, seeded) = finish(&p)?;
    println!("provisioned: {}", p.key);
    if created {
        println!("created: database {} on {}", p.database, sql_endpoint());
    }
    if seeded > 0 {
        println!("seeded: pushed {seeded} commit(s) to the empty remote database");
    }
    print_after(&p.database, &p.url, &data);
    Ok(())
}

/// The project `vega bd init` provisions.
struct Target {
    key: String,
    common: PathBuf,
    top: PathBuf,
    bd_bin: PathBuf,
    database: String,
    url: String,
}

fn need_dolt() -> Result<PathBuf, Failure> {
    which("dolt").ok_or_else(|| {
        Failure::new(
            "dolt_missing",
            "no dolt binary on PATH (needed to check the remote)",
            "install dolt (brew install dolt)",
        )
    })
}

/// Whether a Dolt remote is configured.
fn remote_set() -> bool {
    !remote_base().is_empty()
}

/// The hint of `remote_unreachable`, distinguishing missing configuration.
fn unreachable_hint(set: bool, then: &str) -> String {
    if set {
        format!("check VEGA_BD_REMOTE / ~/.config/vega/bd.remote and the network; {then}")
    } else {
        format!(
            "no Dolt remote configured (VEGA_BD_REMOTE or ~/.config/vega/bd.remote): beads stay local (plain bd works); `vega onboard` lists the options; {then}"
        )
    }
}

fn ensure_database(dolt: &Path, url: &str, database: &str) -> Result<bool, Failure> {
    let scratch = env::temp_dir().join(format!("vega-bd-probe-{}", std::process::id()));
    std::fs::create_dir_all(&scratch)
        .map_err(|e| Failure::new("probe_failed", e.to_string(), "check the temp dir"))?;
    let probe = bd::probe(dolt, url, &scratch);
    let created = match probe {
        Probe::Exists => Ok(false),
        Probe::Missing => create(dolt, database, &scratch).map(|()| true),
        Probe::Unreachable(m) => Err(Failure::new(
            "remote_unreachable",
            format!("{url}: {m}"),
            &unreachable_hint(
                remote_set(),
                "the remote must be reachable for the first clone",
            ),
        )),
        Probe::Unknown(m) => Err(Failure::new(
            "probe_failed",
            format!("{url}: {m}"),
            "check the remote URL (VEGA_BD_REMOTE or ~/.config/vega/bd.remote)",
        )),
    };
    let _ = std::fs::remove_dir_all(&scratch);
    created
}

/// `bd init` in server mode, cloned from the remote; the error is bd's last line.
fn bd_init(p: &Target, prefix: &str) -> Result<(), String> {
    let args = [
        "init",
        "--prefix",
        prefix,
        "--database",
        &p.database,
        "--remote",
        &p.url,
        "--server",
        "--stealth",
        "--skip-agents",
        "--skip-hooks",
        "--non-interactive",
    ];
    let r = run_bd(&p.bd_bin, &p.top, &args, 600);
    if r.ok {
        Ok(())
    } else {
        Err(bd::last_line(&r.out))
    }
}

/// After `bd init`: find the data dir, seed the remote, register for vegad.
fn finish(p: &Target) -> Result<(PathBuf, u64), Failure> {
    let data = data_dir(&p.bd_bin, &p.top).ok_or_else(|| {
        Failure::new(
            "bd_init_failed",
            "bd init succeeded but bd info finds no database",
            "run bd info here and report it",
        )
    })?;
    // An empty remote: this clone just created the schema. Seed the remote
    // now, so the next clone starts from it; two clones that both seed an
    // empty remote end with clashing project identities.
    let seeded = seed(&p.bd_bin, &p.top)?;
    register(p, &data)?;
    Ok((data, seeded))
}

/// `vega bd init --adopt`: moves the existing local database to the remote
/// (docs/design/beads-sync.md, "Adopting an existing database"). Nothing
/// local changes before its history is on the remote and `.beads` is backed
/// up.
fn adopt_existing(
    p: &Target,
    prefix: Option<String>,
    data: PathBuf,
    origin: Option<String>,
) -> Result<(), Failure> {
    let l = adopt::local(&data).map_err(|m| {
        Failure::new(
            "adopt_unsupported",
            m,
            "--adopt moves a local embedded or server-mode database only",
        )
    })?;
    let current = info(&p.bd_bin, &p.top)
        .and_then(|v| v["config"]["issue_prefix"].as_str().map(String::from))
        .ok_or_else(|| {
            Failure::new(
                "adopt_unsupported",
                "bd info shows no issue prefix",
                "run bd info here and report it",
            )
        })?;
    if let Some(want) = prefix.filter(|w| *w != current) {
        return Err(Failure::new(
            "usage",
            format!("--prefix {want}: the database's issue prefix is {current}"),
            "drop --prefix; --adopt keeps the existing prefix",
        ));
    }
    let dolt = need_dolt()?;
    let created = ensure_database(&dolt, &p.url, &p.database)?;
    if l.mode == Mode::Server {
        // The dolt CLI below works on the files; bd's server must not hold them.
        let _ = run_bd(&p.bd_bin, &p.top, &["dolt", "stop"], 60);
    }

    let pushed = adopt::push(&dolt, &l.repo, &p.url).map_err(|e| match e {
        PushError::Diverged { remote_commits } => Failure::new(
            "remote_diverged",
            format!(
                "database {} on {} holds {remote_commits} commit(s) that are not this project's history; nothing was pushed or changed",
                p.database,
                remote_base()
            ),
            "vega never forces onto foreign history: pick another --database, or have the Slayer check that database on the remote",
        ),
        PushError::Unreachable(m) => Failure::new(
            "remote_unreachable",
            format!("{}: {m}", p.url),
            &unreachable_hint(remote_set(), "nothing local was changed"),
        ),
        PushError::Failed(m) => Failure::new(
            "adopt_push_failed",
            m,
            "nothing local was changed; fix the cause and rerun vega bd init --adopt",
        ),
    })?;

    let now = unix_now();
    let backup = store::backup_dir(&p.key, &p.common, now).ok_or_else(|| {
        Failure::new(
            "backup_failed",
            "no state dir for the backup",
            "set HOME or XDG_STATE_HOME",
        )
    })?;
    adopt::backup(&l.beads, &backup).map_err(|m| {
        Failure::new(
            "backup_failed",
            m,
            "nothing local was changed; the history is on the remote already; fix the cause and rerun vega bd init --adopt",
        )
    })?;
    // A bd write after the push would be lost: the backup and the live repo
    // must both hold exactly the pushed head.
    let backup_repo = l
        .repo
        .strip_prefix(&l.beads)
        .map(|r| backup.join(r))
        .unwrap_or_default();
    let heads = [
        adopt::head(&dolt, &backup_repo),
        adopt::head(&dolt, &l.repo),
    ];
    if heads.iter().any(|h| h.as_ref() != Ok(&pushed.head)) {
        let _ = std::fs::remove_dir_all(&backup);
        return Err(Failure::new(
            "beads_changed",
            "a bd write landed during the adoption; nothing local was changed",
            "rerun vega bd init --adopt when no agent writes beads (it pushes the new commits)",
        ));
    }

    let old_config = std::fs::read_to_string(l.beads.join("config.yaml")).unwrap_or_default();
    let backup_shown = collapse_home(&backup.display().to_string());
    let beads_shown = collapse_home(&l.beads.display().to_string());
    // The new .beads moves next to the backup (out of the repo), not away.
    let aside = backup
        .parent()
        .map(|d| collapse_home(&d.join("beads.adopted").display().to_string()))
        .unwrap_or_default();
    let rollback = format!(
        "bd dolt stop; mv {beads_shown} {aside}; cp -pR {backup_shown} {beads_shown} (docs/design/beads-sync.md)"
    );
    adopt::clear(&l).map_err(|m| {
        Failure::new(
            "adopt_clear_failed",
            m,
            &format!("the history is on the remote; roll back: {rollback}"),
        )
    })?;
    bd_init(p, &current).map_err(|m| {
        Failure::new(
            "bd_init_failed",
            m,
            &format!(
                "the history is on the remote: vega bd init clones it; or roll back: {rollback}"
            ),
        )
    })?;
    let (data, seeded) = finish(p)?;

    println!("adopted: {}", p.key);
    println!(
        "from: {} database (previous remote: {})",
        match l.mode {
            Mode::Embedded => "embedded",
            Mode::Server => "server-mode",
        },
        origin.as_deref().unwrap_or("none")
    );
    if created {
        println!("created: database {} on {}", p.database, sql_endpoint());
    }
    println!(
        "pushed: {}",
        match pushed.plan {
            Plan::Present => "nothing; the remote already held this history",
            Plan::Push => "the local history",
            Plan::ForceOntoEmpty =>
                "the local history, forced once onto the empty database (only its initial commit)",
            Plan::Diverged => "nothing",
        }
    );
    if seeded > 0 {
        println!("seeded: pushed {seeded} commit(s) bd init wrote");
    }
    println!("backup: {backup_shown}");
    if adopt::has_settings(&old_config) {
        println!("note: bd init rewrote .beads/config.yaml; the old settings are in the backup's config.yaml");
    }
    print_after(&p.database, &p.url, &data);
    let record = store::project_path(&p.key, &p.common)
        .map(|f| collapse_home(&f.display().to_string()))
        .unwrap_or_default();
    println!("rollback: {rollback}; rm {record}");
    Ok(())
}

/// The vega secret that holds the SQL password of [`SQL_USER`] on the remote.
const SQL_SECRET: &str = "titan-dolt";

/// Creates the missing `database` on the remote's SQL port as the
/// CREATE-only user; the password comes from vega secrets in process.
fn create(dolt: &Path, database: &str, dir: &Path) -> Result<(), Failure> {
    let endpoint = sql_endpoint();
    if endpoint.is_empty() {
        return Err(Failure::new(
            "sql_unconfigured",
            "creating a database needs a SQL endpoint",
            "set VEGA_BD_SQL or ~/.config/vega/bd.sql, then rerun vega bd init",
        ));
    }
    let store = vega_secrets::Store::from_env()
        .map_err(|e| Failure::new("credential_unavailable", e.message, &e.hint))?;
    let password = store.get(SQL_SECRET, "vega-bd").map_err(|e| match e.code {
        "unknown_secret" | "missing" => Failure::new(
            "credential_missing",
            format!("database {database} does not exist on {}; creating it needs the vega secret {SQL_SECRET}, which this host does not have ({}: {})", remote_base(), e.code, e.message),
            &format!("the Slayer runs on proteus: vega secrets set {SQL_SECRET} --env VEGA_TITAN_DOLT_PASSWORD --for vega-bd --hosts proteus,ganymede (commit, push); here: chezmoi update; then rerun vega bd init"),
        ),
        _ => Failure::new(
            "credential_unavailable",
            format!("vega secret {SQL_SECRET}: {} ({})", e.message, e.code),
            &e.hint,
        ),
    })?;
    match bd::create_database(dolt, &endpoint, SQL_USER, password.expose(), database, dir) {
        Create::Ok => Ok(()),
        Create::Denied(m) => Err(Failure::new(
            "create_denied",
            format!("CREATE DATABASE {database} on {endpoint} as {SQL_USER}: {m}"),
            &format!("the secret {SQL_SECRET} or the user's CREATE grant is wrong; the Slayer checks both (orrery orr-rcg)"),
        )),
        Create::Unreachable(m) => Err(Failure::new(
            "create_unreachable",
            format!("{endpoint}: {m}"),
            "check the SQL network access and VEGA_BD_SQL or ~/.config/vega/bd.sql (environment wins; no compiled endpoint)",
        )),
        Create::Failed(m) => Err(Failure::new(
            "create_failed",
            format!("CREATE DATABASE {database} on {endpoint}: {m}"),
            "report it; nothing was changed on the remote",
        )),
    }
}

/// Pushes what `bd init` committed on top of the remote (the schema of a new
/// database); 0 when the clone is even with the remote.
fn seed(bd_bin: &Path, dir: &Path) -> Result<u64, Failure> {
    let r = run_bd(bd_bin, dir, &["sql", "--json", bd::COUNTS_SQL], 60);
    let c = bd::parse_counts(&r.out).ok_or_else(|| {
        Failure::new(
            "bd_init_failed",
            format!(
                "cannot count commits after bd init: {}",
                bd::last_line(&r.out)
            ),
            "run bd dolt status here and report it",
        )
    })?;
    if c.ahead == 0 {
        return Ok(0);
    }
    let r = run_bd(bd_bin, dir, &["dolt", "push"], 600);
    match bd::classify(&r) {
        bd::Outcome::Ok => Ok(c.ahead),
        bd::Outcome::Behind(m) => Err(Failure::new(
            "seed_race",
            format!("another clone seeded the empty remote database first: {m}"),
            "this clone's schema clashes with it: remove this checkout's .beads (it holds no beads yet) and rerun vega bd init",
        )),
        bd::Outcome::Unreachable(m)
        | bd::Outcome::Conflict(m)
        | bd::Outcome::Retry(m)
        | bd::Outcome::Failed(m) => {
            Err(Failure::new(
                "seed_failed",
                format!("bd dolt push of the new schema failed: {m}"),
                "fix the cause, then bd dolt push here before any other clone runs vega bd init",
            ))
        }
    }
}

fn register(t: &Target, data: &Path) -> Result<(), Failure> {
    let previous = store::project(&t.key, &t.common);
    let since = previous.as_ref().map_or(unix_now(), |p| p.since);
    let p = Project {
        key: t.key.clone(),
        common: t.common.clone(),
        dir: t.top.clone(),
        database: t.database.clone(),
        remote: t.url.clone(),
        bd: t.bd_bin.clone(),
        data: data.into(),
        since,
        resumed: previous.map_or(0, |p| p.resumed),
    };
    if store::save_project(&p) {
        Ok(())
    } else {
        Err(Failure::new(
            "write_failed",
            "could not write the project record",
            "check $XDG_STATE_HOME/vega/bd (default ~/.local/state/vega/bd)",
        ))
    }
}

fn sync_line() -> &'static str {
    if vega_herdr::state::relay_local() {
        "sync: off (vegad in local mode, no broker); beads stay local until it reaches one"
    } else if vega_herdr::state::relay_running() {
        "sync: vegad running (pull every 60s, push after local writes)"
    } else {
        "sync: vegad not running; nothing syncs until it runs (vega post relay start)"
    }
}

fn print_after(database: &str, url: &str, data: &Path) {
    println!("database: {database}");
    println!("remote: {url}");
    println!("data: {}", collapse_home(&data.display().to_string()));
    println!("{}", sync_line());
    println!(
        "help: vega bd status   # ahead, behind, last sync; no manual bd dolt pull/push needed"
    );
}

fn status() -> Result<(), Failure> {
    let now = unix_now();
    let bin = env::current_exe()
        .map(|p| collapse_home(&p.display().to_string()))
        .unwrap_or_else(|_| "unknown".into());
    println!("bin: {bin}");
    let remote = remote_base();
    println!(
        "description: beads projects on this host synced by vegad; provisioning remote: {}",
        if remote.is_empty() {
            "unconfigured (provisioned projects keep their recorded remotes)"
        } else {
            &remote
        }
    );
    println!("{}", sync_line());
    let projects = store::projects();
    let here = env::current_dir()
        .ok()
        .and_then(|d| vega_herdr::project::resolve(&d, &vega_herdr::state::post_host()).ok());
    if projects.is_empty() {
        println!("projects: 0 provisioned on this host");
    } else {
        println!("projects[{}]{{{}}}:", projects.len(), store::COLUMNS);
    }
    let mut problems = Vec::new();
    let mut findings = Vec::new();
    for p in &projects {
        let s = store::state(&p.key, &p.common);
        let (cells, problem) = store::row(p, &s, now);
        println!(
            "  {}",
            cells.iter().map(|c| cell(c)).collect::<Vec<_>>().join(",")
        );
        if let Some(m) = problem {
            problems.push((p.key.clone(), m));
        }
        if let Some(b) = p.beads_dir() {
            findings.extend(doctor::check(b).into_iter().map(|f| (p.key.clone(), f)));
        }
    }
    if !problems.is_empty() {
        println!("problems[{}]{{project,problem}}:", problems.len());
        for (k, m) in &problems {
            println!("  {},{}", cell(k), cell(m));
        }
    }
    if !findings.is_empty() {
        println!("doctor[{}]{{project,finding}}:", findings.len());
        for (k, f) in &findings {
            println!("  {},{}", cell(k), cell(f));
        }
    }
    git_status(now);
    let mut help = Vec::new();
    match &here {
        Some(k)
            if !projects
                .iter()
                .any(|p| p.key == k.id.as_str() && p.common == k.location.common_dir) =>
        {
            println!("here: {} not provisioned", k.id);
            help.push("vega bd init                 # provision this project");
        }
        _ => {}
    }
    if projects
        .iter()
        .any(|p| store::state(&p.key, &p.common).is_stopped(p))
    {
        help.push(
            "vega bd resume --project <location>   # use the location column after resolving the conflict",
        );
    }
    if !help.is_empty() {
        println!("help[{}]:", help.len());
        for h in help {
            println!("  {h}");
        }
    }
    Ok(())
}

/// vegad's git sync of the same projects plus the UAC claims
/// (docs/design/git-sync.md): one row per repo, its problems, and the
/// worktrees behind the upstream.
fn git_status(now: i64) {
    let claimed = git::claimed();
    let repos = git::known_repos(claimed);
    if repos.is_empty() {
        return;
    }
    println!("git[{}]{{{}}}:", repos.len(), git::COLUMNS);
    let mut problems = Vec::new();
    let mut lags = Vec::new();
    for r in &repos {
        let s = git::state(&r.key, &r.common);
        let (cells, problem) = git::row(r, &s, now);
        println!(
            "  {}",
            cells.iter().map(|c| cell(c)).collect::<Vec<_>>().join(",")
        );
        if let Some(m) = problem {
            problems.push((r.key.clone(), m));
        }
        for l in s.worktrees.iter().filter(|l| l.behind > 0) {
            lags.push((
                collapse_home(&l.dir.display().to_string()),
                l.branch.clone().unwrap_or_else(|| "(detached)".into()),
                l.behind,
            ));
        }
    }
    if !problems.is_empty() {
        println!("git_problems[{}]{{project,problem}}:", problems.len());
        for (k, m) in &problems {
            println!("  {},{}", cell(k), cell(m));
        }
    }
    if !lags.is_empty() {
        println!(
            "worktrees_behind[{}]{{worktree,branch,behind}}:",
            lags.len()
        );
        for (d, b, n) in &lags {
            println!("  {},{},{n}", cell(d), cell(b));
        }
    }
}

fn resume(project: Option<String>) -> Result<(), Failure> {
    let projects = store::projects();
    let arg = match project {
        Some(arg) => arg,
        None => env::current_dir()
            .map_err(|e| Failure::new("no_cwd", e.to_string(), "pass --project <location>"))?
            .display()
            .to_string(),
    };
    let mut p = find_project(&arg, &projects)?;
    let s = store::state(&p.key, &p.common);
    p.resumed = unix_now();
    if !store::save_project(&p) {
        return Err(Failure::new(
            "write_failed",
            "could not write the project record",
            "check $XDG_STATE_HOME/vega/bd",
        ));
    }
    println!(
        "resumed: {} at {}{}",
        p.key,
        p.common.display(),
        if s.stopped.is_some() {
            ""
        } else {
            " (was not stopped)"
        }
    );
    println!("{}", sync_line());
    println!("help: vega bd status   # vegad syncs within seconds; a conflict left unresolved stops it again");
    Ok(())
}

/// IDs/database names must select a single clone. A local path selects its
/// canonical common directory, including linked worktrees and bare repos.
fn find_project(arg: &str, projects: &[Project]) -> Result<Project, Failure> {
    let arg = arg.trim();
    let path = match (arg.strip_prefix("~/"), env::var_os("HOME")) {
        (Some(rest), Some(home)) => Path::new(&home).join(rest),
        _ => PathBuf::from(arg),
    };
    let exact: Vec<_> = projects
        .iter()
        .filter(|p| p.key == arg || p.database.eq_ignore_ascii_case(arg))
        .collect();
    let matches = if !exact.is_empty() {
        exact
    } else {
        let path = if path.join(".bare").is_dir() {
            path.join(".bare")
        } else {
            path
        };
        let common = if path.exists() {
            Some(
                vega_herdr::project::resolve(&path, &vega_herdr::state::post_host())?
                    .location
                    .common_dir,
            )
        } else {
            None
        };
        projects
            .iter()
            .filter(|p| common.as_ref() == Some(&p.common))
            .collect()
    };
    match matches.as_slice() {
        [p] => Ok((*p).clone()),
        [] => Err(Failure::new(
            "not_provisioned",
            format!("{arg} is not provisioned on this host"),
            "vega bd status lists project IDs and locations",
        )),
        _ => Err(Failure::new(
            "project_location_ambiguous",
            format!("{arg} matches {} local locations", matches.len()),
            "pass --project <location> using a location from vega bd status",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reregister_preserves_since_and_resume_without_changing_state() {
        const FIXTURE: &str = "VEGA_BD_REGISTER_FIXTURE";
        if std::env::var_os(FIXTURE).is_some() {
            let root = PathBuf::from(std::env::var_os("HOME").unwrap());
            let target = Target {
                key: "github.com/team/repo".into(),
                common: root.join("repo/.git"),
                top: root.join("repo"),
                bd_bin: "/nonexistent/bd".into(),
                database: "DB".into(),
                url: "http://remote/DB".into(),
            };
            let data = root.join("repo/.beads/dolt");
            assert!(register(&target, &data).is_ok());
            let mut original = store::project(&target.key, &target.common).unwrap();
            original.since = 17;
            original.resumed = 101;
            assert!(store::save_project(&original));
            let state = store::State {
                stopped: Some(100),
                ..store::State::default()
            };
            assert!(store::save_state(&target.key, &target.common, &state));
            assert!(register(&target, &data).is_ok());
            let registered = store::project(&target.key, &target.common).unwrap();
            assert_eq!(registered.since, 17);
            assert_eq!(registered.resumed, 101);
            assert!(!state.is_stopped(&registered));
            assert_eq!(store::state(&target.key, &target.common), state);
            std::fs::write(root.join("returned"), "registered").unwrap();
            return;
        }
        let root = tempfile::tempdir().unwrap();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "tests::reregister_preserves_since_and_resume_without_changing_state",
                "--nocapture",
            ])
            .env(FIXTURE, "1")
            .env("HOME", root.path())
            .env("XDG_STATE_HOME", root.path().join("state"))
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(root.path().join("returned").is_file());
    }

    #[test]
    fn skill_has_valid_frontmatter() {
        vega_plugin::assert_guide(SKILL, "bd");
    }

    #[test]
    fn unreachable_hint_names_the_remote_config_when_none_is_set() {
        let none = unreachable_hint(false, "nothing local was changed");
        assert!(
            none.contains("no Dolt remote configured (VEGA_BD_REMOTE or ~/.config/vega/bd.remote)"),
            "{none}"
        );
        assert!(
            none.contains("beads stay local") && none.contains("vega onboard"),
            "{none}"
        );
        assert!(none.ends_with("nothing local was changed"));
        assert!(!unreachable_hint(true, "x").contains("no Dolt remote configured"));
    }

    #[test]
    fn names_from_the_common_dir() {
        assert_eq!(repo_name(Path::new("/s/vega/.git")), "vega");
        assert_eq!(repo_name(Path::new("/s/spellkave/.bare")), "spellkave");
        assert_eq!(repo_name(Path::new("/s/x.git")), "x");
        assert_eq!(default_database("rally-ng"), "RALLY_NG");
        assert_eq!(default_prefix("rally-ng"), "rally-ng");
    }
}
