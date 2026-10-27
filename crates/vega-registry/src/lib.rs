//! The cross-machine registry (vegad M4, docs/design/vegad.md, vega-lhc.12).
//!
//! Each host's local state is the truth for its own panes: herdr knows its
//! agents, `~/.local/state/vega` holds lineage (parents, done, silent,
//! replied) and UAC registrations. vegad on every host reconciles that into
//! the NATS KV bucket VEGA_REGISTRY on titan, under keys that start with the
//! host's own token (the broker lets a host write only those). Readers (vega
//! uac, the parent's vegad) see every host at once; when the broker is away
//! they fall back to local state and say so.
//!
//! Keys (KV allows `[A-Za-z0-9_=/.-]`):
//! - `<host>.beat` = {"schema": 2, "at": unix, human_at?}: the host's vegad ran a reconcile; entries
//!   of a host without a fresh beat are not trusted (absence proves nothing). `human_at`:
//!   the host's last herdr pane focus (`uber_engaged`).
//! - `<host>.agent.<pane>` = {pane, name, kind, status, cwd, repo_key,
//!   waiting?}: `waiting` = {status, since} while the agent waits for the
//!   Slayer (vega_herdr::waiting).
//! - `<host>.link.<child host>.<pane>` = {parent, child_host, pane, name,
//!   done, summary, silent, replied}: a child this host's agent spawned or
//!   adopted, also on another machine.
//! - `<host>.uac.v2.<hex(id)>` = {schema: 2, pane, host, project, at,
//!   origin_name?, folder_name?, aliases?}: the project's UAC.
//! - `<host>.note.<id>` = a notification vega sent the human on that host
//!   (vega_herdr::notes); `read` lists these keys but fetches only the
//!   other hosts' notes this host's viewer has not read (the badges need
//!   their project and resolved state); `read_notes` (vega post notes) all.
//! - `<host>.notes.seen.<source>` = {read, cleared}: what the viewer on
//!   `<host>` read of `<source>`'s notes (last ids).
//! - `<host>.janitor` = {at, rows}: the host's checkouts (vega janitor's
//!   `summary.json`, docs/design/janitor.md); `read` lists but does not
//!   fetch it (~70 KB), vegad pulls it every few minutes into a file.
//! - `<host>.theme` = {mode, source, at}: the dark/light mode vega theme last
//!   applied there; headless hosts follow their control host's (vega-lhc.86).
//! - `<host>.lanista.v2.<project>.<seat>.<lane>.<tier>` = runtime model pins;
//!   components are hex-encoded, managed by `pins`, independent of vegad.
//! - `<host>.uber.v1` = {schema: 1, pane, host, at, epoch, engaged_at?}: the Uber-UAC
//!   claim (docs/design/uber-uac.md §2); `View::live_uber` picks the live one,
//!   `View::uber_engaged` says whether it is engaged.
//!
//! Inbox-only hosts may hold a read grant for single keys (`get_one`, one
//! direct get each, vega-lhc.90) and nothing else.
//!
//! Only this crate talks to the bucket; mux, uac and the relay call it.

pub mod cutover;
pub mod pins;
pub mod uber_asks;
pub mod uber_engaged;
pub mod uber_state;

use std::collections::BTreeMap;
use std::env;
use std::time::Duration;

use futures_util::StreamExt;
use serde_json::{json, Value};
use vega_herdr::state;

pub const BUCKET: &str = "VEGA_REGISTRY";
/// A host's entries count only while its beat is younger than this.
pub const FRESH: Duration = Duration::from_secs(90);

/// A KV key token: pane ids and paths contain `:`, `/` and `~`.
pub fn token(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

pub fn beat_key(host: &str) -> String {
    format!("{}.beat", token(host))
}

pub fn agent_key(host: &str, pane: &str) -> String {
    format!("{}.agent.{}", token(host), token(pane))
}

pub fn link_key(host: &str, child_host: &str, pane: &str) -> String {
    format!("{}.link.{}.{}", token(host), token(child_host), token(pane))
}

pub fn uac_key(host: &str, project: &str) -> String {
    format!(
        "{}.uac.v2.{}",
        token(host),
        vega_herdr::project::encode_component(project)
    )
}

/// Legacy project keys belong exclusively to the backed-up cutover plan.
/// Ordinary reconciliation must never garbage-collect them.
pub fn is_legacy_project_key(key: &str) -> bool {
    let parts: Vec<_> = key.split('.').collect();
    matches!(parts.get(1), Some(&"uac" | &"lanista"))
        && !(parts.get(2) == Some(&"v2") && parts.len() > 3)
}

/// Runtime wait IDs carry the coordinator host, never a project routing ID.
pub fn wait_key(host: &str, id: &str) -> String {
    format!(
        "{}.wait.{}",
        token(host),
        vega_herdr::project::encode_component(id)
    )
}
pub fn ask_key(host: &str, thread: &str) -> String {
    format!(
        "{}.ask.{}",
        token(host),
        vega_herdr::project::encode_component(thread)
    )
}

pub fn note_key(host: &str, id: u64) -> String {
    format!("{}.note.{id}", token(host))
}

pub fn seen_key(host: &str, source: &str) -> String {
    format!("{}.notes.seen.{}", token(host), token(source))
}

pub fn janitor_key(host: &str) -> String {
    format!("{}.janitor", token(host))
}

/// A host's janitor summary: large and rarely read, so `read` skips it.
pub fn is_janitor(key: &str) -> bool {
    key.split('.').nth(1) == Some("janitor")
}

/// The Uber-UAC claim of `host`: one key per host, one live claim estate-wide.
pub fn uber_key(host: &str) -> String {
    format!("{}.uber.v1", token(host))
}

/// One `<host>.uber.v1` record (docs/design/uber-uac.md §2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UberClaim {
    pub pane: String,
    pub host: String,
    /// Claim time in ms; a new claim always gets a new one.
    pub epoch: u64,
}

impl UberClaim {
    /// Winner order: the highest epoch, ties to the lower host name.
    fn beats(&self, other: &Self) -> bool {
        (self.epoch, std::cmp::Reverse(token(&self.host)))
            > (other.epoch, std::cmp::Reverse(token(&other.host)))
    }
}

pub fn theme_key(host: &str) -> String {
    format!("{}.theme", token(host))
}

/// A note key's (host, id); `None` for every other key.
pub fn parse_note_key(key: &str) -> Option<(&str, u64)> {
    let mut t = key.splitn(3, '.');
    let host = t.next()?;
    (t.next()? == "note").then_some(())?;
    Some((host, t.next()?.parse().ok()?))
}

