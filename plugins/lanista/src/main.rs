//! `vega lanista`: owns the external lanista CLI (the Slayer's LLM model
//! catalog, github.com/b4arena/lanista): install and upgrade from git, the
//! seats allowlist shipped from this repo, health, and model pins per lane.
//!
//! lanista stays a separate Python tool; vega wraps it like token-burn
//! (crates/vega-herdr/src/capacity.rs): it reads lanista's `--json` output
//! and files, never its internals.

mod install;
mod seats;

use std::env;
use std::path::Path;
use std::process::{Command, ExitCode};
use std::time::Duration;

use clap::error::ErrorKind;
use clap::{CommandFactory, Parser, Subcommand};
use serde_json::Value;
use vega_herdr::output_within;

use install::{Found, Install};
use vega_herdr::catalog;

#[derive(Parser)]
#[command(
    name = "vega lanista",
    bin_name = "vega lanista",
    disable_version_flag = true,
    about = "Install, configure and use lanista, the LLM model catalog",
    after_help = "Examples:\n  vega lanista                    # install, catalog, seats (read-only)\n  vega lanista install            # from git main, seats from the vega repo\n  vega lanista pin xai --tier budget"
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
    /// Install or upgrade lanista from git and write the seats file (idempotent)
    #[command(
        after_help = "Installs with `uv tool install` at the commit the ref names, so it also upgrades. An editable or other install is left alone unless --replace.\n\nExamples:\n  vega lanista install\n  vega lanista install --ref v0.2.0\n  vega lanista install --replace"
    )]
    Install {
        /// Branch, tag or full commit sha of github.com/b4arena/lanista
        #[arg(long, default_value = install::DEFAULT_REF)]
        r#ref: String,
        /// Replace an editable or non-git install
        #[arg(long)]
        replace: bool,
    },
    /// Run lanista's health checks
    Doctor,
    /// Resolve or override the project model for a provider lane and tier
    #[command(
        after_help = include_str!("../help/pin.md")
    )]
    Pin {
        /// Provider lane, as in the seats file (xai, zai, agy, claude, codex)
        provider: String,
        #[arg(long, default_value = "balanced", value_parser = seats::TIERS)]
        tier: String,
        #[arg(long, default_value = seats::DEFAULT_SEAT)]
        seat: String,
        /// Set this project's model override
        #[arg(long, conflicts_with = "clear")]
        set: Option<String>,
        /// Clear this project's override (global seats apply)
        #[arg(long)]
        clear: bool,
        /// Expiry: 30m, 2h, 1d or never (default: binding quota reset)
        #[arg(long, requires = "set")]
        until: Option<String>,
    },
}

const SKILL: &str = include_str!("../SKILL.md");

struct Failure {
    code: String,
    message: String,
    hint: String,
}

