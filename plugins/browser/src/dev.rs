//! Shared headed CfT, with serialized allocation and explicit CDP ownership.
mod groups;
mod process;
mod proxy;
pub mod share;
pub mod shared;
mod snapshot;
mod wire;

use std::collections::BTreeMap;
use std::env;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use vega_herdr::{platform, project, state, Failure};

use crate::{mark, owner, real, session_name};
use wire::Cdp;

pub const PROXY: &str = "--internal-dev-proxy";
pub use proxy::serve;

#[derive(Clone, Serialize, Deserialize)]
struct Browser {
    pid: u32,
    started: String,
    executable: PathBuf,
    profile: PathBuf,
    port: u16,
    path: String,
}

#[derive(Clone, Serialize, Deserialize)]
struct Project {
    label: String,
    context: String,
    window: i64,
    #[serde(default)]
    origins: std::collections::BTreeSet<String>,
}

#[derive(Serialize, Deserialize)]
struct Tab {
    target: String,
    port: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    chrome_tab_id: Option<i64>,
    #[serde(default)]
    init_script: Option<PathBuf>,
}

#[derive(Serialize, Deserialize)]
struct Session {
    project: project::ProjectId,
    generation: String,
    pane: Option<String>,
    active: u64,
    next: u64,
    tabs: BTreeMap<u64, Tab>,
}

#[derive(Default, Serialize, Deserialize)]
struct Registry {
    browser: Option<Browser>,
    projects: BTreeMap<project::ProjectId, Project>,
    sessions: BTreeMap<String, Session>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pending_restore: Option<String>,
}

fn failure(error: impl std::fmt::Display) -> Failure {
    Failure::new(
        "dev_browser_failed",
        error.to_string(),
        "vega browser dev --help",
    )
}

fn stale(message: &str) -> Failure {
    Failure::new(
        "stale_dev_session",
        message,
        "vega browser dev --session <name> close",
    )
}

fn usage(message: &str) -> Failure {
    Failure::new("usage", message, "vega browser dev --help")
}

fn profile() -> Result<PathBuf, Failure> {
    Ok(env::var_os("XDG_DATA_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| platform::home().map(|home| home.join(".local/share")))
        .ok_or_else(|| failure("Could not resolve the CfT data directory"))?
        .join("vega/browser/dev-cft"))
}

fn load(profile: &Path) -> Result<Registry, Failure> {
    match fs::read(profile.join("vega-dev.json")) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(failure),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Registry::default()),
        Err(e) => Err(failure(e)),
    }
}

fn save(profile: &Path, data: &Registry) -> Result<(), Failure> {
    let mut file = tempfile::NamedTempFile::new_in(profile).map_err(failure)?;
    serde_json::to_writer(file.as_file_mut(), data).map_err(failure)?;
    file.persist(profile.join("vega-dev.json"))
        .map_err(failure)?;
    Ok(())
}

fn runtime_config(profile: &Path) -> Result<PathBuf, Failure> {
    // Explicit config replaces upstream user and checkout config discovery.
    let path = profile.join("vega-agent-browser.json");
    match fs::read(&path) {
        Ok(config) if config == b"{}" => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            let mut file = tempfile::NamedTempFile::new_in(profile).map_err(failure)?;
            file.write_all(b"{}").map_err(failure)?;
            file.persist(&path).map_err(failure)?;
        }
        Ok(_) => return Err(failure("Unexpected managed agent-browser configuration")),
        Err(e) => return Err(failure(e)),
    }
    Ok(path)
}

fn lock(profile: &Path) -> Result<File, Failure> {
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .mode(0o600)
        .open(profile.join("vega-dev.lock"))
        .map_err(failure)?;
    platform::lock(&file).map_err(failure)?;
    Ok(file)
}

/// Existing dev names remain scoped on subsequent shim calls, even when
/// --headed is omitted. An unreadable registry fails closed.
pub fn routes(session: &str, args: &[String]) -> Result<bool, Failure> {
    if load(&profile()?)?.sessions.contains_key(session) {
        return Ok(true);
    }
    // Review invokes the adapter explicitly and preserves its legacy routes.
    if env::var("VEGA_BROWSER_LEGACY_HEADED").as_deref() == Ok("1") {
        return Ok(false);
    }
    Ok(mark::headed(args) && !custom_launch(args) && !crate::legacy::has_binding(session))
}