/// Notes and read marks: many keys nobody needs for the state, so `read`
/// lists them but does not fetch them.
pub fn is_note(key: &str) -> bool {
    matches!(key.split('.').nth(1), Some("note" | "notes"))
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// This host's desired entries (without the beat), from herdr's agent and
/// workspace lists, local state records and local Git origin configuration.
pub fn local_entries(
    host: &str,
    agents: &[Value],
    workspaces: &[Value],
) -> BTreeMap<String, Value> {
    let mut out = BTreeMap::new();
    let waiting = vega_herdr::waiting::read();
    for a in agents {
        let Some(pane) = a["pane_id"].as_str() else {
            continue;
        };
        let workspace = workspaces
            .iter()
            .find(|w| w["workspace_id"] == a["workspace_id"]);
        let repo_key = workspace
            .and_then(|w| w["worktree"]["repo_key"].as_str())
            .unwrap_or("");
        let mut entry = json!({
            "pane": pane,
            "name": vega_herdr::agent_name(a),
            "kind": a["agent"].as_str().unwrap_or(""),
            "status": a["agent_status"].as_str().unwrap_or("unknown"),
            // herdr counts finished turns; a reader that polls every 15 s
            // still sees a short turn by this number rising.
            "turns": a["completion_seq"].as_u64().unwrap_or(0),
            "cwd": vega_herdr::collapse_home(a["cwd"].as_str().unwrap_or("")),
            "repo_key": vega_herdr::collapse_home(repo_key),
        });
        // Resolve only this host's local checkout. Paths remain metadata.
        let path = workspace
            .and_then(|w| {
                w["worktree"]["checkout_path"]
                    .as_str()
                    .filter(|p| !p.is_empty())
            })
            .or_else(|| (!repo_key.is_empty()).then_some(repo_key))
            .unwrap_or_else(|| a["cwd"].as_str().unwrap_or(""));
        let owner_path = match path.strip_prefix("~/") {
            Some(rest) => env::var_os("HOME")
                .map(|h| std::path::PathBuf::from(h).join(rest))
                .unwrap_or_else(|| path.into()),
            None => path.into(),
        };
        static PROJECT_ERRORS: std::sync::OnceLock<std::sync::Mutex<BTreeMap<String, String>>> =
            std::sync::OnceLock::new();
        let errors = PROJECT_ERRORS.get_or_init(Default::default);
        match vega_herdr::project::resolve(&owner_path, host) {
            Ok(project) => {
                if let Ok(mut logged) = errors.lock() {
                    logged.remove(path);
                }
                entry["project"] = json!(project.id);
                entry["schema"] = json!(2);
            }
            Err(e) => {
                if let Ok(mut logged) = errors.lock() {
                    if logged.get(path) != Some(&e.code) {
                        eprintln!("registry project skipped: {}: {}", pane, e.code);
                        logged.insert(path.to_owned(), e.code);
                    }
                }
            }
        }
        // Since when it waits for the Slayer (vega mux next).
        if let Some(w) = waiting.entry(pane) {
            entry["waiting"] = w;
        }
        out.insert(agent_key(host, pane), entry);
    }
    for c in state::children() {
        let m = c.machine.as_deref();
        let child_host = m.map_or_else(|| host.to_string(), str::to_lowercase);
        let done = state::done_of(m, &c.pane);
        out.insert(
            link_key(host, &child_host, &c.pane),
            json!({
                "parent": c.parent.pane,
                "parent_host": host,
                "child_host": child_host,
                "pane": c.pane,
                "name": c.name,
                "done": done.as_ref().map(|d| d.outcome.as_str()),
                "summary": done.map(|d| d.summary),
                "silent": state::is_silent(m, &c.pane),
                "question": state::silence_of(m, &c.pane).and_then(|s| s.question),
                "quota": state::quota_json(m, &c.pane),
                "replied": state::has_replied(m, &c.pane),
            }),
        );
    }
    for (project, _, _) in state::project_uac_records() {
        let Some(file) = state::project_file("uac", &project) else {
            continue;
        };
        let Ok(text) = std::fs::read_to_string(file) else {
            continue;
        };
        let Ok(record) = serde_json::from_str::<Value>(&text) else {
            continue;
        };
        if record["schema"] != 2 {
            continue;
        }
        if let Ok(entry) = uac_entry(host, &record) {
            out.insert(uac_key(host, &project), entry);
        }
    }
    // The Uber-UAC claim file (vega uber claim); its absence deletes the key.
    if let Some(claim) = uber_state::file().and_then(|f| uber_state::published(&f, host)) {
        out.insert(uber_key(host), claim);
    }
    if let Some(store) = vega_herdr::notes::Store::local() {
        for n in store.list() {
            out.insert(note_key(host, n.id), n.to_json());
        }
        let seen = store.seen();
        for (source, read) in &seen.read {
            out.insert(
                seen_key(host, source),
                json!({"read": read, "cleared": seen.cleared.get(source)}),
            );
        }
    }
    let wait_records = state_wait_records(host);
    let open_asks = vega_herdr::gather::open();
    out.insert(format!("{}.orchestration",token(host)),json!({"at":unix_now(),"waits_available":wait_records.is_ok(),"asks_available":open_asks.is_ok()}));
    if let Ok(waits) = wait_records {
        for w in waits.into_iter().filter(|w| w.resolved.is_none()) {
            if let Ok(value) = serde_json::to_value(&w) {
                out.insert(wait_key(host, &w.id), value);
            }
        }
    }
    if let Ok(asks) = open_asks {
        for (thread, ask) in asks {
            out.insert(ask_key(host,&thread),json!({"host":host,"thread":thread,"ask":serde_json::from_str::<Value>(&ask.to_json()).unwrap_or(Value::Null)}));
        }
    }
    out
}

/// Folder alias from an owner-local common directory; never an identity.
pub fn project_folder(common: &str) -> &str {
    let dir = common.trim_end_matches('/');
    let dir = dir
        .strip_suffix("/.git")
        .or_else(|| dir.strip_suffix("/.bare"))
        .unwrap_or(dir);
    dir.rsplit('/').next().unwrap_or(dir)
}

fn state_wait_records(host: &str) -> Result<Vec<vega_herdr::waits::Wait>, String> {
    let file = vega_herdr::waits::path().ok_or("wait source unavailable")?;
    let waits = vega_herdr::waits::Store::read(&file).map_err(|_| "wait source unavailable")?;
    if waits.iter().any(|w| w.coordinator.host != host) {
        return Err("wait coordinator host changed; explicit recovery required".into());
    }
    Ok(waits)
}

/// Read the origin locally: readers must never inspect their own filesystem
/// to interpret another host's key. Publish only the name, never credentials.
pub fn project_origin_name(project: &str) -> Option<String> {
    let dir = match project.strip_prefix("~/") {
        Some(rest) => std::path::PathBuf::from(env::var_os("HOME")?).join(rest),
        None => std::path::PathBuf::from(project),
    };
    let out = vega_herdr::output_within(
        std::process::Command::new("git")
            .arg("--git-dir")
            .arg(dir)
            .args(["config", "--get", "remote.origin.url"])
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_OPTIONAL_LOCKS", "0"),
        Duration::from_secs(2),
    )
    .ok()??;
    if !out.status.success() {
        return None;
    }
    origin_name(std::str::from_utf8(&out.stdout).ok()?).map(String::from)
}

/// HTTPS, SSH URL, SCP-style SSH and local-path origins share a final name.
fn origin_name(url: &str) -> Option<&str> {
    let url = url.trim().trim_end_matches('/');
    if url.contains(['?', '#']) {
        return None;
    }
    // Never mistake a credential-bearing URL authority for a repository name.
    let path = match url.split_once("://") {
        Some((_, authority_path)) => authority_path.split_once('/')?.1,
        None => url,
    };
    let name = path.rsplit(['/', ':']).next()?;
    let name = name.strip_suffix(".git").unwrap_or(name);
    (!name.is_empty() && name != "." && name != "..").then_some(name)
}

/// Everything in the bucket, by key.
#[derive(Debug, Default, Clone)]
pub struct View {
    pub entries: BTreeMap<String, Value>,
    /// Every key in the bucket, also the read notes `entries` leaves out.
    pub keys: Vec<String>,
}

impl View {
    /// Hosts whose vegad reconciled within FRESH.
    pub fn fresh(&self, host: &str) -> bool {
        self.entries
            .get(&beat_key(host))
            .and_then(|v| v["at"].as_i64())
            .is_some_and(|at| unix_now() - at <= FRESH.as_secs() as i64)
    }

    fn of_kind<'a>(&'a self, kind: &'a str) -> impl Iterator<Item = (&'a str, &'a Value)> + 'a {
        self.entries.iter().filter_map(move |(k, v)| {
            let mut t = k.splitn(3, '.');
            let host = t.next()?;
            (t.next()? == kind).then_some((host, v))
        })
    }

    /// When the host's vegad last reconciled (Unix seconds).
    pub fn beat_at(&self, host: &str) -> Option<i64> {
        self.entries
            .get(&beat_key(host))
            .and_then(|v| v["at"].as_i64())
    }

    /// An agent entry on a fresh host.
    pub fn agent(&self, host: &str, pane: &str) -> Option<&Value> {
        self.fresh(host)
            .then(|| self.entries.get(&agent_key(host, pane)))?
    }

    /// (host, agent entry) of fresh hosts.
    pub fn agents(&self) -> Vec<(String, Value)> {
        self.of_kind("agent")
            .filter(|(h, _)| self.fresh(h))
            .map(|(h, v)| (h.to_string(), v.clone()))
            .collect()
    }

    /// Agents of fresh hosts that wait for the Slayer (vega mux next).
    pub fn waiting(&self) -> Vec<vega_herdr::waiting::Candidate> {
        self.agents()
            .into_iter()
            .filter_map(|(host, a)| {
                let w = &a["waiting"];
                Some(vega_herdr::waiting::Candidate {
                    pane: a["pane"].as_str()?.into(),
                    name: a["name"].as_str().unwrap_or("").into(),
                    status: w["status"].as_str()?.into(),
                    since: w["since"].as_i64()?,
                    host,
                })
            })
            .collect()
    }

    /// An agent's status on a fresh host: `None` when the host is stale or
    /// unknown (absence proves nothing), `Some(None)` when the host is fresh
    /// and the pane is gone.
    pub fn status(&self, host: &str, pane: &str) -> Option<Option<String>> {
        if !self.fresh(host) {
            return None;
        }
        Some(
            self.entries
                .get(&agent_key(host, pane))
                .and_then(|v| v["status"].as_str())
                .map(String::from),
        )
    }

    /// Lineage links from every host (also stale ones: lineage is a record).
    pub fn links(&self) -> Vec<Value> {
        self.of_kind("link").map(|(_, v)| v.clone()).collect()
    }

    /// (host, id) of every note in the bucket, without its body.
    pub fn notes(&self) -> Vec<(String, u64)> {
        self.keys
            .iter()
            .filter_map(|k| parse_note_key(k))
            .map(|(h, id)| (h.to_string(), id))
            .collect()
    }

    /// The other hosts' notes `read` fetched: those this host's viewer had
    /// not read (a deleted note's key has no body).
    pub fn unread_notes(&self) -> Vec<vega_herdr::notes::Note> {
        self.entries
            .iter()
            .filter(|(k, _)| parse_note_key(k).is_some())
            .filter_map(|(_, v)| vega_herdr::notes::Note::from_json(v))
            .collect()
    }

    /// The mode (`dark`/`light`) a fresh host last applied (vega theme).
    pub fn theme(&self, host: &str) -> Option<&str> {
        self.fresh(host)
            .then(|| self.entries.get(&theme_key(host)))??["mode"]
            .as_str()
    }

    /// Runtime declarations, retaining stale records as evidence with unknown freshness.
    pub fn explicit_waits(&self) -> Result<Vec<(vega_herdr::waits::Wait, bool)>, String> {
        let mut out = Vec::new();
        for (key, v) in &self.entries {
            let mut parts = key.splitn(3, '.');
            let Some(host) = parts.next() else {
                continue;
            };
            if parts.next() != Some("wait") {
                continue;
            }
            let w: vega_herdr::waits::Wait =
                serde_json::from_value(v.clone()).map_err(|_| "invalid wait projection")?;
            if !vega_herdr::waits::valid(&w)
                || w.coordinator.host != host
                || key != &wait_key(host, &w.id)
            {
                return Err("unverified wait projection owner".into());
            }
            out.push((w, self.fresh(host)));
        }
        Ok(out)
    }
    /// Authenticated publisher host plus its unresolved ask. Never a read-ack inference.
    pub fn open_asks(
        &self,
    ) -> Result<Vec<(String, String, vega_herdr::gather::Ask, bool)>, String> {
        let mut out = Vec::new();
        for (key, v) in &self.entries {
            let mut parts = key.splitn(3, '.');
            let Some(host) = parts.next() else {
                continue;
            };
            if parts.next() != Some("ask") {
                continue;
            }
            let thread = v["thread"].as_str().ok_or("invalid ask projection")?;
            if v["host"] != host || key != &ask_key(host, thread) {
                return Err("unverified ask projection owner".into());
            }
            let ask = vega_herdr::gather::Ask::from_json(&v["ask"].to_string())
                .ok_or("invalid ask projection")?;
            out.push((host.into(), thread.into(), ask, self.fresh(host)));
        }
        Ok(out)
    }
    pub fn wait_source(&self, host: &str) -> Option<&Value> {
        self.fresh(host)
            .then(|| self.entries.get(&format!("{}.orchestration", token(host))))?
    }

    /// Every well-formed Uber claim, also of stale hosts and gone panes. A
    /// record counts only under its own host's key.
    pub fn uber_claims(&self) -> Vec<UberClaim> {
        self.entries
            .iter()
            .filter_map(|(key, v)| {
                let host = v["host"].as_str()?;
                let claim = UberClaim {
                    pane: v["pane"].as_str().filter(|p| !p.is_empty())?.to_string(),
                    host: host.to_string(),
                    epoch: v["epoch"].as_u64()?,
                };
                (v["schema"] == 1 && *key == uber_key(host)).then_some(claim)
            })
            .collect()
    }

    /// Whether the claim's host beats and still lists its pane.
    pub fn uber_live(&self, claim: &UberClaim) -> bool {
        matches!(self.status(&token(&claim.host), &claim.pane), Some(Some(_)))
    }

    /// The live Uber-UAC: of the live claims, the highest epoch, ties to the
    /// lower host name. `None` when no claim is live (`uber_absent`).
    pub fn live_uber(&self) -> Option<UberClaim> {
        self.uber_claims()
            .into_iter()
            .filter(|c| self.uber_live(c))
            .reduce(|best, c| if c.beats(&best) { c } else { best })
    }

    /// The newest `human_at` of any host's beat: (host, at).
    pub fn human_at(&self) -> Option<(&str, i64)> {
        self.of_kind("beat")
            .filter_map(|(h, v)| Some((h, v["human_at"].as_i64()?)))
            .max_by_key(|(_, at)| *at)
    }

    /// Whether `claim` is engaged (`uber_engaged`): its `engaged_at` and the
    /// newest `human_at` of any host. While it is not, asks stay with their
    /// UACs and vegad sends no digest.
    pub fn uber_engaged(&self, claim: &UberClaim, now: i64) -> uber_engaged::Engaged {
        let input = self
            .entries
            .get(&uber_key(&claim.host))
            .and_then(|v| v["engaged_at"].as_i64());
        uber_engaged::decide(input, self.human_at(), now)
    }

    /// UAC registrations of fresh hosts for a full project ID.
    pub fn uacs(&self, project: &str) -> Vec<Value> {
        self.uac_entries()
            .into_iter()
            .filter(|(h, v)| self.fresh(h) && v["project"] == project && v["schema"] == 2)
            .map(|(_, v)| v.clone())
            .collect()
    }

    /// Every UAC registration, also of stale hosts: (host, entry).
    pub fn uac_entries(&self) -> Vec<(String, Value)> {
        self.entries
            .iter()
            .filter_map(|(key, value)| {
                let host = key.split('.').next()?;
                let project = value["project"].as_str()?;
                (value["schema"] == 2
                    && value["host"].as_str().is_some_and(|h| token(h) == host)
                    && *key == uac_key(host, project))
                .then(|| (host.to_string(), value.clone()))
            })
            .collect()
    }
}

/// `VEGA_NATS_URL`, else `~/.config/vega/nats.url`, else unconfigured.
fn url() -> String {
    vega_herdr::service_config::get("VEGA_NATS_URL", "nats.url").unwrap_or_default()
}

fn config_dir() -> Option<String> {
    env::var("XDG_CONFIG_HOME")
        .ok()
        .filter(|c| !c.is_empty())
        .or_else(|| env::var("HOME").ok().map(|h| format!("{h}/.config")))
}

fn seed() -> Option<String> {
    let path = env::var("VEGA_NATS_NKEY")
        .ok()
        .filter(|p| !p.is_empty())
        .or_else(|| config_dir().map(|c| format!("{c}/vega/nats.nk")))?;
    std::fs::read_to_string(path)
        .ok()
        .map(|s| s.trim().to_string())
}

/// Connects as this host (its nkey, its inbox prefix).
pub async fn connect(role: &str) -> Result<async_nats::Client, String> {
    connect_with(role, |options| options).await
}

/// Keep host authentication/configuration while observing protocol errors.
/// Permission denials arrive as events, not request replies.
pub async fn connect_with(
    role: &str,
    configure: impl FnOnce(async_nats::ConnectOptions) -> async_nats::ConnectOptions,
) -> Result<async_nats::Client, String> {
    let url = url();
    if url.is_empty() {
        return Err("no broker configured: set VEGA_NATS_URL or ~/.config/vega/nats.url".into());
    }
    let host = state::post_host();
    let opts = match seed() {
        Some(s) => async_nats::ConnectOptions::with_nkey(s),
        None => async_nats::ConnectOptions::new(),
    };
    let opts = opts
        .name(format!("vega-registry {role} {host}"))
        .custom_inbox_prefix(format!("_INBOX_{host}"))
        .connection_timeout(Duration::from_secs(2))
        .request_timeout(Some(Duration::from_secs(3)));
    configure(opts)
        .connect(url)
        .await
        .map_err(|e| format!("broker unreachable: {e}"))
}

pub async fn bucket(
    client: &async_nats::Client,
) -> Result<async_nats::jetstream::kv::Store, String> {
    async_nats::jetstream::new(client.clone())
        .get_key_value(BUCKET)
        .await
        .map_err(|e| format!("registry bucket {BUCKET}: {e}"))
}

/// Direct gets in flight at once while reading the bucket.
const GET_CONCURRENCY: usize = 64;

/// Every live key and value. Keys come from the stream's subject list and
/// values from direct gets: no consumer, so hosts need no consumer-create
/// right (a push consumer could make the server publish anywhere).
pub async fn read(
    client: &async_nats::Client,
    kv: &async_nats::jetstream::kv::Store,
) -> Result<View, String> {
    read_kinds(client, kv, None).await
}

/// `read`, fetching only the keys of these kinds (`beat`, `agent`, `uac`, ...):
/// `vega post send --uac` and the pane-gone hint need no lineage or notes.
/// `None`: every kind.
pub async fn read_kinds(
    client: &async_nats::Client,
    kv: &async_nats::jetstream::kv::Store,
    kinds: Option<&[&str]>,
) -> Result<View, String> {
    let stream = async_nats::jetstream::new(client.clone())
        .get_stream(format!("KV_{BUCKET}"))
        .await
        .map_err(|e| e.to_string())?;
    let prefix = format!("$KV.{BUCKET}.");
    let mut subjects = stream
        .info_with_subjects(format!("{prefix}>"))
        .await
        .map_err(|e| e.to_string())?;
    let mut view = View::default();
    while let Some(s) = subjects.next().await {
        let (subject, _) = s.map_err(|e| e.to_string())?;
        if let Some(k) = subject.strip_prefix(&prefix) {
            view.keys.push(k.to_string());
        }
    }
    // Concurrent direct gets: a deleted key keeps its subject (the delete
    // marker), so the subject list holds every key ever written and a
    // sequential get per key took minutes (vegad's loop stalled).
    // Notes only where this host's viewer has not read them: the badges
    // need their project and resolved state, and few are unread.
    let seen = vega_herdr::notes::Store::local().map(|s| s.seen());
    let own = token(&state::post_host());
    let wanted = |k: &str| {
        kinds.is_none_or(|ks| k.split('.').nth(1).is_some_and(|kind| ks.contains(&kind)))
            && match parse_note_key(k) {
                Some((h, id)) => h != own && seen.as_ref().is_some_and(|s| s.unread(h, id)),
                None => !is_note(k) && !is_janitor(k),
            }
    };
    let keys = view.keys.clone();
    let gets = keys.iter().filter(|k| wanted(k)).map(|k| async move {
        let v = kv
            .get(k)
            .await
            .ok()
            .flatten()
            .and_then(|b| serde_json::from_slice::<Value>(&b).ok());
        (k.clone(), v)
    });
    let mut got = futures_util::stream::iter(gets).buffer_unordered(GET_CONCURRENCY);
    while let Some((k, v)) = got.next().await {
        if let Some(v) = v {
            view.entries.insert(k, v);
        }
    }
    Ok(view)
}

/// The bodies of the notes `view` lists (direct gets of `<host>.note.<id>`).
pub async fn read_notes(
    kv: &async_nats::jetstream::kv::Store,
    view: &View,
) -> Vec<vega_herdr::notes::Note> {
    let mut out = view.unread_notes();
    for (host, id) in view.notes() {
        if view.entries.contains_key(&note_key(&host, id)) {
            continue;
        }
        if let Ok(Some(bytes)) = kv.get(note_key(&host, id)).await {
            if let Some(n) = serde_json::from_slice::<Value>(&bytes)
                .ok()
                .and_then(|v| vega_herdr::notes::Note::from_json(&v))
            {
                out.push(n);
            }
        }
    }
    out
}

/// Every host's notes, blocking, within `timeout` (vega post notes).
pub fn read_notes_blocking(timeout: Duration) -> Result<Vec<vega_herdr::notes::Note>, String> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;
    rt.block_on(async {
        tokio::time::timeout(timeout, async {
            let client = connect("notes").await?;
            let kv = bucket(&client).await?;
            let view = read(&client, &kv).await?;
            Ok(read_notes(&kv, &view).await)
        })
        .await
        .map_err(|_| "registry read timed out".to_string())?
    })
}

