//! One nonfatal vegad poller; approved v1 in docs/design/pagerduty.md.
//! Ownership is local, UAC resolution is cross-host, delivery at-least-once.

use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::Duration;

use async_nats::{jetstream, HeaderMap};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use vega_herdr::notes::{Note, Store};
use vega_registry::View;

const POLL_EVERY: i64 = 15 * 60;
const REMIND_EVERY: i64 = 4 * 3600;
const TIMEOUT: Duration = Duration::from_secs(60);
const AGENT: &str = "tool vega-pagerduty";
const CONFIG: &str = ".vega/pagerduty.toml";
// pdc v0.13.5 internal/agent/detect.go. Agent mode wins over --format json.
const PDC_AGENT_MARKERS: &[&str] = &[
    "CLAUDE_CODE",
    "CLAUDECODE",
    "CURSOR_AGENT",
    "CODEX",
    "OPENAI_CODEX",
    "AIDER",
    "CLINE",
    "WINDSURF_AGENT",
    "GITHUB_COPILOT",
    "AMAZON_Q",
    "AWS_Q_DEVELOPER",
    "GEMINI_CODE_ASSIST",
    "SRC_CODY",
    "FORCE_AGENT_MODE",
];

pub fn spawn() {
    if let Err(e) = thread::Builder::new().name("pagerduty".into()).spawn(|| {
        let mut said = Failures::default();
        let mut system = System::default();
        loop {
            let result = tick(&mut system);
            for line in said.update(result.err().into_iter().collect()) {
                crate::log(&line);
            }
            thread::sleep(Duration::from_secs(60));
        }
    }) {
        crate::log(&format!("pagerduty: thread unavailable: {e}"));
    }
}

fn tick(system: &mut System) -> Result<(), String> {
    let poller = poller_host()?;
    if crate::host() != crate::subject_token(&poller.to_lowercase()) {
        return Ok(());
    }
    let dir = crate::service::state_dir()
        .map_err(|e| e.message)?
        .join("pagerduty");
    fs::create_dir_all(&dir).map_err(|_| "cannot create pagerduty state directory")?;
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700))
        .map_err(|_| "cannot protect pagerduty state directory")?;
    system.dir = dir.clone();
    let result = cycle(&dir.join("state.json"), vega_bd_sync::unix_now(), system);
    if let Err(e) = &result {
        if e.starts_with("state:") {
            system.problem("pagerduty-state", None, e)?;
        }
    }
    result
}

