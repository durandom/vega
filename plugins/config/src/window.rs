//! Launch an applied herdr-app profile without changing the main window.
use clap::{error::ErrorKind, CommandFactory, Parser, Subcommand};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    fs::{self, File},
    path::Path,
    process::{Command, ExitCode, Stdio},
    time::{Duration, Instant},
};
use vega_config::{
    profiles::{self, Profile, Terminal},
    window_overlay as overlay, Env,
};
use vega_herdr::{cell, print_failure, Failure};

const GUIDE: &str = include_str!("../WINDOW.md");
#[derive(Parser)]
#[command(name = "vega window", bin_name = "vega window", disable_version_flag = true,
    about = "Open or raise an applied herdr-app profile", after_help = include_str!("../help/window.md"))]
struct Cli {
    #[arg(value_parser = profile_name)]
    name: Option<String>,
    #[command(subcommand)]
    action: Option<Action>,
    #[arg(long)]
    skill: bool,
    #[arg(short = 'V', short_alias = 'v', long)]
    version: bool,
}

#[derive(Subcommand)]
enum Action {
    /// Effective profile and local overlay (JSON for the config view).
    Show { name: String },
    /// Add a plain shell on a machine (use local for this computer).
    Add {
        name: String,
        #[arg(long)]
        machine: String,
    },
    /// Change a display title without changing terminal labels.
    Rename {
        name: String,
        id: String,
        title: String,
    },
    /// Move a tab to a one-based position.
    Move {
        name: String,
        id: String,
        index: usize,
    },
    /// Hide a tab, retaining its running terminals.
    Close { name: String, id: String },
    /// Discard all local tab customizations.
    Reset { name: String },
    /// Write the effective profile into a checkout, without committing or applying.
    Promote {
        name: String,
        #[arg(long, default_value = ".")]
        repo: std::path::PathBuf,
    },
}
impl Action {
    fn name(&self) -> &str {
        match self {
            Self::Show { name }
            | Self::Add { name, .. }
            | Self::Rename { name, .. }
            | Self::Move { name, .. }
            | Self::Close { name, .. }
            | Self::Reset { name }
            | Self::Promote { name, .. } => name,
        }
    }
}

fn profile_name(s: &str) -> Result<String, String> {
    if profiles::valid_name(s) {
        Ok(s.into())
    } else {
        Err("use 1-32 lowercase letters, digits, - or _".into())
    }
}

trait Backend {
    fn inventory(&mut self) -> Result<Value, String>;
    fn available(&mut self, terminal: &Terminal) -> Result<bool, String>;
    fn start(&mut self, window: &str, terminal: &Terminal) -> Result<String, String>;
    fn open(&mut self, home: &Path) -> Result<(), String>;
}