pub const LAUNCH_OVERRIDES: &[&str] = &[
    "AGENT_BROWSER_CDP",
    "AGENT_BROWSER_CDP_HEADERS",
    "AGENT_BROWSER_PROVIDER",
    "AGENT_BROWSER_AUTO_CONNECT",
    "AGENT_BROWSER_CONFIG",
    "AGENT_BROWSER_EXECUTABLE_PATH",
    "AGENT_BROWSER_PROFILE",
    "AGENT_BROWSER_ENGINE",
    "AGENT_BROWSER_RESTORE",
    "AGENT_BROWSER_NAMESPACE",
];

/// Recognize explicit upstream launch settings, including auto-loaded configs.
pub fn custom_launch(args: &[String]) -> bool {
    let custom = args.iter().any(|a| {
        matches!(
            a.split('=').next().unwrap_or_default(),
            "--cdp"
                | "--cdp-headers"
                | "--auto-connect"
                | "--provider"
                | "-p"
                | "--config"
                | "--restore"
                | "--executable-path"
                | "--profile"
                | "--engine"
                | "--namespace"
                | "connect"
        )
    }) || LAUNCH_OVERRIDES
        .iter()
        .any(|var| env::var_os(var).is_some_and(|v| !v.is_empty()));
    custom
        || crate::legacy::configs().any(|config| {
            [
                "cdp",
                "cdpHeaders",
                "provider",
                "executablePath",
                "profile",
                "engine",
                "restore",
                "namespace",
            ]
            .iter()
            .any(|key| !config[key].is_null())
                || config["autoConnect"] == true
        })
}

/// Named bindings in the configured dev registry, including stale generations.
pub fn session_names() -> Result<Vec<String>, Failure> {
    Ok(load(&profile()?)?.sessions.into_keys().collect())
}

struct Invocation<'a> {
    name: String,
    words: Vec<&'a str>,
    forwarded: Vec<&'a str>,
    init_script: Option<&'a str>,
    json: bool,
}

impl<'a> Invocation<'a> {
    fn background_tab_new(&self) -> bool {
        matches!(self.words.as_slice(), ["tab", "new", ..])
            && [
                "CLAUDECODE",
                "CLAUDE_CODE_SESSION_ID",
                "CODEX_THREAD_ID",
                "PI_CODING_AGENT",
                "AI_AGENT",
            ]
            .iter()
            .any(|key| env::var_os(key).is_some_and(|value| !value.is_empty()))
    }