impl Failure {
    fn new(code: &str, message: impl Into<String>, hint: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            hint: hint.into(),
        }
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
    let result = match cli.cmd {
        None => home(),
        Some(Cmd::Install { r#ref, replace }) => install_cmd(&r#ref, replace),
        Some(Cmd::Doctor) => doctor(),
        Some(Cmd::Pin {
            provider,
            tier,
            seat,
            set,
            clear,
            until,
        }) => pin(&provider, &tier, &seat, set, clear, until),
    };
    match result {
        Ok(code) => code,
        Err(f) => {
            print_failure(&f);
            ExitCode::from(if f.code == "usage" { 2 } else { 1 })
        }
    }
}

fn print_failure(f: &Failure) {
    println!("error{{code,message,hint}}:");
    println!("  {},{},{}", f.code, cell(&f.message), cell(&f.hint));
}

fn found() -> Result<Found, Failure> {
    let dir = install::tool_dir().map_err(|e| {
        Failure::new(
            "uv_missing",
            e,
            "install uv: https://docs.astral.sh/uv/getting-started/installation/",
        )
    })?;
    let on_path = on_path("lanista");
    Ok(install::detect(&dir, on_path.as_deref()))
}

fn on_path(bin: &str) -> Option<std::path::PathBuf> {
    env::var_os("PATH").and_then(|p| {
        env::split_paths(&p)
            .map(|d| d.join(bin))
            .find(|f| f.is_file())
    })
}

/// One line on the install: `git 03892da (main 03892da, current)`.
fn install_line(f: &Found, git_ref: &str, target: &Result<String, String>) -> String {
    match &f.install {
        Install::Missing => "missing".into(),
        Install::Editable { path } => format!("editable {}", tilde(path)),
        Install::Other { what } => format!("other ({what})"),
        Install::Git { commit } => {
            let have = commit.as_deref().map(install::short).unwrap_or("unknown");
            match (commit, target) {
                (Some(c), Ok(t)) => format!(
                    "git {have} ({} {}, {})",
                    install::ref_label(git_ref),
                    install::short(t),
                    if c == t { "current" } else { "differs" }
                ),
                _ => format!("git {have} ({} unknown)", install::ref_label(git_ref)),
            }
        }
    }
}

fn home() -> Result<ExitCode, Failure> {
    let f = found()?;
    let target = install::resolve(install::DEFAULT_REF);
    println!("bin: {}", tilde(&bin()));
    println!("description: Install, configure and use lanista, the LLM model catalog");
    println!(
        "lanista: {}",
        install_line(&f, install::DEFAULT_REF, &target)
    );
    if let Some(v) = &f.version {
        println!("version: {v}");
    }
    if f.install != Install::Missing {
        println!("catalog: {}", catalog());
        println!(
            "catalog_sync: {}",
            sync_line(
                catalog::State::load(&catalog::state_path()).as_ref(),
                &catalog::builder(),
                &host(),
                vega_bd_sync::unix_now(),
            )
        );
        println!("aa_key: {}", aa_key());
    }
    let path = seats::path();
    let state = seats::state(&path);
    println!(
        "seats: {} {}",
        state.label(),
        tilde(&path.display().to_string())
    );
    let pins = seats::pins(seats::SEATS, seats::DEFAULT_SEAT).unwrap_or_default();
    let lanes = seats::providers(&pins);
    println!(
        "global_pins[{}]{{seat,provider,flagship,balanced,budget}}:",
        lanes.len()
    );
    for prov in &lanes {
        let m = |t| seats::pick(&pins, prov, t).map_or("-", |p| p.model.as_str());
        println!(
            "  {},{prov},{},{},{}",
            seats::DEFAULT_SEAT,
            m("flagship"),
            m("balanced"),
            m("budget")
        );
    }
    let upgrade =
        matches!((&f.install, &target), (Install::Git { commit: Some(c) }, Ok(t)) if c != t);
    match &f.install {
        Install::Missing => println!("help: vega lanista install   # from git main"),
        Install::Editable { .. } | Install::Other { .. } => {
            println!("help: vega lanista install --replace   # replace with git main")
        }
        _ if upgrade => println!("help: vega lanista install   # upgrade to main"),
        _ if state != seats::State::InSync => {
            println!("help: vega lanista install   # write the seats file from the vega repo")
        }
        _ => println!("help: vega lanista pin <provider> [--tier flagship|balanced|budget]"),
    }
    Ok(ExitCode::SUCCESS)
}

/// Where `lanista fetch` gets the Artificial Analysis key from: the env, or
/// vega secrets (docs/design/secrets.md). Never the value.
fn aa_key() -> String {
    if env::var_os(AA_ENV).is_some_and(|v| !v.is_empty()) {
        return "env".into();
    }
    let Ok(store) = vega_secrets::Store::from_env() else {
        return "missing".into();
    };
    let m = store.manifest().unwrap_or_default();
    let entry = m
        .entries()
        .into_iter()
        .find(|e| e.env == AA_ENV && e.tools.iter().any(|t| t == "lanista"));
    aa_key_label(entry.map(|e| store.here(&m, &e)))
}

const AA_ENV: &str = "ARTIFICIAL_ANALYSIS_KEY";

fn aa_key_label(here: Option<vega_secrets::Here>) -> String {
    match here {
        Some(vega_secrets::Here::Ok) => {
            "vega secrets (fetch with: vega secrets run --for lanista -- lanista fetch)".into()
        }
        Some(h) => format!("vega secrets {} (see vega secrets)", h.label()),
        None => "missing (fetch skips Artificial Analysis; the Slayer: vega secrets set)".into(),
    }
}

/// This host, as vegad names it (`VEGA_POST_HOST`, else the hostname).
fn host() -> String {
    vega_herdr::state::host_token(
        &env::var("VEGA_POST_HOST")
            .ok()
            .filter(|h| !h.is_empty())
            .unwrap_or_else(vega_herdr::local_host),
    )
}

/// `3526 models, 2 days old, from ganymede`, from `lanista --json` (its
/// status object) and vegad's catalog state.
fn catalog() -> String {
    let out = output_within(
        Command::new("lanista").arg("--json"),
        Duration::from_secs(15),
    );
    let Ok(Some(out)) = out else {
        return "unknown (lanista did not answer)".into();
    };
    let v: Value = serde_json::from_slice(&out.stdout).unwrap_or(Value::Null);
    let st = catalog::State::load(&catalog::state_path());
    let source = catalog::source(st.as_ref(), v["generated_at"].as_str(), &host());
    catalog_summary(&v, &source)
}

fn catalog_summary(v: &Value, source: &str) -> String {
    match v["status"].as_str() {
        Some("ok") => format!(
            "{} models, {} days old, from {source}{}",
            v["model_count"]
                .as_u64()
                .map_or("?".into(), |n| n.to_string()),
            v["age_days"].as_i64().map_or("?".into(), |n| n.to_string()),
            if v["age_days"]
                .as_i64()
                .is_some_and(|d| d > catalog::STALE_DAYS)
            {
                " (stale: see catalog_sync)"
            } else {
                ""
            }
        ),
        Some("no_index") => "none (vegad copies it from the builder; see catalog_sync)".into(),
        _ => "unknown (unexpected lanista --json output)".into(),
    }
}

/// What vegad does with the catalog here: the builder fetches and
/// publishes, every other host checks and receives
/// (docs/design/lanista-catalog.md).
fn sync_line(st: Option<&catalog::State>, builder: &str, host: &str, now: i64) -> String {
    let ago = |t: i64| {
        if t == 0 {
            "never".to_string()
        } else {
            vega_bd_sync::ago(now - t)
        }
    };
    let Some(st) = st else {
        return format!("none yet (vegad syncs it; builder {builder})");
    };
    let check = if st.check.is_empty() { "-" } else { &st.check };
    if host == builder {
        let fetch = if st.fetch.is_empty() { "-" } else { &st.fetch };
        format!(
            "builder: fetched {} ({fetch}), published {} ({check})",
            ago(st.fetched_at),
            ago(st.published_at)
        )
    } else {
        format!(
            "from {builder}: checked {} ({check}), received {}",
            ago(st.checked_at),
            ago(st.received_at)
        )
    }
}

fn install_cmd(git_ref: &str, replace: bool) -> Result<ExitCode, Failure> {
    let f = found()?;
    let target = install::resolve(git_ref)
        .map_err(|e| Failure::new("ref_unknown", e, "check --ref and the network"))?;
    let spec = install::spec(&target);
    let path = seats::path();
    let would = format!(
        "would run `uv tool install --force {spec}` and write {}",
        tilde(&path.display().to_string())
    );
    match &f.install {
        Install::Editable { path } if !replace => {
            return Err(Failure::new(
                "editable_install",
                format!(
                    "lanista is an editable install from {}; {would}",
                    tilde(path)
                ),
                "vega lanista install --replace",
            ));
        }
        Install::Other { what } if !replace => {
            return Err(Failure::new(
                "foreign_install",
                format!("lanista is installed another way ({what}); {would}"),
                "vega lanista install --replace",
            ));
        }
        _ => {}
    }
    let before = install_line(&f, git_ref, &Ok(target.clone()));
    let current = matches!(&f.install, Install::Git { commit: Some(c) } if *c == target);
    if !current {
        let out = Command::new("uv")
            .args(["tool", "install", "--force", &spec])
            .output()
            .map_err(|e| Failure::new("uv_missing", e.to_string(), "install uv"))?;
        if !out.status.success() {
            let err = String::from_utf8_lossy(&out.stderr);
            let last = err
                .lines()
                .rev()
                .find(|l| !l.trim().is_empty())
                .unwrap_or("");
            return Err(Failure::new(
                "uv_failed",
                format!("uv tool install {spec}: {}", last.trim()),
                "run the uv command by hand to see the full error",
            ));
        }
    }
    let wrote = seats::sync(&path)
        .map_err(|e| Failure::new("seats_write", e, "check the lanista config dir"))?;
    println!("before: {before}");
    println!(
        "lanista: {} {} ({})",
        if current { "current" } else { "installed" },
        install::short(&target),
        install::ref_label(git_ref)
    );
    println!("seats: {wrote} {}", tilde(&path.display().to_string()));
    let note = seats_backup_note(&path);
    if wrote == "written" && !note.is_empty() {
        println!("{note}");
    }
    println!("help: vega lanista   # catalog age; lanista fetch refreshes it");
    Ok(ExitCode::SUCCESS)
}

fn seats_backup_note(path: &Path) -> String {
    let bak = path.with_extension("json.vega-bak");
    if bak.exists() {
        format!("backup: {}", tilde(&bak.display().to_string()))
    } else {
        String::new()
    }
}

fn doctor() -> Result<ExitCode, Failure> {
    let out = Command::new("lanista")
        .args(["--json", "doctor"])
        .output()
        .map_err(|_| {
            Failure::new(
                "lanista_missing",
                "lanista is not installed",
                "vega lanista install",
            )
        })?;
    let v: Value = serde_json::from_slice(&out.stdout).map_err(|_| {
        Failure::new(
            "lanista_bad_output",
            "lanista --json doctor returned non-JSON output",
            "run `lanista doctor` by hand",
        )
    })?;
    let rows = doctor_rows(&v);
    println!("checks[{}]{{status,name,message,fix}}:", rows.len());
    for r in &rows {
        println!("  {r}");
    }
    let errors = v["summary"]["errors"].as_u64().unwrap_or(0);
    if errors > 0 {
        println!("help: fix the errors above, then vega lanista doctor");
    }
    Ok(if out.status.success() && errors == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    })
}

/// TOON rows from lanista's doctor JSON, errors first.
fn doctor_rows(v: &Value) -> Vec<String> {
    [("errors", "error"), ("warnings", "warn"), ("info", "ok")]
        .iter()
        .flat_map(|(key, status)| {
            v[*key]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .map(move |c| {
                    format!(
                        "{status},{},{},{}",
                        cell(c["name"].as_str().unwrap_or("-")),
                        cell(c["message"].as_str().unwrap_or("")),
                        cell(c["fix_hint"].as_str().unwrap_or("-"))
                    )
                })
        })
        .collect()
}

fn pin(
    provider: &str,
    tier: &str,
    seat: &str,
    set: Option<String>,
    clear: bool,
    until: Option<String>,
) -> Result<ExitCode, Failure> {
    // vega's tiers: the repo copy, whatever a host's file says.
    let blob = seats::SEATS;
    let pins = seats::pins(blob, seat).map_err(|e| {
        Failure::new(
            "seat_unknown",
            e,
            format!("known seats: {}", seats::seat_ids(blob).join(" ")),
        )
    })?;
    let lanes = seats::providers(&pins);
    if !lanes.iter().any(|l| l == provider) {
        return Err(Failure::new(
            "provider_unknown",
            format!("seat {seat} has no lane {provider}"),
            format!("lanes: {}", lanes.join(" ")),
        ));
    }
    let p = seats::pick(&pins, provider, tier).ok_or_else(|| {
        Failure::new(
            "tier_missing",
            format!("no {tier} pin for {provider} in seat {seat}"),
            "add a `tier` to a pin in plugins/lanista/seats.json, then vega lanista install",
        )
    })?;
    let now = vega_herdr::capacity::now();
    let mut model = p.model.clone();
    let mut source = "global seats".to_string();
    let cwd =
        env::current_dir().map_err(|e| Failure::new("project_io", e.to_string(), "check cwd"))?;
    let project = vega_herdr::project::resolve_optional(&cwd, &vega_herdr::state::post_host())
        .map_err(|e| Failure::new(&e.code, e.message, e.hint))?
        .map(|p| p.id);
    let mutation = set.is_some() || clear;
    if set
        .as_ref()
        .is_some_and(|m| m.trim().is_empty() || m.chars().any(char::is_control))
    {
        return Err(Failure::new(
            "usage",
            "model must be nonempty and contain no control characters",
            "vega lanista pin <lane> --set <model>",
        ));
    }
    if project.is_none() && mutation {
        return Err(Failure::new(
            "project_missing",
            "cwd is not a git project",
            "run the pin command inside the project's checkout",
        ));
    }
    let expires = if set.is_some() {
        match until.as_deref() {
            Some(u) => vega_registry::pins::expiry(u, now)
                .map_err(|e| Failure::new("usage", e, "--until 30m|2h|1d|never"))?,
            None => Some(
                vega_herdr::capacity::read()
                    .and_then(|sample| vega_registry::pins::reset(&sample, provider, now))
                    .map_err(|e| {
                        Failure::new(
                            "reset_unknown",
                            e,
                            "choose --until <duration> or --until never, or refresh capacity",
                        )
                    })?,
            ),
        }
    } else {
        None
    };
    if let Some(project) = project {
        let record = vega_registry::pins::Pin {
            schema: 2,
            project: project.clone(),
            seat: seat.into(),
            lane: provider.into(),
            tier: tier.into(),
            model: None,
            at: 0,
            expires,
        };
        let change = if mutation { Some(set) } else { None };
        match vega_registry::pins::access(record, change) {
            Ok(view) => {
                if let Some((host, pin)) =
                    vega_registry::pins::latest(&view, &project, seat, provider, tier)
                {
                    if let Some(m) = pin.active(vega_herdr::capacity::now()) {
                        model = m.to_string();
                        source = format!("project ({host})");
                        println!(
                            "expires: {}",
                            pin.expires.map_or("never".into(), |t| t.to_string())
                        );
                    }
                }
                if mutation {
                    println!("action: {}", if clear { "cleared" } else { "set" });
                }
            }
            Err(e) if mutation => {
                return Err(Failure::new(
                    "registry_write",
                    e,
                    "check broker reachability and this host's registry write grant",
                ))
            }
            Err(e) => println!(
                "registry: {}",
                cell(&format!("unavailable: {e}; using global seats"))
            ),
        }
        println!("project: {}", cell(&project));
    }
    println!("model: {}", cell(&model));
    println!("source: {}", cell(&source));
    println!("provider: {}", p.provider);
    println!("tier: {}", p.tier);
    println!("role: {}", p.role);
    println!("engine: {}", p.engine);
    println!("seat: {seat}");
    Ok(ExitCode::SUCCESS)
}

fn bin() -> String {
    env::current_exe()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "unknown".into())
}