fn poller_host() -> Result<String, String> {
    if let Some(host) = env::var("VEGA_PAGERDUTY_POLLER")
        .ok()
        .filter(|s| !s.trim().is_empty())
    {
        return Ok(host.trim().into());
    }
    let config = env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")));
    let Some(path) = config.map(|p| p.join("vega/pagerduty-poller")) else {
        return Ok("proteus".into());
    };
    match fs::read_to_string(path) {
        Ok(s) if !s.trim().is_empty() => Ok(s.trim().into()),
        Ok(_) => Err("empty pagerduty-poller host file".into()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok("proteus".into()),
        Err(_) => Err("cannot read pagerduty-poller host file".into()),
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
struct Service {
    id: String,
    summary: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
struct Incident {
    id: String,
    incident_number: u64,
    #[serde(default)]
    title: String,
    #[serde(default)]
    summary: String,
    status: String,
    last_status_change_at: String,
    service: Service,
    html_url: String,
    urgency: String,
    created_at: String,
}

fn parse_incidents(bytes: &[u8]) -> Result<Vec<Incident>, String> {
    // Full JSON is intentionally used: --agent may drop fields to fit its budget.
    let value: Value = serde_json::from_slice(bytes).map_err(|_| "pdc: invalid JSON")?;
    let incidents: Vec<Incident> = if value.is_null() {
        Vec::new()
    } else {
        serde_json::from_value(value)
            .map_err(|_| "pdc: expected full incident array with routing fields")?
    };
    if incidents.len() >= 10_000 {
        return Err("pdc: 10000-row pagination cap reached; snapshot may be incomplete".into());
    }
    let mut seen = BTreeSet::new();
    for i in &incidents {
        if i.id.is_empty()
            || !i.id.chars().all(|c| c.is_ascii_alphanumeric())
            || !matches!(i.status.as_str(), "triggered" | "acknowledged")
            || i.service.id.is_empty()
            || i.service.summary.is_empty()
            || i.last_status_change_at.is_empty()
            || i.created_at.is_empty()
            || !i.html_url.starts_with("https://")
            || i.html_url.chars().any(char::is_control)
            || !seen.insert(&i.id)
        {
            return Err("pdc: invalid or duplicate incident in open snapshot".into());
        }
    }
    Ok(incidents)
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
enum Destination {
    Mail {
        project: String,
        host: String,
        pane: String,
    },
    Board {
        project: Option<String>,
        reason: String,
    },
}

impl Destination {
    fn same_recipient(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Board { project: a, .. }, Self::Board { project: b, .. }) => a == b,
            _ => self == other,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct Receipt {
    status: String,
    changed_at: String,
    to: Destination,
    at: i64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct Tracked {
    incident: Incident,
    last_seen: i64,
    delivered: Option<Receipt>,
}

impl Tracked {
    fn due(&self, to: &Destination, now: i64) -> bool {
        self.delivered.as_ref().is_none_or(|d| {
            d.status != self.incident.status
                || d.changed_at != self.incident.last_status_change_at
                || !d.to.same_recipient(to)
                || now.saturating_sub(d.at) >= REMIND_EVERY
        })
    }

    fn reminder(&self, to: &Destination) -> bool {
        self.delivered.as_ref().is_some_and(|d| {
            d.status == self.incident.status
                && d.changed_at == self.incident.last_status_change_at
                && d.to.same_recipient(to)
        })
    }
}

#[derive(Debug, Deserialize, Serialize)]
struct State {
    version: u32,
    attempted: Option<i64>,
    poll_error: Option<String>,
    incidents: BTreeMap<String, Tracked>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            version: 2,
            attempted: None,
            poll_error: None,
            incidents: BTreeMap::new(),
        }
    }
}

impl State {
    fn load(path: &Path) -> Result<Self, String> {
        match fs::read(path) {
            Ok(b) => {
                let s: Self = serde_json::from_slice(&b).map_err(|_| {
                    "state: corrupt pagerduty state; preserve and repair state.json"
                })?;
                if s.version != 2 {
                    return Err("state: unsupported pagerduty state version".into());
                }
                Ok(s)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(_) => Err("state: cannot read pagerduty state.json".into()),
        }
    }

    fn save(&self, path: &Path) -> Result<(), String> {
        let bytes =
            serde_json::to_vec(self).map_err(|_| "state: cannot serialize pagerduty state")?;
        write_private(path, &bytes).map_err(|_| "state: cannot save pagerduty state.json".into())
    }

    fn observe(&mut self, incidents: Vec<Incident>, now: i64) {
        let mut next = BTreeMap::new();
        for incident in incidents {
            let delivered = self
                .incidents
                .remove(&incident.id)
                .and_then(|t| t.delivered);
            next.insert(
                incident.id.clone(),
                Tracked {
                    incident,
                    delivered,
                    last_seen: now,
                },
            );
        }
        self.incidents = next;
    }
}

fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension("tmp");
    let mut f = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&tmp)?;
    f.set_permissions(fs::Permissions::from_mode(0o600))?;
    f.write_all(bytes)?;
    f.sync_all()?;
    fs::rename(tmp, path)
}

#[derive(Clone, Default)]
struct Ownership {
    // Project keys and their service names/IDs. Sets deduplicate within a project.
    projects: BTreeMap<String, BTreeSet<String>>,
    invalid_configs: BTreeMap<String, String>,
    lookup_errors: BTreeMap<String, String>,
    note_errors: BTreeSet<String>,
}

impl Ownership {
    fn report_problems(&mut self, mut report: impl FnMut(&str, &str) -> Result<(), String>) {
        // A failed diagnostic must not discard valid ownership or block unrelated mail.
        let errors: Vec<String> = self
            .problems()
            .filter_map(|(project, error)| {
                report(project, error)
                    .err()
                    .map(|e| format!("ownership {} board note: {e}", clean(project, 250)))
            })
            .collect();
        self.note_errors.extend(errors);
    }

    fn problems(&self) -> impl Iterator<Item = (&String, &String)> {
        self.invalid_configs.iter().chain(self.lookup_errors.iter())
    }

    fn has_problem(&self, key: &str) -> bool {
        self.invalid_configs.contains_key(key) || self.lookup_errors.contains_key(key)
    }

    fn owner(&self, i: &Incident) -> Result<String, String> {
        let owners: Vec<_> = self
            .projects
            .iter()
            .filter(|(_, ss)| ss.contains(&i.service.id) || ss.contains(&i.service.summary))
            .map(|(key, _)| key.clone())
            .collect();
        match owners.as_slice() {
            [owner] if self.invalid_configs.contains_key(owner) => {
                Err("invalid local PagerDuty ownership config; see board notes".into())
            }
            [owner] if self.has_problem(owner) => {
                Err("local PagerDuty ownership unavailable; see board notes".into())
            }
            [] if !self.invalid_configs.is_empty() => {
                Err("invalid local PagerDuty ownership config; see board notes".into())
            }
            [owner] => Ok(owner.clone()),
            [] => Err("unmapped service (v1 reads only projects known on the poller host)".into()),
            _ => Err(format!(
                "service claimed by multiple local projects: {}",
                owners.join(", ")
            )),
        }
    }
}

fn parse_services(text: &str) -> Result<BTreeSet<String>, String> {
    let doc: toml_edit::DocumentMut = text.parse().map_err(|_| "invalid TOML")?;
    if doc.iter().any(|(k, _)| k != "services") {
        return Err("unknown keys; expected services only".into());
    }
    let list = doc
        .get("services")
        .and_then(|s| s.as_array())
        .ok_or("services must be an array of names or IDs")?;
    list.iter()
        .map(|s| {
            s.as_str()
                .filter(|s| !s.trim().is_empty())
                .map(String::from)
                .ok_or_else(|| "each service must be a nonempty string".into())
        })
        .collect()
}

fn git_output(common: &Path, args: &[&str]) -> Result<Vec<u8>, String> {
    let mut cmd = Command::new("git");
    cmd.arg("--git-dir").arg(common).args(args);
    let out = vega_herdr::output_within(&mut cmd, Duration::from_secs(10))
        .map_err(|_| "git unavailable reading PagerDuty ownership")?
        .ok_or("git timeout reading PagerDuty ownership")?;
    if !out.status.success() {
        return Err("git failed reading committed PagerDuty ownership".into());
    }
    Ok(out.stdout)
}

fn read_services(common: &Path) -> Result<BTreeSet<String>, String> {
    // The common dir's HEAD belongs to the main checkout, never a sentinel's HEAD.
    let tree = git_output(common, &["ls-tree", "HEAD", "--", CONFIG])?;
    let tree = String::from_utf8(tree).map_err(|_| "invalid git tree")?;
    if tree.is_empty() {
        return Ok(BTreeSet::new());
    }
    let fields: Vec<_> = tree.split_whitespace().collect();
    if fields.len() != 4 || fields[0] != "100644" || fields[1] != "blob" {
        return Err("PagerDuty ownership must be a regular committed file".into());
    }
    let bytes = git_output(common, &["cat-file", "blob", fields[2]])?;
    let text = String::from_utf8(bytes).map_err(|_| "PagerDuty ownership must be UTF-8")?;
    parse_services(&text)
}

#[derive(Clone)]
struct OwnershipLocation {
    /// Expected ID for provisioned locations/claims; discovered agents resolve
    /// locally, inheriting any registration for the same common directory.
    project: Option<String>,
    path: PathBuf,
}

impl From<String> for OwnershipLocation {
    fn from(path: String) -> Self {
        Self {
            project: None,
            path: vega_herdr::project::metadata_path(None, &path),
        }
    }
}

fn discover() -> Result<Ownership, String> {
    let mut locations: Vec<OwnershipLocation> = vega_bd_sync::store::projects()
        .into_iter()
        .map(|p| OwnershipLocation {
            project: Some(p.key),
            path: p.common,
        })
        .collect();
    locations.extend(vega_herdr::state::project_uac_locations().into_iter().map(
        |(project, location)| OwnershipLocation {
            project: Some(project),
            path: location.common_dir,
        },
    ));
    let agents = vega_herdr::herdr_local(&["agent", "list"])
        .map_err(|_| "cannot enumerate local agents for PagerDuty ownership")?;
    let workspaces = vega_herdr::herdr_local(&["workspace", "list"])
        .map_err(|_| "cannot enumerate local workspaces for PagerDuty ownership")?;
    if !agents["result"]["agents"].is_array() || !workspaces["result"]["workspaces"].is_array() {
        return Err("unknown herdr schema discovering PagerDuty ownership".into());
    }
    locations.extend(
        agent_projects(&agents, &workspaces)
            .into_iter()
            .map(OwnershipLocation::from),
    );
    Ok(read_ownership(locations, |path| fs::canonicalize(path)))
}

fn read_ownership<T: Into<OwnershipLocation>>(
    locations: impl IntoIterator<Item = T>,
    canonicalize: impl Fn(&Path) -> std::io::Result<PathBuf>,
) -> Ownership {
    let mut out = Ownership::default();
    let mut seen = BTreeSet::new();
    let locations: Vec<OwnershipLocation> = locations.into_iter().map(Into::into).collect();
    let mut expected = BTreeMap::new();
    for location in &locations {
        if let Some(id) = &location.project {
            if let Some(prior) = expected.insert(location.path.clone(), id.clone()) {
                if prior != *id {
                    for id in [prior, id.clone()] {
                        out.lookup_errors.insert(
                            id,
                            "conflicting registered identity for local location".into(),
                        );
                    }
                }
            }
        }
    }
    for source in locations {
        let local = source.path;
        if !local.is_dir() {
            if let Some(id) = source.project {
                out.lookup_errors
                    .insert(id, "registered project location is unavailable".into());
            }
            continue;
        }
        let resolved = match vega_herdr::project::resolve(&local, &vega_herdr::state::post_host()) {
            Ok(p) => p,
            Err(e) => {
                let key = source
                    .project
                    .unwrap_or_else(|| format!("location:{}", local.display()));
                out.lookup_errors
                    .insert(key, format!("{}: {}", e.code, e.message));
                continue;
            }
        };
        let registered = source
            .project
            .or_else(|| expected.get(&resolved.location.common_dir).cloned());
        if let Some(id) = &registered {
            if id != resolved.id.as_str() {
                out.lookup_errors.insert(
                    id.clone(),
                    "registered project identity changed; journaled rekey required".into(),
                );
                continue;
            }
        }
        let key = registered.unwrap_or_else(|| resolved.id.to_string());
        let common = match canonicalize(&resolved.location.common_dir) {
            Ok(common) => common,
            Err(_) => {
                out.lookup_errors
                    .insert(key, "cannot canonicalize known project".into());
                continue;
            }
        };
        if !seen.insert(common.clone()) {
            continue;
        }
        match read_services(&common) {
            Ok(services) => {
                if out
                    .projects
                    .get(&key)
                    .is_some_and(|prior| prior != &services)
                {
                    out.invalid_configs.insert(key.clone(), "clones of the same project have divergent committed PagerDuty ownership; make their service sets agree".into());
                } else {
                    out.projects.insert(key, services);
                }
            }
            Err(e) => {
                out.invalid_configs.insert(key, e);
            }
        }
    }
    out
}

fn agent_projects(agents: &Value, workspaces: &Value) -> Vec<String> {
    agents["result"]["agents"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|a| {
            let ws = workspaces["result"]["workspaces"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|w| w["workspace_id"] == a["workspace_id"]);
            let cwd = a["cwd"]
                .as_str()
                .filter(|s| !s.is_empty())
                .or_else(|| a["repo_key"].as_str())
                .unwrap_or_default();
            let path = vega_herdr::project::metadata_path(ws, cwd);
            (!path.as_os_str().is_empty()).then(|| path.display().to_string())
        })
        .collect()
}

fn destination(i: &Incident, ownership: &Ownership, view: &View) -> Destination {
    match ownership.owner(i) {
        Err(reason) => Destination::Board {
            project: None,
            reason,
        },
        Ok(project) => match crate::reach::resolve(view, &project, None).map_err(|e| e.message) {
            Ok((host, pane, _)) => Destination::Mail {
                project,
                host,
                pane,
            },
            Err(reason) => Destination::Board {
                project: Some(project),
                reason,
            },
        },
    }
}

fn clean(s: &str, max: usize) -> String {
    s.chars()
        .filter(|c| !c.is_control() && *c != '\u{202e}' && *c != '\u{202d}')
        .take(max)
        .collect()
}

fn mail_text(i: &Incident, to: &Destination, reminder: bool) -> String {
    let (project, fallback) = match to {
        Destination::Mail { project, .. } => (project.as_str(), String::new()),
        Destination::Board { project, reason } => (
            project.as_deref().unwrap_or("unmapped"),
            format!("\nBoard fallback: {}", clean(reason, 700)),
        ),
    };
    let marker = if reminder { "reminder, still " } else { "" };
    let title = if i.title.is_empty() {
        &i.summary
    } else {
        &i.title
    };
    format!("pagerduty: #{} {} {marker}{} ({}) — {}\nService: {} ({}); project: {}\n{}\nCreated: {}\nYou may investigate, acknowledge and add notes. Resolve, escalate or reassign only after the Slayer approves a board decision. Use vega secrets run --for pdc -- pdc ...{fallback}",
        i.incident_number, clean(&i.id, 80), i.status, clean(&i.urgency, 30), clean(title, 800),
        clean(&i.service.summary, 160), clean(&i.service.id, 80), clean(project, 250), clean(&i.html_url, 500), clean(&i.created_at, 80))
}

trait Ops {
    fn poll(&mut self) -> Result<Vec<Incident>, String>;
    fn ownership(&mut self) -> Result<Ownership, String>;
    fn registry(&mut self) -> Result<View, String>;
    fn deliver(
        &mut self,
        to: &Destination,
        i: &Incident,
        text: &str,
        now: i64,
    ) -> Result<(), String>;
    fn clear_notes(&mut self, state: &State, now: i64);
}

fn cycle(path: &Path, now: i64, ops: &mut impl Ops) -> Result<(), String> {
    let mut state = State::load(path)?;
    if state
        .attempted
        .is_none_or(|t| now.saturating_sub(t) >= POLL_EVERY)
    {
        state.attempted = Some(now);
        state.save(path)?; // A failed command or crash cannot start a tight poll loop.
        match ops.poll() {
            Ok(incidents) => {
                state.observe(incidents, now);
                state.poll_error = None;
            }
            Err(e) => {
                state.poll_error = Some(e);
            }
        }
        state.save(path)?;
    }
    if let Some(error) = state.poll_error {
        return Err(error);
    }
    let ownership = ops.ownership()?;
    // Unmapped/conflicting incidents do not require a working registry to reach the board.
    let needs_registry = state
        .incidents
        .values()
        .any(|t| ownership.owner(&t.incident).is_ok());
    let registry = if needs_registry {
        ops.registry()
    } else {
        Ok(View::default())
    };
    let mut errors: BTreeSet<String> = ownership
        .problems()
        .map(|(project, error)| format!("ownership {}: {error}", clean(project, 250)))
        .chain(ownership.note_errors.iter().cloned())
        .collect();
    for tracked in state.incidents.values_mut() {
        let view = match &registry {
            Ok(v) => v,
            Err(e) if ownership.owner(&tracked.incident).is_ok() => {
                errors.insert(e.clone());
                continue;
            }
            Err(_) => &View::default(),
        };
        let to = destination(&tracked.incident, &ownership, view);
        if !tracked.due(&to, now) {
            continue;
        }
        let text = mail_text(&tracked.incident, &to, tracked.reminder(&to));
        match ops.deliver(&to, &tracked.incident, &text, now) {
            Ok(()) => {
                tracked.delivered = Some(Receipt {
                    status: tracked.incident.status.clone(),
                    changed_at: tracked.incident.last_status_change_at.clone(),
                    to,
                    at: now,
                });
            }
            Err(e) => {
                errors.insert(e);
            }
        }
    }
    state.save(path)?; // Failure here permits a rare duplicate, not a lost notification.
    ops.clear_notes(&state, now);
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.into_iter().collect::<Vec<_>>().join("; "))
    }
}

fn problem_note(
    store: &Store,
    cause: &str,
    project: Option<&str>,
    body: &str,
) -> Result<(), String> {
    if store.list().iter().any(|n| {
        let same = n.cause.as_deref() == Some(cause) && n.body == body;
        let migrated_config = project.is_some()
            && n.project.as_deref() == project
            && cause.starts_with("pagerduty-config:")
            && n.cause
                .as_deref()
                .is_some_and(|c| c.starts_with("pagerduty-config:"))
            && n.body
                .split_once(": ")
                .zip(body.split_once(": "))
                .is_some_and(|((_, previous), (_, current))| previous == current);
        n.resolved.is_none() && (same || migrated_config)
    }) {
        return Ok(());
    }
    store
        .add(
            Note {
                host: crate::host(),
                kind: "pagerduty".into(),
                project: project.map(String::from),
                cause: Some(cause.into()),
                title: "PagerDuty poller needs attention".into(),
                body: body.into(),
                ..Default::default()
            },
            vega_herdr::notes::now_ms(),
        )
        .map_err(|_| "cannot persist PagerDuty board note")?;
    Ok(())
}

#[derive(Default)]
struct System {
    dir: PathBuf,
    client: Option<async_nats::Client>,
    runtime: Option<tokio::runtime::Runtime>,
}

impl System {
    fn problem(&self, cause: &str, project: Option<&str>, body: &str) -> Result<(), String> {
        let store = Store::local().ok_or("PagerDuty board note store unavailable")?;
        problem_note(&store, cause, project, body)
    }

    fn broker(&mut self) -> Result<(&tokio::runtime::Runtime, async_nats::Client), String> {
        if self.runtime.is_none() {
            self.runtime = Some(crate::runtime().map_err(|_| "pagerduty: runtime unavailable")?);
        }
        let rt = self
            .runtime
            .as_ref()
            .ok_or("pagerduty: runtime unavailable")?;
        if self.client.is_none() {
            self.client = Some(rt.block_on(async {
                tokio::time::timeout(
                    Duration::from_secs(6),
                    crate::connect_as("pagerduty", crate::Seed::Host, None),
                )
                .await
                .map_err(|_| "pagerduty: broker connection timeout".to_string())?
                .map_err(|_| "pagerduty: broker connection unavailable".to_string())
            })?);
        }
        Ok((
            rt,
            self.client
                .as_ref()
                .ok_or("pagerduty: broker unavailable")?
                .clone(),
        ))
    }
}

impl Ops for System {
    fn poll(&mut self) -> Result<Vec<Incident>, String> {
        let store =
            vega_secrets::Store::from_env().map_err(|_| "pdc: secrets store unavailable")?;
        let (vars, _) = store
            .env_for("pdc")
            .map_err(|_| "pdc: cannot decrypt secrets")?;
        let vars: Vec<(&str, &str)> = vars
            .iter()
            .map(|(key, value)| (key.as_str(), value.expose()))
            .collect();
        run_pdc(Path::new("pdc"), &self.dir, &vars, TIMEOUT)
    }

    fn ownership(&mut self) -> Result<Ownership, String> {
        let mut ownership = discover()?;
        ownership.report_problems(|project, error| {
            self.problem(
                &format!("pagerduty-config:{project}"),
                (!project.starts_with("location:")).then_some(project),
                &format!("{}: {error}", clean(project, 250)),
            )
        });
        if let Some(store) = Store::local() {
            store.resolve(vega_bd_sync::unix_now(), |n| {
                n.cause.as_deref().is_some_and(|c| {
                    c.strip_prefix("pagerduty-config:").is_some_and(|key| {
                        !ownership.has_problem(n.project.as_deref().unwrap_or(key))
                    })
                })
            });
        }
        Ok(ownership)
    }

    fn registry(&mut self) -> Result<View, String> {
        let (rt, client) = self.broker()?;
        rt.block_on(async {
            tokio::time::timeout(crate::reach::WAIT, crate::reach::read(&client))
                .await
                .map_err(|_| "pagerduty: registry timeout".to_string())?
                .map_err(|_| "pagerduty: registry unavailable".to_string())
        })
    }

    fn deliver(
        &mut self,
        to: &Destination,
        i: &Incident,
        text: &str,
        now: i64,
    ) -> Result<(), String> {
        match to {
            Destination::Mail { host, pane, .. } => {
                let (rt, client) = self.broker()?;
                rt.block_on(async {
                    tokio::time::timeout(
                        Duration::from_secs(10),
                        publish_mail(client, host, pane, text),
                    )
                    .await
                    .map_err(|_| "pagerduty: mail acceptance timeout".to_string())?
                })
            }
            Destination::Board { .. } => {
                let store = Store::local().ok_or("PagerDuty board note store unavailable")?;
                board_note(&store, &crate::host(), to, i, text, now)?;
                Ok(())
            }
        }
    }

    fn clear_notes(&mut self, state: &State, now: i64) {
        if let Some(store) = Store::local() {
            resolve_notes(&store, state, now);
        }
    }
}

fn board_note(
    store: &Store,
    host: &str,
    to: &Destination,
    i: &Incident,
    text: &str,
    now: i64,
) -> Result<(), String> {
    let Destination::Board { project, .. } = to else {
        return Err("expected board destination".into());
    };
    store
        .add(
            Note {
                host: host.into(),
                kind: "pagerduty".into(),
                project: project.clone(),
                cause: Some(format!("pagerduty:{}", i.id)),
                title: text.lines().next().unwrap_or("PagerDuty").into(),
                body: text.into(),
                ..Default::default()
            },
            now.max(0) as u64 * 1000,
        )
        .map_err(|_| "pagerduty: cannot persist incident board note")?;
    Ok(())
}

fn resolve_notes(store: &Store, state: &State, now: i64) {
    store.resolve(now, |n| {
        if n.cause.as_deref() == Some("pagerduty-state") {
            return true;
        }
        n.cause
            .as_deref()
            .and_then(|c| c.strip_prefix("pagerduty:"))
            .is_some_and(|id| {
                state.incidents.get(id).is_none_or(|t| {
                    t.delivered
                        .as_ref()
                        .is_some_and(|d| matches!(d.to, Destination::Mail { .. }))
                })
            })
    });
}

fn run_pdc(
    bin: &Path,
    dir: &Path,
    vars: &[(&str, &str)],
    timeout: Duration,
) -> Result<Vec<Incident>, String> {
    if !vars
        .iter()
        .any(|(key, value)| *key == "PDC_TOKEN" && !value.trim().is_empty())
    {
        return Err("pdc: PDC_TOKEN secret unavailable on this host".into());
    }
    let config = dir.join("pdc.toml");
    write_private(&config, b"config_version = 1\n")
        .map_err(|_| "pdc: cannot write isolated config")?;
    let mut cmd = Command::new(bin);
    cmd.current_dir(dir)
        .args(["--format", "json", "--color", "never", "--config"])
        .arg(config)
        .args([
            "incident",
            "list",
            "--all",
            "--status",
            "triggered",
            "--status",
            "acknowledged",
        ])
        .envs(vars.iter().copied())
        .env("PDC_SERVICE", "")
        .env("PDC_TEAM", "")
        .env("PDC_DEBUG", "0");
    for marker in PDC_AGENT_MARKERS {
        cmd.env_remove(marker);
    }
    let out = vega_herdr::output_within(&mut cmd, timeout)
        .map_err(|_| "pdc: executable unavailable")?
        .ok_or("pdc: incident list timed out")?;
    // Never echo stderr: it may contain secret values or volatile HTTP diagnostics.
    if !out.status.success() {
        return Err(
            "pdc: incident list failed; check credentials, API availability and rate limit".into(),
        );
    }
    parse_incidents(&out.stdout)
}

async fn publish_mail(
    client: async_nats::Client,
    host: &str,
    pane: &str,
    text: &str,
) -> Result<(), String> {
    let mut headers = HeaderMap::new();
    headers.insert("Nats-Msg-Id", nuid::next().as_str());
    headers.insert(crate::h::FROM_AGENT, AGENT);
    headers.insert(crate::h::FROM_CWD, "vegad");
    headers.insert(crate::h::SENT_MS, crate::now_ms().to_string().as_str());
    jetstream::new(client)
        .publish_with_headers(
            crate::inbox_subject(&crate::subject_token(host), pane, &crate::host()),
            headers,
            text.to_string().into(),
        )
        .await
        .map_err(|_| "pagerduty: mail publish failed")?
        .await
        .map_err(|_| "pagerduty: mail not durably accepted")?;
    Ok(())
}

#[derive(Default)]
struct Failures(BTreeSet<String>);

impl Failures {
    fn update(&mut self, errors: BTreeSet<String>) -> Vec<String> {
        let mut lines: Vec<_> = errors
            .difference(&self.0)
            .map(|e| format!("pagerduty: {e}"))
            .collect();
        if !self.0.is_empty() && errors.is_empty() {
            lines.push("pagerduty: recovered".into());
        }
        self.0 = errors;
        lines
    }
}

#[cfg(test)]
mod tests;