    fn parse(args: &'a [String]) -> Result<Self, Failure> {
        let mut this = Self {
            name: session_name(args).ok_or_else(crate::name_failure)?,
            words: Vec::new(),
            forwarded: Vec::new(),
            init_script: None,
            json: false,
        };
        let mut args = args.iter().map(String::as_str).peekable();
        while let Some(arg) = args.next() {
            let (flag, value) = arg.split_once('=').unwrap_or((arg, ""));
            match flag {
                "--session" => {
                    if !arg.contains('=') {
                        args.next()
                            .filter(|v| !v.is_empty() && !v.starts_with('-'))
                            .ok_or_else(|| usage("--session requires a name"))?;
                    } else if value.is_empty() {
                        return Err(usage("--session requires a name"));
                    }
                }
                "--headed" if arg == flag => match args.peek() {
                    Some(&"true") => {
                        args.next();
                    }
                    Some(&"false") => {
                        return Err(usage("Use a fresh name for an isolated headless session"))
                    }
                    _ => {}
                },
                "--pin-tab" if arg == flag => {}
                "--json" if arg == flag => {
                    this.json = true;
                    this.forwarded.push(arg);
                }
                "--init-script" => {
                    let value = if arg == flag {
                        args.next().unwrap_or_default()
                    } else {
                        value
                    };
                    if value.is_empty() || value.starts_with('-') {
                        return Err(usage("--init-script requires a path"));
                    }
                    this.init_script = Some(value);
                    this.forwarded.extend(["--init-script", value]);
                }
                "--full" | "--interactive" | "--compact" | "--stdin" | "--base64" | "-i" | "-c"
                | "-d" | "-s" | "--selector" | "--depth" | "--quality" | "--timeout" | "--type"
                | "--annotate" => {
                    this.forwarded.push(arg);
                    if arg == flag
                        && matches!(
                            flag,
                            "-d" | "-s"
                                | "--selector"
                                | "--depth"
                                | "--quality"
                                | "--timeout"
                                | "--type"
                        )
                    {
                        this.forwarded.push(
                            args.next()
                                .filter(|v| !v.starts_with('-'))
                                .ok_or_else(|| usage("Missing option value"))?,
                        );
                    }
                }
                _ if arg.starts_with('-') => {
                    return Err(usage(&format!("{flag} is unsupported in dev sessions")))
                }
                _ => {
                    this.words.push(arg);
                    this.forwarded.push(arg);
                }
            }
        }
        if !matches!(
            this.words.as_slice(),
            ["tab", "new"]
                | ["tab", "new", _]
                | ["tab", "list"]
                | ["tab"]
                | ["tab", "close"]
                | ["tab", "close", _]
                | ["tab", _]
                | ["close"]
                | ["set", "viewport", ..]
                | ["network", "requests", ..]
                | [
                    "open"
                        | "goto"
                        | "navigate"
                        | "snapshot"
                        | "get"
                        | "is"
                        | "eval"
                        | "screenshot"
                        | "click"
                        | "dblclick"
                        | "hover"
                        | "focus"
                        | "type"
                        | "fill"
                        | "press"
                        | "check"
                        | "uncheck"
                        | "select"
                        | "scroll"
                        | "scrollintoview"
                        | "wait"
                        | "reload"
                        | "back"
                        | "forward"
                        | "dialog"
                        | "find"
                        | "pdf"
                        | "keyboard"
                        | "mouse",
                    ..
                ]
        ) {
            return Err(usage("Unsupported dev session command"));
        }
        if this.words.first() == Some(&"tab")
            && this.forwarded.len() != this.words.len() + usize::from(this.json)
        {
            return Err(usage("Use --json as the only option with dev tab commands"));
        }
        Ok(this)
    }

    fn output(&self, data: Value, text: impl std::fmt::Display) {
        if self.json {
            println!("{}", json!({"success": true, "data": data, "error": null}));
        } else {
            println!("{text}");
        }
    }
}

fn create_tab(
    cdp: &mut Cdp,
    browser: &Browser,
    project: &mut Project,
    name: &str,
    owned: &[i64],
    background: bool,
) -> Result<Tab, Failure> {
    let (target, chrome_tab_id) = if env::var("VEGA_BROWSER_MARK").as_deref() == Ok("0") {
        let target = cdp.create_target("about:blank", Some(project), background, false)?;
        (target, None)
    } else {
        let (target, id) = groups::allocate(cdp, project, name, owned, background)?;
        (target, Some(id))
    };
    let result = (|| {
        if project.window == 0 {
            project.window = cdp.window(&target)?;
        }
        cdp.info(project, &target)?;
        let port = proxy::start(&proxy::Binding {
            scope: proxy::Scope::Dev(browser.clone(), project.clone()),
            target: target.clone(),
        })?;
        Ok(Tab {
            target: target.clone(),
            port,
            chrome_tab_id,
            init_script: None,
        })
    })();
    if result.is_err() {
        let _ = cdp.call("Target.closeTarget", json!({"targetId": target}));
    }
    result
}