fn tilde(path: &str) -> String {
    match env::var("HOME") {
        Ok(home) if !home.is_empty() && path.starts_with(&home) => {
            format!("~{}", &path[home.len()..])
        }
        _ => path.to_string(),
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
    use serde_json::json;

    #[test]
    fn skill_has_valid_frontmatter() {
        vega_plugin::assert_guide(SKILL, "lanista");
    }

    #[test]
    fn install_line_says_current_or_differs() {
        let git = |c: &str| Found {
            install: Install::Git {
                commit: Some(c.into()),
            },
            version: None,
        };
        let t = Ok("03892da05d".to_string());
        assert_eq!(
            install_line(&git("03892da05d"), "main", &t),
            "git 03892da (main 03892da, current)"
        );
        assert_eq!(
            install_line(&git("c671af1aaa"), "main", &t),
            "git c671af1 (main 03892da, differs)"
        );
        assert_eq!(
            install_line(&git("c671af1aaa"), "main", &Err("offline".into())),
            "git c671af1 (main unknown)"
        );
        let ed = Found {
            install: Install::Editable {
                path: "/x/lanista".into(),
            },
            version: None,
        };
        assert_eq!(install_line(&ed, "main", &t), "editable /x/lanista");
    }

    #[test]
    fn aa_key_names_the_source_not_the_value() {
        assert!(
            aa_key_label(Some(vega_secrets::Here::Ok)).contains("vega secrets run --for lanista")
        );
        assert_eq!(
            aa_key_label(Some(vega_secrets::Here::Missing)),
            "vega secrets missing (see vega secrets)"
        );
        assert!(aa_key_label(None).starts_with("missing"));
    }

    #[test]
    fn catalog_from_lanista_status_json() {
        let old = json!({"status": "ok", "age_days": 3, "model_count": 3526});
        assert_eq!(
            catalog_summary(&old, "ganymede"),
            "3526 models, 3 days old, from ganymede (stale: see catalog_sync)"
        );
        let fresh = json!({"status": "ok", "age_days": 2, "model_count": 10});
        assert_eq!(
            catalog_summary(&fresh, "proteus"),
            "10 models, 2 days old, from proteus"
        );
        assert!(catalog_summary(&json!({"status": "no_index"}), "x").starts_with("none"));
        assert!(catalog_summary(&Value::Null, "x").starts_with("unknown"));
    }

    #[test]
    fn sync_line_per_role() {
        let now = 100_000;
        assert_eq!(
            sync_line(None, "ganymede", "proteus", now),
            "none yet (vegad syncs it; builder ganymede)"
        );
        let st = catalog::State {
            checked_at: now - 240,
            check: "ok".into(),
            received_at: now - 7200,
            fetched_at: now - 3600,
            fetch: "error: lanista fetch: exit status: 1".into(),
            published_at: 0,
            ..Default::default()
        };
        assert_eq!(
            sync_line(Some(&st), "ganymede", "proteus", now),
            "from ganymede: checked 4m ago (ok), received 2h ago"
        );
        assert_eq!(
            sync_line(Some(&st), "ganymede", "ganymede", now),
            "builder: fetched 1h ago (error: lanista fetch: exit status: 1), published never (ok)"
        );
    }

    #[test]
    fn doctor_rows_put_errors_first_and_quote_cells() {
        let v = json!({
            "errors": [{"name": "index", "message": "missing, run fetch", "fix_hint": "lanista fetch"}],
            "warnings": [],
            "info": [{"name": "seats", "message": "ok", "fix_hint": null}],
            "summary": {"errors": 1}
        });
        assert_eq!(
            doctor_rows(&v),
            [
                "error,index,\"missing, run fetch\",lanista fetch",
                "ok,seats,ok,-"
            ]
        );
    }
}