/// The whole bucket, blocking, within `timeout` (vega uac).
pub fn read_blocking(timeout: Duration) -> Result<View, String> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;
    rt.block_on(async {
        tokio::time::timeout(timeout, async {
            let client = connect("read").await?;
            let kv = bucket(&client).await?;
            read(&client, &kv).await
        })
        .await
        .map_err(|_| "registry read timed out".to_string())?
    })
}

/// The subject of a last-by-subject direct get of `key`: the only registry
/// subject a host with `auth-config --registry-read <host>=<key>` may publish
/// (vega-lhc.90). No stream info: its `subjects_filter` would list every key.
pub fn direct_get_subject(key: &str) -> String {
    format!("$JS.API.DIRECT.GET.KV_{BUCKET}.$KV.{BUCKET}.{key}")
}

/// One key's value by a single direct get, without opening the bucket (which
/// needs stream info): for hosts granted just that key. `None` when the key
/// is missing or deleted.
pub async fn get_one(client: &async_nats::Client, key: &str) -> Result<Option<Value>, String> {
    let subject = direct_get_subject(key);
    let msg = client
        .request(subject, "".into())
        .await
        .map_err(|e| format!("{key}: {e}"))?;
    if msg.status == Some(async_nats::StatusCode::NOT_FOUND) {
        return Ok(None);
    }
    if let Some(s) = msg.status.filter(|s| !s.is_success()) {
        return Err(format!("{key}: status {s}"));
    }
    let h = msg.headers.as_ref();
    let header = |n: &str| h.and_then(|h| h.get(n)).map(|v| v.as_str().to_string());
    if header("Nats-Subject").as_deref() != Some(&format!("$KV.{BUCKET}.{key}")) {
        return Err(format!("{key}: unexpected reply"));
    }
    if matches!(header("KV-Operation").as_deref(), Some("DEL" | "PURGE")) {
        return Ok(None);
    }
    serde_json::from_slice(&msg.payload)
        .map(Some)
        .map_err(|e| format!("{key}: {e}"))
}