fn release(cdp: &mut Cdp, project: &Project, target: &str) -> Result<(), Failure> {
    let exists = |cdp: &mut Cdp| -> Result<bool, Failure> {
        let targets = cdp.call("Target.getTargets", json!({"filter": [{"type": "page"}]}))?;
        let items = targets["targetInfos"]
            .as_array()
            .ok_or_else(|| failure("Chrome returned no target list"))?;
        Ok(items.iter().any(|t| t["targetId"] == target))
    };
    if exists(cdp)? {
        cdp.info(project, target)?;
        if cdp.call("Target.closeTarget", json!({"targetId": target}))?["success"] != true {
            return Err(failure("Chrome did not close the owned dev tab"));
        }
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while exists(cdp)? {
            if std::time::Instant::now() >= deadline {
                return Err(failure("Chrome has not finished closing the owned dev tab"));
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
    }
    Ok(())
}

fn dispose_unused(cdp: &mut Cdp, project: &Project) -> Result<bool, Failure> {
    let targets = cdp.call("Target.getTargets", json!({}))?;
    let items = targets["targetInfos"]
        .as_array()
        .ok_or_else(|| failure("Chrome returned no target list"))?;
    if items
        .iter()
        .any(|t| t["type"] == "page" && t["browserContextId"] == project.context)
    {
        return Ok(false);
    }
    let contexts = cdp.call("Target.getBrowserContexts", json!({}))?;
    if contexts["browserContextIds"]
        .as_array()
        .is_some_and(|ids| ids.iter().any(|id| id == &project.context))
    {
        cdp.call(
            "Target.disposeBrowserContext",
            json!({"browserContextId": project.context}),
        )?;
    }
    Ok(true)
}

fn tab_id(raw: &str) -> Result<u64, Failure> {
    raw.strip_prefix('t')
        .and_then(|n| n.parse().ok())
        .ok_or_else(|| usage("Use an owned tab ID such as t1"))
}

fn passed() -> Result<ExitCode, Failure> {
    Ok(ExitCode::SUCCESS)
}

/// Close an abandoned managed session under the same registry and owner
/// locks used for allocation. A changed owner or unreadable pane holds it.
pub fn gc_close(name: &str) -> Result<bool, Failure> {
    let profile = profile()?.canonicalize().map_err(failure)?;
    let _guard = lock(&profile)?;
    let mut data = load(&profile)?;
    let Some(session) = data.sessions.get(name) else {
        return Ok(false);
    };
    let owners = owner::Locked::acquire().map_err(failure)?;
    let Some(record) = owners.get(name).map_err(failure)? else {
        return Ok(false);
    };
    let Some(pane) = record.pane.as_deref() else {
        return Ok(false);
    };
    let local = vega_herdr::local_host();
    if local.is_empty()
        || local == "unknown"
        || record.host != local
        || session.pane.as_deref() != Some(pane)
        || crate::pane::alive(pane) != Some(false)
    {
        return Ok(false);
    }
    if let Some(browser) = data.browser.clone() {
        if browser.path == session.generation && process::running(&browser, &profile)? {
            let mut cdp = Cdp::connect(&browser)?;
            close_session(&mut data, name, &browser, &mut cdp)?;
        } else {
            data.sessions.remove(name);
        }
    } else {
        data.sessions.remove(name);
    }
    save(&profile, &data)?;
    owners.remove(name).map_err(failure)?;
    Ok(true)
}

fn close_session(
    data: &mut Registry,
    name: &str,
    browser: &Browser,
    cdp: &mut Cdp,
) -> Result<(), Failure> {
    let session = data
        .sessions
        .get(name)
        .ok_or_else(|| stale("Missing dev session"))?;
    let project = data
        .projects
        .get(&session.project)
        .ok_or_else(|| stale("Missing project context"))?;
    for tab in session.tabs.values() {
        release(cdp, project, &tab.target)?;
    }
    let id = session.project.clone();
    data.sessions.remove(name);
    if !data
        .sessions
        .values()
        .any(|s| s.project == id && s.generation == browser.path)
        && dispose_unused(cdp, project)?
    {
        data.projects.remove(&id);
    }
    Ok(())
}

pub fn command(args: &[String]) -> Result<ExitCode, Failure> {
    if let Some((word, rest)) = args.split_first() {
        if word == "share" {
            return share::command(rest);
        }
    }
    let (call, action) = match args.split_first() {
        Some((word, rest)) if word == "state" => (None, Some(snapshot::Action::parse(rest)?)),
        _ => (Some(Invocation::parse(args)?), None),
    };
    let overridden = |var| env::var_os(var).is_some_and(|value| !value.is_empty());
    if [
        "AGENT_BROWSER_CDP",
        "AGENT_BROWSER_CDP_HEADERS",
        "AGENT_BROWSER_PROVIDER",
        "AGENT_BROWSER_EXECUTABLE_PATH",
        "AGENT_BROWSER_PROFILE",
    ]
    .into_iter()
    .any(overridden)
        || std::env::var("AGENT_BROWSER_AUTO_CONNECT")
            .is_ok_and(|v| matches!(v.as_str(), "1" | "true"))
        || action.is_some()
            && [
                "CDP_PORT",
                "CDP_PROFILE",
                "CDP_CHROMIUM",
                "AGENT_BROWSER_CONFIG",
            ]
            .into_iter()
            .any(overridden)
    {
        return Err(usage(
            "Unset browser endpoint or profile overrides for managed dev commands",
        ));
    }
    let cwd = env::current_dir().map_err(failure)?;
    let repo = project::resolve(&cwd, &state::post_host())?;
    let profile = profile()?;
    if fs::symlink_metadata(&profile).is_ok_and(|meta| meta.is_symlink()) {
        return Err(failure("The managed CfT profile cannot be a symlink"));
    }
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&profile)
        .map_err(failure)?;
    let profile = profile.canonicalize().map_err(failure)?;
    let guard = lock(&profile)?;
    let mut data = load(&profile)?;
    if let Some(action) = action {
        return snapshot::command(action, &profile, &repo, &mut data);
    }
    let call = call.expect("named dev invocation");
    let config = runtime_config(&profile)?;
    let previous = data.sessions.get(&call.name);
    if previous.is_some_and(|s| s.project != repo.id) {
        return Err(stale("This dev session name belongs to another project"));
    }
    let running = match &data.browser {
        Some(browser) => process::running(browser, &profile)?,
        None => false,
    };
    let expired = previous
        .is_some_and(|s| !running || data.browser.as_ref().is_none_or(|b| b.path != s.generation));
    if call.words == ["close"] && (expired || previous.is_none()) {
        data.sessions.remove(&call.name);
        save(&profile, &data)?;
        if expired {
            owner::remove(&call.name);
        }
        call.output(json!({"closed": true}), "✓ Browser closed");
        return passed();
    }
    if expired {
        return Err(stale("The managed CfT generation has changed"));
    }
    let opening_tab = matches!(call.words.as_slice(), ["tab", "new", ..]);
    let background = call.background_tab_new();
    let opening =
        opening_tab || matches!(call.words.first(), Some(&("open" | "goto" | "navigate")));
    if previous.is_none() && crate::legacy::has_binding(&call.name) {
        return Err(usage(
            "Use a fresh name for a dev session; this name already has a legacy binding",
        ));
    }
    if previous.is_none() && !opening {
        return Err(stale("Start the named dev session with open or tab new"));
    }
    if !running {
        data.browser = Some(process::launch(&profile, background)?);
        data.projects.clear();
        data.pending_restore = None;
        save(&profile, &data)?;
    }
    let browser = data.browser.clone().expect("running CfT recorded");
    let mut cdp = Cdp::connect(&browser).map_err(|error| {
        if data.sessions.contains_key(&call.name) {
            stale("The recorded CfT generation is unreachable")
        } else {
            error
        }
    })?;
    let mut record = owner::record_from_env(vega_shim::unix_now());
    let new_session = !data.sessions.contains_key(&call.name);
    if new_session {
        if env::var("VEGA_BROWSER_MARK").as_deref() != Ok("0") {
            groups::require_access(&mut cdp)?;
        }
        let new_project = !data.projects.contains_key(&repo.id);
        if new_project {
            let label = repo
                .location
                .common_dir
                .parent()
                .and_then(Path::file_name)
                .and_then(|n| n.to_str())
                .unwrap_or(repo.id.as_str())
                .to_owned();
            let context =
                cdp.string("Target.createBrowserContext", json!({}), "browserContextId")?;
            data.projects.insert(
                repo.id.clone(),
                Project {
                    label,
                    context,
                    window: 0,
                    origins: Default::default(),
                },
            );
        }
        let project = data.projects.get_mut(&repo.id).expect("project allocated");
        let tab = match create_tab(&mut cdp, &browser, project, &call.name, &[], background) {
            Ok(tab) => tab,
            Err(e) => {
                if new_project {
                    let _ = cdp.call(
                        "Target.disposeBrowserContext",
                        json!({"browserContextId": project.context}),
                    );
                }
                return Err(e);
            }
        };
        data.sessions.insert(
            call.name.clone(),
            Session {
                project: repo.id.clone(),
                generation: browser.path.clone(),
                pane: record.pane.clone(),
                active: 1,
                next: 2,
                tabs: BTreeMap::from([(1, tab)]),
            },
        );
        save(&profile, &data)?;
    }

    let project = data
        .projects
        .get_mut(&repo.id)
        .ok_or_else(|| stale("Missing project context"))?;
    let session = data
        .sessions
        .get_mut(&call.name)
        .expect("session allocated");

    match call.words.as_slice() {
        ["close"] => {
            close_session(&mut data, &call.name, &browser, &mut cdp)?;
            save(&profile, &data)?;
            owner::remove(&call.name);
            call.output(json!({"closed": true}), "✓ Browser closed");
            return passed();
        }
        ["tab", "new", ..] => {
            if !new_session {
                let active = &session.tabs[&session.active];
                cdp.info(project, &active.target)?;
                let mut owned = Vec::new();
                for tab in session.tabs.values() {
                    if let Some(id) = tab.chrome_tab_id {
                        cdp.info(project, &tab.target)?;
                        owned.push(id);
                    }
                }
                if env::var("VEGA_BROWSER_MARK").as_deref() != Ok("0") {
                    groups::require_access(&mut cdp)?;
                }
                let tab = create_tab(&mut cdp, &browser, project, &call.name, &owned, background)?;
                session.active = session.next;
                session.next += 1;
                session.tabs.insert(session.active, tab);
            }
        }
        ["tab", "close", rest @ ..] => {
            let id = rest
                .first()
                .map(|s| tab_id(s))
                .transpose()?
                .unwrap_or(session.active);
            let tab = session
                .tabs
                .get(&id)
                .ok_or_else(|| usage("Tab is not owned by this dev session"))?;
            let target = tab.target.clone();
            release(&mut cdp, project, &target)?;
            session.tabs.remove(&id);
            if session.tabs.is_empty() {
                data.sessions.remove(&call.name);
                if !data
                    .sessions
                    .values()
                    .any(|s| s.project == repo.id && s.generation == browser.path)
                    && dispose_unused(&mut cdp, project)?
                {
                    data.projects.remove(&repo.id);
                }
                owner::remove(&call.name);
            } else if session.active == id {
                session.active = *session.tabs.keys().next().expect("remaining tab");
            }
            save(&profile, &data)?;
            call.output(
                json!({"closed": true, "tabId": format!("t{id}")}),
                format!("✓ Tab [t{id}] closed"),
            );
            return passed();
        }
        ["tab", "list"] | ["tab"] => {
            let mut tabs = Vec::new();
            let mut lines = Vec::new();
            for (id, tab) in &session.tabs {
                let info = cdp.info(project, &tab.target)?;
                let active = *id == session.active;
                let title = info["title"].as_str().unwrap_or_default();
                let url = info["url"].as_str().unwrap_or_default();
                tabs.push(json!({"tabId": format!("t{id}"), "title": title, "url": url, "active": active}));
                lines.push(format!(
                    "{} [t{id}] {title} - {url}",
                    if active { "→" } else { " " }
                ));
            }
            call.output(json!({"tabs": tabs}), lines.join("\n"));
            return passed();
        }
        ["tab", id] => {
            let id = tab_id(id)?;
            let tab = session
                .tabs
                .get(&id)
                .ok_or_else(|| usage("Tab is not owned by this dev session"))?;
            let info = cdp.info(project, &tab.target)?;
            cdp.activate_target(&tab.target)?;
            session.active = id;
            save(&profile, &data)?;
            let url = info["url"].as_str().unwrap_or_default();
            call.output(
                json!({"tabId": format!("t{id}"), "url": url}),
                format!("✓ Switched to tab [t{id}] ({url})"),
            );
            return passed();
        }
        _ => {}
    }

    let tab = session.tabs.get_mut(&session.active).expect("active tab");
    if let Some(path) = call.init_script {
        tab.init_script = Some(std::path::absolute(path).map_err(failure)?);
    }
    session.pane = record.pane.clone();
    let init_script = tab.init_script.clone();
    cdp.info(project, &tab.target)?;
    let binding = proxy::Binding {
        scope: proxy::Scope::Dev(browser, project.clone()),
        target: tab.target.clone(),
    };
    let mut port = tab.port;
    // A dead adapter can only be replaced with the exact same verified target.
    if !proxy::alive(port, &binding) {
        port = proxy::start(&binding)?;
        tab.port = port;
    }
    let active = session.active;
    let total = session.tabs.len();
    let label = project.label.clone();
    record.project = Some(repo.id.to_string());
    owner::upsert(&call.name, record);
    save(&profile, &data)?;
    let focus = matches!(
        call.words.first(),
        Some(
            &("click"
                | "dblclick"
                | "hover"
                | "focus"
                | "type"
                | "fill"
                | "press"
                | "check"
                | "uncheck"
                | "select"
                | "scroll"
                | "scrollintoview"
                | "keyboard"
                | "mouse"
                | "find")
        )
    );
    let _focus_guard = if focus {
        cdp.activate_target(&binding.target)?;
        Some(guard)
    } else {
        drop(guard);
        None
    };

    if opening_tab && call.words.len() == 2 {
        call.output(
            json!({"tabId": format!("t{active}"), "total": total}),
            format!("✓ Tab opened [t{active}] ({total} total)"),
        );
        return passed();
    }
    let script = if call.init_script.is_some() {
        None
    } else {
        init_script.or_else(|| {
            (env::var("VEGA_BROWSER_MARK").as_deref() != Ok("0"))
                .then(|| mark::dev_mark(&call.name, &repo.id, &label))
                .flatten()
        })
    };
    let status = run_agent(&profile, &config, &binding, &call, port, script)?;
    if opening_tab && status.success() {
        call.output(
            json!({"tabId": format!("t{active}"), "total": total}),
            format!("✓ Tab opened [t{active}] ({total} total)"),
        );
    }
    Ok(ExitCode::from(
        status.code().unwrap_or(1).clamp(0, 255) as u8
    ))
}

fn run_agent(
    profile: &Path,
    config: &Path,
    binding: &proxy::Binding,
    call: &Invocation<'_>,
    port: u16,
    script: Option<PathBuf>,
) -> Result<std::process::ExitStatus, Failure> {
    let binary = real::real_binary()
        .ok_or_else(|| failure("Install pinned agent-browser with vega browser install"))?;
    let version = Command::new(&binary)
        .arg("--version")
        .output()
        .map_err(failure)?;
    let pinned = include_str!("../agent-browser.version").trim();
    if !version.status.success()
        || String::from_utf8_lossy(&version.stdout)
            .split_whitespace()
            .last()
            != Some(pinned)
    {
        return Err(failure(format!(
            "Dev attachment requires agent-browser {pinned}"
        )));
    }
    let endpoint = format!("ws://127.0.0.1:{port}/devtools/browser/{}", binding.target);
    let mut cmd = Command::new(binary);
    for (key, _) in env::vars_os() {
        if key.to_string_lossy().starts_with("AGENT_BROWSER_") {
            cmd.env_remove(key);
        }
    }
    cmd.env("AGENT_BROWSER_SOCKET_DIR", profile.join("ab"))
        .arg("--config")
        .arg(config)
        .args([
            "--session",
            &format!(
                "d{}-{port}",
                binding.target.chars().take(16).collect::<String>()
            ),
            "--idle-timeout",
            "5m",
            "--cdp",
            &endpoint,
        ]);
    if let Some(path) = script {
        cmd.arg("--init-script").arg(path);
    }
    if let ["tab", "new", url] = call.words.as_slice() {
        cmd.args(["open", url]).stdout(Stdio::null());
    } else {
        cmd.args(&call.forwarded);
    }
    cmd.status().map_err(failure)
}