struct Live {
    raise: bool,
}
impl Backend for Live {
    fn inventory(&mut self) -> Result<Value, String> {
        vega_herdr::hc::terminal_list().map_err(|e| e.message)
    }
    fn available(&mut self, t: &Terminal) -> Result<bool, String> {
        let out = Command::new("hc")
            .args([
                "exec",
                "--on",
                &t.machine,
                "--timeout",
                "10",
                "--",
                "sh",
                "-lc",
                r#"command -v -- "$1" >/dev/null"#,
                "vega-window",
                &t.argv[0],
            ])
            .output()
            .map_err(|e| e.to_string())?;
        Ok(out.status.success())
    }
    fn start(&mut self, window: &str, t: &Terminal) -> Result<String, String> {
        let out = Command::new("hc")
            .args(start_args(window, t))
            .output()
            .map_err(|e| e.to_string())?;
        let reply: Value = serde_json::from_slice(&out.stdout)
            .map_err(|_| format!("starting {}: invalid terminal reply", t.name))?;
        if !out.status.success() {
            return Err(format!("starting {} failed: {reply}", t.name));
        }
        reply["address"]
            .as_str()
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .ok_or_else(|| format!("starting {}: missing terminal address", t.name))
    }
    fn open(&mut self, home: &Path) -> Result<(), String> {
        if !self.raise {
            return Ok(());
        }
        // Bare herdr-app opens or raises the instance selected by its home.
        let mut child = Command::new("herdr-app")
            .env("HERDR_APP_HOME", home)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| e.to_string())?;
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let switched = Command::new("herdr-app")
                .env("HERDR_APP_HOME", home)
                .args(["do", "layout.switch", "--name", "vega-window"])
                .output()
                .map_err(|e| e.to_string())?;
            if switched.status.success() {
                let reply: Value = serde_json::from_slice(&switched.stdout)
                    .map_err(|_| "invalid layout.switch reply")?;
                if reply.get("error").is_none() {
                    let status = Command::new("herdr-app")
                        .env("HERDR_APP_HOME", home)
                        .arg("status")
                        .output()
                        .map_err(|e| e.to_string())?;
                    let state: Value = serde_json::from_slice(&status.stdout)
                        .map_err(|_| "invalid profile status reply")?;
                    if status.status.success()
                        && state["layout"] == "vega-window"
                        && state["errors"].as_array().is_some_and(|a| a.is_empty())
                    {
                        let _ = child.try_wait();
                        return Ok(());
                    }
                    return Err(format!("profile layout was not accepted: {state}"));
                }
            }
            if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
                if !status.success() {
                    return Err(format!("profile app exited {status}"));
                }
            }
            if Instant::now() >= deadline {
                return Err("profile window did not accept its layout within 10s; inspect HERDR_APP_HOME=<profile-home> herdr-app status".into());
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

fn start_args(window: &str, t: &Terminal) -> Vec<String> {
    let mut args: Vec<String> = [
        "run",
        "--json",
        "--bg",
        "--no-parent",
        "--no-notify",
        "--on",
        &t.machine,
        "--name",
        &t.name,
        "--label",
        &format!("vega.window={window}"),
        "--label",
        &format!("vega.window.slot={}", t.name),
        "--",
    ]
    .into_iter()
    .map(String::from)
    .collect();
    if t.machine == "local" {
        let on = args.iter().position(|a| a == "--on").unwrap();
        args.drain(on..on + 2);
    }
    args.extend(t.argv.clone());
    args
}

#[derive(Debug, Default)]
struct Launch {
    started: usize,
    reused: usize,
    skipped: usize,
    warnings: Vec<String>,
}

fn launch(
    home: &Path,
    name: &str,
    p: &Profile,
    mut layout: Value,
    backend: &mut impl Backend,
) -> Result<Launch, String> {
    // Validate slots before terminal side effects, including hand-edited applied profiles.
    let placeholders = p
        .terminals()
        .map(|t| (t.name.clone(), "t0".into()))
        .collect();
    let mut validated = layout.clone();
    profiles::build_tabs(&mut validated, p, &placeholders)?;
    profiles::resolve_layout(&mut validated, &placeholders)?;
    let inventory = backend.inventory()?;
    let all = inventory["terminals"]
        .as_array()
        .ok_or("terminal inventory has no terminals array")?;
    let mut addresses = BTreeMap::new();
    let mut result = Launch::default();
    for t in p.terminals() {
        let matches: Vec<_> = all
            .iter()
            .filter(|x| {
                x["state"] == "live"
                    && x["labels"]["vega.window"] == name
                    && x["labels"]["vega.window.slot"] == t.name
            })
            .collect();
        if matches.len() > 1 {
            return Err(format!(
                "multiple live terminals for {name}/{}; inspect hc ls before retrying",
                t.name
            ));
        }
        let address = if let Some(existing) = matches.first() {
            existing["address"]
                .as_str()
                .filter(|s| !s.is_empty())
                .ok_or("existing terminal has no address")?
                .to_string()
        } else {
            if t.optional {
                match backend.available(t) {
                    Ok(true) => {}
                    outcome => {
                        let detail = outcome.err().unwrap_or_else(|| {
                            format!(
                                "{} is not installed or {} is unavailable",
                                t.argv[0], t.machine
                            )
                        });
                        result
                            .warnings
                            .push(format!("skipping {}: {detail}", t.name));
                        result.skipped += 1;
                        continue;
                    }
                }
            }
            let address = backend.start(name, t)?;
            result.started += 1;
            address
        };
        addresses.insert(t.name.clone(), address);
    }
    result.reused = addresses.len() - result.started;
    profiles::build_tabs(&mut layout, p, &addresses)?;
    profiles::resolve_layout(&mut layout, &addresses)?;
    fn configure(node: &mut Value, name: &str, p: &Profile) {
        if node["content"]["mod"] == "vega-window-tabs" {
            node["content"]["props"] = serde_json::json!({"profile":name, "order":p.tabs.iter().map(|t| &t.name).collect::<Vec<_>>(), "titles":p.tabs.iter().map(|t| (t.name.clone(), t.title.clone().unwrap_or_else(|| t.name.clone()))).collect::<BTreeMap<_,_>>()});
        }
        if let Some(children) = node["children"].as_array_mut() {
            for child in children {
                configure(child, name, p);
            }
        }
    }
    configure(&mut layout, name, p);
    fs::create_dir_all(home.join("layouts")).map_err(|e| e.to_string())?;
    let mut tmp =
        tempfile::NamedTempFile::new_in(home.join("layouts")).map_err(|e| e.to_string())?;
    serde_json::to_writer_pretty(&mut tmp, &layout).map_err(|e| e.to_string())?;
    tmp.persist(home.join("layouts/vega-window.json"))
        .map_err(|e| e.to_string())?;
    backend.open(home)?;
    Ok(result)
}

fn run(cli: Cli) -> Result<(), String> {
    let env = Env::from_env().ok_or("HOME is required")?;
    if let Some(action) = cli.action {
        return manage(&env, action);
    }
    let Some(name) = cli.name else {
        println!(
            "bin: {}",
            cell(
                &std::env::current_exe()
                    .map_err(|e| e.to_string())?
                    .display()
                    .to_string()
            )
        );
        println!("description: Open independent herdr-app profile windows");
        let profiles = profiles::list(&env)?;
        println!("profiles[{}]{{name,applied}}:", profiles.len());
        for (file, _) in profiles {
            let name = file.trim_end_matches(".toml");
            println!(
                "  {},{}",
                cell(name),
                profiles::home(&env, name)?.join("profile.toml").is_file()
            );
        }
        println!("help: vega config apply herdr-app.profiles; vega window <name>");
        return Ok(());
    };
    let home = profiles::home(&env, &name)?;
    // OS releases the advisory lock even after a crash; concurrent opens cannot duplicate terminals.
    let lock = File::options()
        .create(true)
        .truncate(false)
        .write(true)
        .open(home.join(".vega-window.lock"))
        .map_err(|e| e.to_string())?;
    lock.try_lock()
        .map_err(|_| "this profile is already opening; retry vega window after it finishes")?;
    let (base, layout) = profiles::read_applied(&home)?;
    let profile = overlay::effective(&base, &overlay::read(&overlay::path(&env, &name))?)?;
    let result = launch(&home, &name, &profile, layout, &mut Live { raise: true })?;
    println!("window: {}", cell(&name));
    println!("started: {}", result.started);
    println!("reused: {}", result.reused);
    println!("skipped: {}", result.skipped);
    for warning in result.warnings {
        println!("warning: {}", cell(&warning));
    }
    Ok(())
}

fn manage(env: &Env, action: Action) -> Result<(), String> {
    let name = action.name().to_owned();
    let home = profiles::home(env, &name)?;
    let lock = File::options()
        .create(true)
        .truncate(false)
        .write(true)
        .open(home.join(".vega-window.lock"))
        .map_err(|e| e.to_string())?;
    lock.try_lock()
        .map_err(|_| "profile is busy; retry after the current edit")?;
    let (base, layout) = profiles::read_applied(&home)?;
    if !base.terminals.is_empty() {
        return Err("tab management requires a grouped profile".into());
    }
    let path = overlay::path(env, &name);
    let mut local = if matches!(action, Action::Reset { .. }) {
        overlay::Overlay::default()
    } else {
        overlay::read(&path)?
    };
    let before = overlay::effective(&base, &local)?;
    if matches!(action, Action::Show { .. }) {
        println!(
            "{}",
            serde_json::json!({"profile":before,"overlay":local,"config":overlay::text(&before)?,"diff":overlay::diff(&base, &before)?})
        );
        return Ok(());
    }
    if let Action::Promote { repo, .. } = action {
        let destination = overlay::promote(&repo, &name, &base, &local)?;
        println!("profile: {}", cell(&destination.display().to_string()));
        println!("status: promoted; review and commit the working-tree change, then apply and reset the overlay");
        return Ok(());
    }
    match action {
        Action::Add { machine, .. } => {
            local.order = before.tabs.iter().map(|t| t.name.clone()).collect();
            overlay::add(&base, &mut local, &machine)?;
        }
        Action::Rename { id, title, .. } => {
            if !before.tabs.iter().any(|t| t.name == id) {
                return Err("unknown tab ID".into());
            }
            if title.trim().is_empty() || title.len() > 120 || title.chars().any(char::is_control) {
                return Err("title must be 1-120 bytes without control characters".into());
            }
            local.titles.insert(id, title);
        }
        Action::Move { id, index, .. } => {
            let mut ids: Vec<_> = before.tabs.iter().map(|t| t.name.clone()).collect();
            let from = ids.iter().position(|t| t == &id).ok_or("unknown tab ID")?;
            if index == 0 || index > ids.len() {
                return Err("index must be within the tab list (1-based)".into());
            }
            ids.remove(from);
            ids.insert(index - 1, id);
            local.order = ids;
        }
        Action::Close { id, .. } => {
            if !local.hidden.contains(&id) {
                if !before.tabs.iter().any(|t| t.name == id) {
                    return Err("unknown tab ID".into());
                }
                local.hidden.push(id);
            }
        }
        Action::Reset { .. } => {
            local = overlay::Overlay::default();
        }
        _ => unreachable!(),
    }
    let effective = overlay::effective(&base, &local)?;
    // Persist intent first: a failed remote start can be retried by reopening.
    overlay::save(&path, &local)?;
    let result = launch(&home, &name, &effective, layout, &mut Live { raise: false })?;
    println!("window: {}", cell(&name));
    println!("status: saved");
    for warning in result.warnings {
        println!("warning: {}", cell(&warning));
    }
    Ok(())
}

fn main() -> ExitCode {
    let cli = match Cli::try_parse() {
        Ok(c) => c,
        Err(e) if e.kind() == ErrorKind::DisplayHelp => {
            let _ = e.print();
            return ExitCode::SUCCESS;
        }
        Err(e) => {
            print_failure(&Failure::new(
                "usage",
                e.to_string(),
                "run vega window --help",
            ));
            print!("{}", Cli::command().render_long_help());
            return ExitCode::from(2);
        }
    };
    if cli.version {
        println!("{}", vega_build::version_line!());
        return ExitCode::SUCCESS;
    }
    if cli.skill {
        print!("{GUIDE}");
        return ExitCode::SUCCESS;
    }
    match run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            print_failure(&Failure::new(
                "window_failed",
                e,
                "run vega config herdr-app.profiles; vega window --help",
            ));
            ExitCode::from(1)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[derive(Default)]
    struct Fake {
        terminals: Vec<Value>,
        starts: usize,
        opens: usize,
        fail: bool,
        available: bool,
        probes: usize,
    }
    impl Backend for Fake {
        fn inventory(&mut self) -> Result<Value, String> {
            Ok(json!({"terminals": self.terminals}))
        }
        fn available(&mut self, _: &Terminal) -> Result<bool, String> {
            self.probes += 1;
            Ok(self.available)
        }
        fn start(&mut self, window: &str, t: &Terminal) -> Result<String, String> {
            if self.fail && self.starts == 1 {
                return Err("fake offline machine".into());
            }
            self.starts += 1;
            let address = format!("{}/t{}", t.machine, self.starts);
            self.terminals.push(json!({"address": address, "state":"live", "labels":{"vega.window":window,"vega.window.slot":t.name}}));
            Ok(address)
        }
        fn open(&mut self, _: &Path) -> Result<(), String> {
            self.opens += 1;
            Ok(())
        }
    }
    fn fixture() -> (Profile, Value) {
        (
            profiles::parse(
                vega_config::sources::default("herdr-app/profiles/status.toml").unwrap(),
            )
            .unwrap(),
            serde_json::from_str(
                vega_config::sources::default("herdr-app/layouts/status.json").unwrap(),
            )
            .unwrap(),
        )
    }
    #[test]
    fn guide() {
        vega_plugin::assert_guide(GUIDE, "window");
    }
    #[test]
    fn launch_reuses_slots_and_does_not_adopt_spaces() {
        let dir = tempfile::tempdir().unwrap();
        let (p, l) = fixture();
        let mut fake = Fake::default();
        assert_eq!(
            launch(dir.path(), "status", &p, l.clone(), &mut fake)
                .unwrap()
                .started,
            3
        );
        assert_eq!(
            launch(dir.path(), "status", &p, l.clone(), &mut fake)
                .unwrap()
                .started,
            0
        );
        assert_eq!(fake.opens, 2);
        let rendered = fs::read_to_string(dir.path().join("layouts/vega-window.json")).unwrap();
        assert!(rendered.contains("ganymede/t3"));
        assert!(!rendered.contains("\"terminal\": \"profile:"));
        fake.terminals[0]["state"] = "finished".into();
        assert_eq!(
            launch(dir.path(), "status", &p, l, &mut fake)
                .unwrap()
                .started,
            1
        );
        let args = start_args("status", p.terminals().last().unwrap());
        assert!(args.windows(2).any(|a| a == ["--on", "ganymede"]));
        assert!(args.windows(3).any(|a| a == ["--", "token-burn", "tui"]));
        assert!(!args
            .iter()
            .any(|a| a.contains("herdr.space") || a.contains("herdr.tab")));
    }
    #[test]
    fn optional_group_is_skipped_then_added_and_live_slot_avoids_probes() {
        let dir = tempfile::tempdir().unwrap();
        let (p, l) = fixture();
        let mut fake = Fake::default();
        let first = launch(dir.path(), "status", &p, l.clone(), &mut fake).unwrap();
        assert_eq!((first.started, first.reused, first.skipped), (3, 0, 1));
        assert_eq!(first.warnings.len(), 1);
        let before: Value =
            serde_json::from_slice(&fs::read(dir.path().join("layouts/vega-window.json")).unwrap())
                .unwrap();
        assert_eq!(
            before["children"][1]["children"].as_array().unwrap().len(),
            3
        );
        fake.available = true;
        let second = launch(dir.path(), "status", &p, l.clone(), &mut fake).unwrap();
        assert_eq!((second.started, second.reused, second.skipped), (1, 3, 0));
        let after: Value =
            serde_json::from_slice(&fs::read(dir.path().join("layouts/vega-window.json")).unwrap())
                .unwrap();
        assert_eq!(after["children"][1]["children"][2]["id"], "io");
        fake.available = false;
        let third = launch(dir.path(), "status", &p, l, &mut fake).unwrap();
        assert_eq!((third.started, third.reused, third.skipped), (0, 4, 0));
        assert_eq!(fake.probes, 2);
    }

    #[test]
    fn partial_failure_retries_without_duplicating_and_bad_slots_do_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let (p, l) = fixture();
        let mut fake = Fake {
            fail: true,
            ..Fake::default()
        };
        assert!(launch(dir.path(), "status", &p, l.clone(), &mut fake).is_err());
        assert_eq!(fake.starts, 1);
        assert_eq!(fake.opens, 0);
        fake.fail = false;
        assert_eq!(
            launch(dir.path(), "status", &p, l, &mut fake)
                .unwrap()
                .started,
            2
        );
        assert!(launch(
            dir.path(),
            "status",
            &p,
            json!({"content":{"terminal":"profile:nope"}}),
            &mut fake
        )
        .is_err());
        assert_eq!(fake.starts, 3);
        fake.terminals.push(fake.terminals[0].clone());
        assert!(launch(dir.path(), "status", &p, fixture().1, &mut fake).is_err());
        assert_eq!(fake.opens, 1);
    }
    #[test]
    fn parser_rejects_unknown_flags_and_extra_names() {
        assert!(Cli::try_parse_from(["vega-window", "status", "--typo"]).is_err());
        assert!(Cli::try_parse_from(["vega-window", "status", "extra"]).is_err());
    }
}