/// `get_one`, blocking, within `timeout` (vega theme on a limited host).
pub fn get_one_blocking(key: &str, timeout: Duration) -> Result<Option<Value>, String> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;
    rt.block_on(async {
        tokio::time::timeout(timeout, async {
            let client = connect("get").await?;
            get_one(&client, key).await
        })
        .await
        .map_err(|_| "registry read timed out".to_string())?
    })
}

/// Shared publication shape for startup plans and steady reconciliation.
pub fn uac_entry(host: &str, record: &Value) -> Result<Value, String> {
    let project = record["project"].as_str().ok_or("claim missing project")?;
    let pane = record["pane"].as_str().ok_or("claim missing pane")?;
    let at = record["at"].as_i64().ok_or("claim missing timestamp")?;
    let mut entry = json!({"schema":2,"pane":pane,"host":host,"project":project,"at":at});
    for alias in ["origin_name", "folder_name", "aliases"] {
        if record[alias].is_string() || (alias == "aliases" && record[alias].is_array()) {
            entry[alias] = record[alias].clone();
        }
    }
    let mut aliases = std::collections::BTreeSet::new();
    for field in ["origin_name", "folder_name"] {
        if let Some(alias) = record[field].as_str() {
            aliases.insert(alias.to_owned());
        }
    }
    if let Some(list) = record["aliases"].as_array() {
        aliases.extend(list.iter().filter_map(|a| a.as_str().map(str::to_owned)));
    }
    entry["aliases"] = json!(aliases);
    Ok(entry)
}

/// Makes this host's keys equal `desired` plus a fresh beat: puts what
/// changed since `last` (the previous reconcile's values), deletes this
/// host's keys that are no longer desired, except CLI-managed project pins.
/// Returns the new `last`.
pub async fn reconcile(
    kv: &async_nats::jetstream::kv::Store,
    host: &str,
    desired: BTreeMap<String, Value>,
    last: &BTreeMap<String, Value>,
    present: &[String],
) -> Result<BTreeMap<String, Value>, String> {
    for (k, v) in &desired {
        if last.get(k) != Some(v) {
            kv.put(k, v.to_string().into())
                .await
                .map_err(|e| e.to_string())?;
        }
    }
    let prefix = format!("{}.", token(host));
    for k in present {
        if k.starts_with(&prefix)
            && *k != beat_key(host)
            && !pins::is_pin(k)
            && !is_legacy_project_key(k)
            && !desired.contains_key(k)
        {
            kv.delete(k).await.map_err(|e| e.to_string())?;
        }
    }
    heartbeat(kv, host).await?;
    Ok(desired)
}

/// This host's beat: `{schema: 2, at, human_at?}`.
pub fn beat() -> Value {
    let mut beat = json!({"schema": 2, "at": unix_now()});
    if let Some(at) = uber_engaged::human_file().and_then(|f| uber_engaged::read_at(&f)) {
        beat["human_at"] = json!(at);
    }
    beat
}

/// Refresh a running v2 owner's presence without changing project records.
/// Used while the cutover guard holds ordinary publication for pending peers.
pub async fn heartbeat(kv: &async_nats::jetstream::kv::Store, host: &str) -> Result<(), String> {
    kv.put(beat_key(host), beat().to_string().into())
        .await
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_requires_explicit_endpoint() {
        const FIXTURE: &str = "VEGA_ENDPOINT_CONFIG_FIXTURE";
        if std::env::var_os(FIXTURE).is_none() {
            let root = tempfile::tempdir().unwrap();
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "tests::registry_requires_explicit_endpoint",
                    "--nocapture",
                ])
                .env(FIXTURE, "1")
                .env("HOME", root.path())
                .env("XDG_CONFIG_HOME", root.path().join("config"))
                .env("XDG_STATE_HOME", root.path().join("state"))
                .env_remove("VEGA_NATS_URL")
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            return;
        }
        assert!(url().is_empty());
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let error = rt
            .block_on(connect_with("test", |_| {
                panic!("unconfigured registry must not dial")
            }))
            .unwrap_err();
        assert!(error.contains("no broker configured"));
        let config =
            std::path::PathBuf::from(std::env::var_os("XDG_CONFIG_HOME").unwrap()).join("vega");
        std::fs::create_dir_all(&config).unwrap();
        std::fs::write(
            config.join("nats.url"),
            "nats://broker.example:4222\nignored\n",
        )
        .unwrap();
        assert_eq!(url(), "nats://broker.example:4222");
        std::env::set_var("VEGA_NATS_URL", "nats://override.example:4222");
        assert_eq!(url(), "nats://override.example:4222");
    }

    #[test]
    fn wait_and_ask_projections_verify_their_publisher_and_keep_stale_records_unknown() {
        use vega_herdr::waits::{Actor, Change, Endpoint, Proof, Request, Store};
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("db");
        let owner = Endpoint {
            host: "owner".into(),
            pane: "w1:p1".into(),
            name: "parent".into(),
        };
        let actor = Actor {
            endpoint: owner.clone(),
            session: "s".into(),
        };
        let mut store = Store::open(&file).unwrap();
        let req = Request {
            version: 1,
            actor,
            change: Change::Waiting {
                coordinator: owner,
                on: "Slayer".into(),
                reason: "decision".into(),
                bead: None,
                thread: None,
            },
        };
        let receipt = store
            .apply(
                "owner",
                "owner",
                &req,
                Proof {
                    actor_known: true,
                    coordinator_known: true,
                    child_of_coordinator: false,
                },
                100,
            )
            .unwrap();
        let wait = store.get(&receipt.id).unwrap().unwrap();
        let mut view = View::default();
        view.entries
            .insert(beat_key("owner"), json!({"at":unix_now()}));
        view.entries.insert(
            wait_key("owner", &wait.id),
            serde_json::to_value(&wait).unwrap(),
        );
        assert!(view.explicit_waits().unwrap()[0].1);
        view.entries
            .insert(beat_key("owner"), json!({"at":unix_now()-1000}));
        assert!(!view.explicit_waits().unwrap()[0].1);
        view.entries.insert(
            "owner.wait.wrong".into(),
            serde_json::to_value(&wait).unwrap(),
        );
        assert!(view.explicit_waits().is_err());
        let ask = vega_herdr::gather::Ask {
            asker: "w1:p1".into(),
            targets: vec!["worker/w2:p1".into()],
            answered: vec![],
            deadline_ms: 1,
            question: "reply?".into(),
            created_ms: 1,
        };
        view.entries.insert(ask_key("owner","t"),json!({"host":"owner","thread":"t","ask":serde_json::from_str::<Value>(&ask.to_json()).unwrap()}));
        let rows = view.open_asks().unwrap();
        assert_eq!(rows.len(), 1);
        assert!(!rows[0].3);
        view.entries.get_mut(&ask_key("owner", "t")).unwrap()["host"] = json!("impostor");
        assert!(view.open_asks().is_err());
    }

    #[test]
    fn origin_repository_names() {
        for url in [
            "git@github.com:durandom/orrery.git",
            "https://github.com/durandom/orrery.git/",
            "ssh://git@github.com/durandom/orrery.git",
            "/srv/git/orrery.git",
            "b4arena/orrery",
        ] {
            assert_eq!(origin_name(url), Some("orrery"), "{url}");
        }
        for url in [
            "",
            ".git",
            "/",
            ".",
            "..",
            "https://secret:password@host",
            "https://host/repo?token=secret",
        ] {
            assert_eq!(origin_name(url), None, "{url}");
        }
    }

    #[test]
    fn origin_name_from_plain_clone_and_bare_worktree() {
        // Isolate HOME in a subprocess; other registry tests run concurrently.
        if env::var_os("VEGA_ORIGIN_FIXTURE_CHILD").is_none() {
            let home = tempfile::tempdir().unwrap();
            let out = std::process::Command::new(env::current_exe().unwrap())
                .args([
                    "--exact",
                    "tests::origin_name_from_plain_clone_and_bare_worktree",
                    "--nocapture",
                ])
                .env("HOME", home.path().canonicalize().unwrap())
                .env("VEGA_ORIGIN_FIXTURE_CHILD", "1")
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{}{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
            return;
        }
        let tmp = tempfile::tempdir_in(env::var_os("HOME").unwrap()).unwrap();
        let git = |args: &[&str]| {
            let out = std::process::Command::new("git")
                .current_dir(tmp.path())
                .args(args)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
        };
        git(&["init", "--bare", "-q", "spellkave/.bare"]);
        git(&[
            "--git-dir=spellkave/.bare",
            "config",
            "remote.origin.url",
            "git@github.com:b4arena/spellkave.git",
        ]);
        git(&[
            "--git-dir=spellkave/.bare",
            "worktree",
            "add",
            "--orphan",
            "-b",
            "main",
            "spellkave/main",
        ]);
        git(&["clone", "-q", "spellkave/.bare", "orrery/main"]);
        git(&[
            "-C",
            "orrery/main",
            "remote",
            "set-url",
            "origin",
            "https://github.com/durandom/orrery.git",
        ]);
        for (cwd, name) in [("spellkave/main", "spellkave"), ("orrery/main", "orrery")] {
            let p =
                vega_herdr::project::resolve(&tmp.path().join(cwd), &state::post_host()).unwrap();
            let key = vega_herdr::home_relative(p.location.common_dir.to_str().unwrap());
            assert!(key.starts_with("~/"), "{key}");
            assert_eq!(project_origin_name(&key).as_deref(), Some(name));
        }
        let plain = tmp.path().join("orrery/main/.git").display().to_string();
        git(&["-C", "orrery/main", "remote", "remove", "origin"]);
        assert_eq!(project_origin_name(&plain), None);
    }

    #[test]
    fn origin_config_failures_are_bounded() {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(mode) = env::var("VEGA_ORIGIN_TEST_MODE") {
            let started = std::time::Instant::now();
            let name = project_origin_name("~/project/.git");
            assert_eq!(name.as_deref(), (mode == "ok").then_some("orrery"));
            assert!(started.elapsed() < Duration::from_secs(5));
            return;
        }
        let home = tempfile::tempdir().unwrap();
        let bin = home.path().join("bin");
        std::fs::create_dir(&bin).unwrap();
        let git = bin.join("git");
        std::fs::write(
            &git,
            r#"#!/bin/sh
[ "$1" = --vega-test-warmup ] && exit 0
[ "$GIT_TERMINAL_PROMPT" = 0 ] && [ "$GIT_OPTIONAL_LOCKS" = 0 ] || exit 70
[ "$2" = "$HOME/project/.git" ] || exit 71
case "$VEGA_ORIGIN_TEST_MODE" in
  ok) echo git@github.com:durandom/orrery.git ;;
  fail) exit 1 ;;
  hang) exec /bin/sleep 60 ;;
esac
"#,
        )
        .unwrap();
        std::fs::set_permissions(&git, std::fs::Permissions::from_mode(0o755)).unwrap();
        // Keep macOS first-exec assessment outside the production Git budget.
        assert!(std::process::Command::new(&git)
            .arg("--vega-test-warmup")
            .status()
            .unwrap()
            .success());
        for mode in ["ok", "fail", "hang"] {
            let out = std::process::Command::new(env::current_exe().unwrap())
                .args([
                    "--exact",
                    "tests::origin_config_failures_are_bounded",
                    "--nocapture",
                ])
                .env("HOME", home.path())
                .env("PATH", &bin)
                .env("VEGA_ORIGIN_TEST_MODE", mode)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{mode}: {}{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
        }
    }

    #[test]
    fn uac_publication_reads_only_versioned_state_and_keeps_aliases() {
        if env::var_os("VEGA_UAC_V2_FIXTURE_CHILD").is_none() {
            let root = tempfile::tempdir().unwrap();
            let out = std::process::Command::new(env::current_exe().unwrap())
                .args([
                    "--exact",
                    "tests::uac_publication_reads_only_versioned_state_and_keeps_aliases",
                ])
                .env("XDG_STATE_HOME", root.path())
                .env("VEGA_UAC_V2_FIXTURE_CHILD", "1")
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{}{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
            return;
        }
        let project = "github.com/team/repo";
        let legacy = state::state_file_on("uac", None, "/old/.git").unwrap();
        std::fs::create_dir_all(legacy.parent().unwrap()).unwrap();
        std::fs::write(
            &legacy,
            json!({"project":"/old/.git","pane":"old","host":"h","at":1}).to_string(),
        )
        .unwrap();
        let file = state::project_file("uac", project).unwrap();
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, json!({"schema":2,"project":project,"pane":"new","host":"h","at":5,"folder_name":"local-folder","origin_name":"origin-repo","aliases":["second-folder"],"tracker":"bead-1"}).to_string()).unwrap();
        assert_eq!(
            state::project_uac_of(project),
            Some(("new".into(), "h".into()))
        );
        assert!(state::project_uac_of("/old/.git").is_none());
        let entries = local_entries("h", &[], &[]);
        let published = &entries[&uac_key("h", project)];
        assert_eq!(published["project"], project);
        assert_eq!(published["folder_name"], "local-folder");
        assert_eq!(
            published["aliases"],
            json!(["local-folder", "origin-repo", "second-folder"])
        );
        assert_eq!(published["at"], 5);
        let stored: Value = serde_json::from_str(&std::fs::read_to_string(file).unwrap()).unwrap();
        assert_eq!(stored["tracker"], "bead-1");
        assert_eq!(entries.keys().filter(|k| k.contains(".uac.")).count(), 1);
    }

    #[test]
    fn local_agents_publish_shared_id_and_keep_locations() {
        let tmp = tempfile::tempdir().unwrap();
        for folder in ["clone-a", "clone-b"] {
            let path = tmp.path().join(folder);
            let init = std::process::Command::new("git")
                .args(["init", "-q"])
                .arg(&path)
                .status()
                .unwrap();
            assert!(init.success());
            assert!(std::process::Command::new("git")
                .arg("-C")
                .arg(&path)
                .args(["remote", "add", "origin", "git@github.com:team/repo.git"])
                .status()
                .unwrap()
                .success());
            let entries = local_entries(
                "host",
                &[json!({"pane_id":"p1", "cwd":path, "workspace_id":"w"})],
                &[json!({"workspace_id":"w", "worktree":{"repo_key":path.join(".git")}})],
            );
            let agent = &entries[&agent_key("host", "p1")];
            assert_eq!(agent["schema"], 2);
            assert_eq!(agent["project"], "github.com/team/repo");
            assert!(agent["repo_key"].as_str().unwrap().contains(folder));
            let moved = local_entries(
                "host",
                &[json!({"pane_id":"p1","cwd":tmp.path(),"workspace_id":"w"})],
                &[
                    json!({"workspace_id":"w","worktree":{"repo_key":path.join(".git"),"checkout_path":path}}),
                ],
            );
            assert_eq!(
                moved[&agent_key("host", "p1")]["project"],
                "github.com/team/repo"
            );
        }
        assert_ne!(
            uac_key("h", "git.example/a/b"),
            uac_key("h", "git.example/a_b")
        );
    }

    #[test]
    fn keys_are_kv_safe_and_host_first() {
        assert_eq!(agent_key("proteus", "w3V:p1"), "proteus.agent.w3V_p1");
        assert_eq!(
            link_key("proteus", "ganymede", "w5S:p9"),
            "proteus.link.ganymede.w5S_p9"
        );
        assert_eq!(
            uac_key("proteus", "~/src/x/.git"),
            "proteus.uac.v2.7e2f7372632f782f2e676974"
        );
        assert_eq!(beat_key("proteus"), "proteus.beat");
        assert_eq!(theme_key("proteus"), "proteus.theme");
        assert_eq!(janitor_key("proteus"), "proteus.janitor");
        assert!(is_janitor("proteus.janitor"));
        assert!(!is_janitor("proteus.agent.janitor"));
        assert_eq!(
            direct_get_subject("proteus.theme"),
            "$JS.API.DIRECT.GET.KV_VEGA_REGISTRY.$KV.VEGA_REGISTRY.proteus.theme"
        );
        assert_eq!(note_key("proteus", 17), "proteus.note.17");
        assert_eq!(
            seen_key("proteus", "gany.mede"),
            "proteus.notes.seen.gany_mede"
        );
    }

    #[test]
    fn notes_are_listed_but_not_state() {
        assert_eq!(parse_note_key("proteus.note.17"), Some(("proteus", 17)));
        assert_eq!(parse_note_key("proteus.notes.seen.x"), None);
        assert_eq!(parse_note_key("proteus.agent.w1_p1"), None);
        assert!(is_note("proteus.note.17"));
        assert!(is_note("proteus.notes.seen.ganymede"));
        assert!(!is_note("proteus.agent.w1_p1"));
        assert!(!is_note("proteus.beat"));
        let v = View {
            keys: vec!["a.note.5".into(), "a.beat".into(), "b.note.9".into()],
            ..Default::default()
        };
        assert_eq!(v.notes(), vec![("a".into(), 5), ("b".into(), 9)]);
    }

    #[test]
    fn stale_hosts_prove_nothing() {
        let mut v = View::default();
        v.entries.insert(beat_key("a"), json!({"at": unix_now()}));
        v.entries
            .insert(beat_key("b"), json!({"at": unix_now() - 600}));
        v.entries
            .insert(agent_key("a", "w1:p1"), json!({"status": "idle"}));
        v.entries
            .insert(agent_key("b", "w1:p1"), json!({"status": "idle"}));
        assert_eq!(v.status("a", "w1:p1"), Some(Some("idle".into())));
        assert_eq!(v.status("a", "w9:p9"), Some(None));
        assert_eq!(v.status("b", "w1:p1"), None);
        assert_eq!(v.agents().len(), 1);
    }

    #[test]
    fn waiting_agents_of_fresh_hosts() {
        let mut v = View::default();
        v.entries.insert(beat_key("a"), json!({"at": unix_now()}));
        v.entries
            .insert(beat_key("b"), json!({"at": unix_now() - 600}));
        let waits = json!({"pane": "w1:p1", "name": "x", "status": "idle",
            "waiting": {"status": "done", "since": 5}});
        v.entries.insert(agent_key("a", "w1:p1"), waits.clone());
        v.entries.insert(agent_key("b", "w1:p1"), waits);
        v.entries.insert(
            agent_key("a", "w1:p2"),
            json!({"pane": "w1:p2", "status": "working"}),
        );
        let w = v.waiting();
        assert_eq!(w.len(), 1);
        assert_eq!(
            (w[0].host.as_str(), w[0].pane.as_str(), w[0].since),
            ("a", "w1:p1", 5)
        );
        assert_eq!((w[0].name.as_str(), w[0].status.as_str()), ("x", "done"));
    }

    #[test]
    fn theme_of_fresh_hosts_only() {
        let mut v = View::default();
        v.entries.insert(beat_key("a"), json!({"at": unix_now()}));
        v.entries
            .insert(beat_key("b"), json!({"at": unix_now() - 600}));
        v.entries.insert(theme_key("a"), json!({"mode": "dark"}));
        v.entries.insert(theme_key("b"), json!({"mode": "light"}));
        assert_eq!(v.theme("a"), Some("dark"));
        assert_eq!(v.theme("b"), None);
        assert_eq!(v.theme("c"), None);
    }

    #[test]
    fn uacs_by_project_on_fresh_hosts() {
        let mut v = View::default();
        v.entries.insert(beat_key("a"), json!({"at": unix_now()}));
        v.entries.insert(
            uac_key("a", "~/src/x/.git"),
            json!({"schema": 2, "host":"a", "pane": "w1:p1", "project": "~/src/x/.git"}),
        );
        v.entries.insert(
            uac_key("b", "~/src/x/.git"),
            json!({"schema": 2, "host":"b", "pane": "w2:p1", "project": "~/src/x/.git"}),
        );
        assert_eq!(v.uacs("~/src/x/.git").len(), 1);
        assert!(v.uacs("~/src/y/.git").is_empty());
    }

    fn uber(v: &mut View, host: &str, pane: &str, epoch: u64, live: bool) {
        v.entries.insert(beat_key(host), json!({"at": unix_now()}));
        v.entries.insert(
            uber_key(host),
            json!({"schema": 1, "host": host, "pane": pane, "at": 0, "epoch": epoch}),
        );
        if live {
            v.entries.insert(
                agent_key(host, pane),
                json!({"pane": pane, "status": "idle"}),
            );
        }
    }

    #[test]
    fn live_uber_is_the_newest_live_claim() {
        let mut v = View::default();
        assert_eq!(uber_key("ganymede"), "ganymede.uber.v1");
        assert_eq!(v.live_uber(), None);
        uber(&mut v, "ganymede", "w1:p1", 100, true);
        uber(&mut v, "proteus", "w2:p1", 200, true);
        assert_eq!(v.live_uber().unwrap().host, "proteus");
        // Tie: the lower host name.
        uber(&mut v, "io", "w3:p1", 200, true);
        assert_eq!(v.live_uber().unwrap().host, "io");
        // A gone pane or a stale host is not live; an older live claim wins.
        uber(&mut v, "titan", "w4:p1", 900, false);
        v.entries
            .insert(beat_key("io"), json!({"at": unix_now() - 600}));
        assert_eq!(v.live_uber().unwrap().host, "proteus");
        assert_eq!(v.uber_claims().len(), 4);
    }

    #[test]
    fn uber_engaged_reads_the_claims_engaged_at_and_every_hosts_human_at() {
        use uber_engaged::{Engaged, Idle};
        let now = unix_now();
        let mut v = View::default();
        uber(&mut v, "ganymede", "w1:p1", 100, true);
        let c = v.live_uber().unwrap();
        assert_eq!(v.uber_engaged(&c, now), Engaged::Off(Idle::Never));
        v.entries.get_mut(&uber_key("ganymede")).unwrap()["engaged_at"] = json!(now - 60);
        assert!(v.uber_engaged(&c, now).on());
        // Laptop focus before the capture keeps it; on any host after it ends it.
        v.entries.insert(
            beat_key("proteus"),
            json!({"at": now - 600, "human_at": now - 120}),
        );
        assert!(v.uber_engaged(&c, now).on());
        v.entries
            .insert(beat_key("io"), json!({"at": now, "human_at": now - 10}));
        assert_eq!(
            v.uber_engaged(&c, now),
            Engaged::Off(Idle::Elsewhere {
                host: "io".into(),
                at: now - 10
            })
        );
    }

    #[test]
    fn uber_claims_count_only_under_their_own_key() {
        let mut v = View::default();
        uber(&mut v, "ganymede", "w1:p1", 100, true);
        // A record naming another host, or another schema, is ignored.
        v.entries.insert(
            uber_key("deimos"),
            json!({"schema": 1, "host": "ganymede", "pane": "w1:p1", "epoch": 999}),
        );
        v.entries
            .insert(beat_key("deimos"), json!({"at": unix_now()}));
        uber(&mut v, "io", "w3:p1", 500, true);
        v.entries.get_mut(&uber_key("io")).unwrap()["schema"] = json!(2);
        assert_eq!(v.uber_claims().len(), 1);
        assert_eq!(v.live_uber().unwrap().epoch, 100);
    }
}
