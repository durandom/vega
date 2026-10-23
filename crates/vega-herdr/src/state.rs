//! Per-pane records that `vega mux spawn` keeps on the spawning machine:
//! the task (title fallback) and the parent (who spawned the agent). The
//! relay (vegad) adds supervision records for those children: `done` (the
//! child's `vega post done`) and `silent` (it went idle without one).
//! `briefs` holds when the parent last gave the child work, so a report
//! sent before it is marked stale. `parked` holds the decision bead a child
//! waits on (`vega uac park`); vegad marks it closed. `modes` holds whether
//! the owner works in the child (headed) or nobody does (headless).
//! `attention` turns them into what the parent must look at
//! (docs/design/vegad.md, Supervision).
//!
//! Files live under `$XDG_STATE_HOME/vega/<kind>/<machine>_<pane>` (default
//! `~/.local/state`). herdr reuses pane ids, so a record holds the agent name
//! it was written for and is ignored when the pane now hosts another agent;
//! `spawn` overwrites supervision records and `close` deletes them. Durable
//! `spawns` records survive close so the harness session can be reopened.

use std::env;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::{local_host, machine};

const KINDS: [&str; 9] = [
    "tasks",
    "parents",
    "modes",
    "done",
    "silent",
    "replied",
    "briefs",
    "undelivered",
    "parked",
];

/// `$XDG_STATE_HOME/vega/<kind>/<machine>_<pane>`; `<machine>` is the
/// `--machine` label, `local` for this machine.
pub fn state_file(kind: &str, pane: &str) -> Option<PathBuf> {
    state_file_on(kind, machine(), pane)
}

/// `state_file` for an explicit machine (`None`: this one).
pub fn state_file_on(kind: &str, machine: Option<&str>, pane: &str) -> Option<PathBuf> {
    let state = env::var("XDG_STATE_HOME")
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| env::var("HOME").ok().map(|h| format!("{h}/.local/state")))?;
    let machine = machine.unwrap_or("local");
    let file = format!("{machine}_{pane}").replace([':', '/'], "_");
    Some(Path::new(&state).join("vega").join(kind).join(file))
}

/// The pane that spawned an agent, and the host that pane runs on.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Parent {
    pub pane: String,
    pub host: String,
}

impl Parent {
    /// `w3V:p1`, or `w3V:p1 on deimos` when the parent runs on another host
    /// than this one, or the child on a `--machine`.
    pub fn label(&self, here: &str) -> String {
        self.label_on(here, machine())
    }

    /// `label` for a child on an explicit machine (`None`: this one).
    pub fn label_on(&self, here: &str, machine: Option<&str>) -> String {
        if self.host == here && machine.is_none() {
            self.pane.clone()
        } else {
            format!("{} on {}", self.pane, self.host)
        }
    }
}

/// Records `parent` for the agent `name` in `pane`; replaces a stale record.
/// Best effort: a missing record only hides the parent.
pub fn save_parent(pane: &str, name: &str, parent: &str) {
    let record = json!({"name": name, "parent": parent, "host": local_host()});
    write(&state_file("parents", pane), &record.to_string());
    save_brief(machine(), pane);
}

/// `save_parent` for a child on an explicit machine (`None`: this one).
pub fn save_parent_on(machine: Option<&str>, pane: &str, name: &str, parent: &str) {
    let record = json!({"name": name, "parent": parent, "host": local_host()});
    write(
        &state_file_on("parents", machine, pane),
        &record.to_string(),
    );
    save_brief(machine, pane);
}

/// The parent briefed the child in `pane` now (spawn, adopt, mail to it). A
/// done report sent before the last brief is stale.
pub fn save_brief(machine: Option<&str>, pane: &str) {
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    write(&state_file_on("briefs", machine, pane), &ms.to_string());
}

/// When the parent last briefed the child in `pane`, ms since the epoch.
pub fn brief_of(machine: Option<&str>, pane: &str) -> Option<u128> {
    std::fs::read_to_string(state_file_on("briefs", machine, pane)?)
        .ok()?
        .trim()
        .parse()
        .ok()
}

/// Versioned project state. Pane-keyed state keeps its existing encoding.
/// Startup migration writes these files before project workers resume.
pub fn project_file(kind: &str, project: &str) -> Option<PathBuf> {
    let root = state_file_on(kind, None, "x")?;
    Some(root.parent()?.join("v2").join(format!(
        "{}.json",
        crate::project::encode_component(project)
    )))
}

/// The UAC of a full project ID. Registry and routing consumers use this
/// ID view; filesystem consumers use the separate location view below.
pub fn project_uac_of(project: &str) -> Option<(String, String)> {
    let text = std::fs::read_to_string(project_file("uac", project)?).ok()?;
    let v: Value = serde_json::from_str(&text).ok()?;
    if v["schema"] != 2 || v["project"] != project {
        return None;
    }
    Some((v["pane"].as_str()?.into(), v["host"].as_str()?.into()))
}

fn project_uac_claims() -> Vec<Value> {
    let Some(dir) = project_file("uac", "x").and_then(|f| f.parent().map(Path::to_path_buf)) else {
        return Vec::new();
    };
    std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let v: Value =
                serde_json::from_str(&std::fs::read_to_string(entry.path()).ok()?).ok()?;
            let project = v["project"].as_str()?;
            if v["schema"] != 2 || entry.path() != project_file("uac", project)? {
                return None;
            }
            v["pane"].as_str()?;
            v["host"].as_str()?;
            Some(v)
        })
        .collect()
}

/// Project IDs, panes and claim times from v2 state, for publication/routing.
pub fn project_uac_records() -> Vec<(String, String, i64)> {
    project_uac_claims()
        .iter()
        .filter_map(|v| {
            Some((
                v["project"].as_str()?.into(),
                v["pane"].as_str()?.into(),
                v["at"].as_i64().unwrap_or(0),
            ))
        })
        .collect()
}

/// The primary execution location of a local v2 claim. Never use a remote
/// registry payload as input: these files belong to this host.
pub fn project_uac_location(project: &str) -> Option<crate::project::RepoLocation> {
    let v: Value =
        serde_json::from_str(&std::fs::read_to_string(project_file("uac", project)?).ok()?).ok()?;
    if v["schema"] != 2 || v["project"] != project {
        return None;
    }
    let common_dir = PathBuf::from(v["common_dir"].as_str()?);
    let checkout = PathBuf::from(v["checkout"].as_str()?);
    (common_dir.is_absolute() && checkout.is_absolute()).then_some(crate::project::RepoLocation {
        common_dir,
        checkout,
    })
}

/// Every owner-local location retained by a migrated/updated v2 claim.
pub fn project_uac_locations() -> Vec<(String, crate::project::RepoLocation)> {
    let mut out = Vec::new();
    for v in project_uac_claims() {
        let Some(id) = v["project"].as_str() else {
            continue;
        };
        if let (Some(common), Some(checkout)) = (v["common_dir"].as_str(), v["checkout"].as_str()) {
            let location = crate::project::RepoLocation {
                common_dir: common.into(),
                checkout: checkout.into(),
            };
            if location.common_dir.is_absolute() && location.checkout.is_absolute() {
                out.push((id.to_owned(), location));
            }
        }
        if let Some(locations) = v["locations"].as_array() {
            for location in locations {
                if let Ok(location) =
                    serde_json::from_value::<crate::project::RepoLocation>(location.clone())
                {
                    if location.common_dir.is_absolute() && location.checkout.is_absolute() {
                        out.push((id.to_owned(), location));
                    }
                }
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

/// Owner-local common directories, panes and claim times projected from v2
/// claims. This is location metadata, never a project comparison key.
pub fn uac_records() -> Vec<(String, String, i64)> {
    let claims = project_uac_claims();
    let mut out = Vec::new();
    for (project, location) in project_uac_locations() {
        if let Some(v) = claims.iter().find(|v| v["project"] == project) {
            if let Some(pane) = v["pane"].as_str() {
                out.push((
                    crate::home_relative(&location.common_dir.to_string_lossy()),
                    pane.to_owned(),
                    v["at"].as_i64().unwrap_or(0),
                ));
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

/// UAC of a known owner-local common directory, including retained clones.
pub fn uac_of(key: &str) -> Option<(String, String)> {
    project_uac_locations().iter().find_map(|(id, location)| {
        let common = location.common_dir.to_str()?;
        (common == key || crate::home_relative(common) == key).then_some(())?;
        project_uac_of(id)
    })
}

/// This machine's host token in vega post subjects: `VEGA_POST_HOST`, else
/// the short host name, lowercase, without `.`, `*`, `>` or whitespace.
pub fn post_host() -> String {
    let h = env::var("VEGA_POST_HOST")
        .ok()
        .filter(|h| !h.is_empty())
        .unwrap_or_else(local_host);
    host_token(&h)
}

pub fn host_token(h: &str) -> String {
    h.trim()
        .to_lowercase()
        .chars()
        .map(|c| {
            if c == '.' || c == '*' || c == '>' || c.is_whitespace() {
                '_'
            } else {
                c
            }
        })
        .collect()
}

/// Hosts whose broker key may not mail agents (`vega post auth-config
/// --inbox-only`): their replies go into quarantine, a notice for the human
/// only (vega-lhc.83). `VEGA_POST_INBOX_ONLY` (comma list; set but empty: none),
/// else `$XDG_CONFIG_HOME/vega/inbox-only` (one host per line, `#` comments),
/// else deimos (Slayer decision 2026-10-07). The broker enforces it; this list
/// only picks the way and the wording.
pub fn inbox_only_hosts() -> Vec<String> {
    if let Ok(v) = env::var("VEGA_POST_INBOX_ONLY") {
        return parse_hosts(&v.replace(',', "\n"));
    }
    let file = env::var("XDG_CONFIG_HOME")
        .ok()
        .filter(|c| !c.is_empty())
        .or_else(|| env::var("HOME").ok().map(|h| format!("{h}/.config")))
        .map(|c| Path::new(&c).join("vega").join("inbox-only"));
    match file.and_then(|f| std::fs::read_to_string(f).ok()) {
        Some(text) => parse_hosts(&text),
        None => vec!["deimos".into()],
    }
}

fn parse_hosts(text: &str) -> Vec<String> {
    text.lines()
        .map(|l| l.split('#').next().unwrap_or(""))
        .filter(|l| !l.trim().is_empty())
        .map(host_token)
        .collect()
}

/// Whether `host` (a vega post host token or machine label) is inbox-only.
pub fn is_inbox_only(host: &str) -> bool {
    inbox_only_hosts().contains(&host_token(host))
}

/// Whether vegad (the post relay) runs on this host: it holds its lock.
pub fn relay_running() -> bool {
    let Some(state) = state_file_on("x", None, "x:x")
        .and_then(|f| f.parent().and_then(|p| p.parent()).map(Path::to_path_buf))
    else {
        return false;
    };
    let lock = state.join(format!("post-relay-{}.lock", post_host()));
    match std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(lock)
    {
        Ok(f) => matches!(f.try_lock(), Err(std::fs::TryLockError::WouldBlock)),
        Err(_) => false,
    }
}

/// Whether this host's vegad runs in local mode (no broker): it records
/// `local (<why>)` in `post-relay-<host>.broker`. Then `vega post` mail
/// cannot reach anyone, and `vega mux send` pastes at once.
pub fn relay_local() -> bool {
    let Some(state) = state_file_on("x", None, "x:x")
        .and_then(|f| f.parent().and_then(|p| p.parent()).map(Path::to_path_buf))
    else {
        return false;
    };
    relay_running()
        && std::fs::read_to_string(state.join(format!("post-relay-{}.broker", post_host())))
            .is_ok_and(|s| s.starts_with("local"))
}

/// The parent recorded for `pane`, if the record was written for the agent
/// `name` that runs there now.
pub fn parent_of(pane: &str, name: &str) -> Option<Parent> {
    parent_of_on(machine(), pane, name)
}

/// `parent_of` for a child on an explicit machine (`None`: this one). Only
/// records this machine wrote are visible: a spawn from another host keeps
/// its record there.
pub fn parent_of_on(machine: Option<&str>, pane: &str, name: &str) -> Option<Parent> {
    let text = std::fs::read_to_string(state_file_on("parents", machine, pane)?).ok()?;
    parse_parent(&text, name)
}

fn parse_parent(text: &str, name: &str) -> Option<Parent> {
    let (n, p) = parse_record(text)?;
    (n == name).then_some(p)
}

fn parse_record(text: &str) -> Option<(String, Parent)> {
    let v: Value = serde_json::from_str(text).ok()?;
    Some((
        v["name"].as_str()?.to_string(),
        Parent {
            pane: v["parent"].as_str()?.to_string(),
            host: v["host"].as_str()?.to_string(),
        },
    ))
}

/// Who answers a child's in-pane questions. Headless (a sentinel nobody
/// watches): the ask-parent hook sends them to its parent. Headed (the owner
/// works in it, e.g. an adopted session): its dialogs stay.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Mode {
    Headless,
    Headed,
}

impl Mode {
    pub fn parse(s: &str) -> Option<Mode> {
        match s {
            "headless" => Some(Mode::Headless),
            "headed" => Some(Mode::Headed),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Headless => "headless",
            Mode::Headed => "headed",
        }
    }
}

/// Records the mode of the agent `name` in `pane` (`None`: this machine).
pub fn save_mode(machine: Option<&str>, pane: &str, name: &str, mode: Mode) {
    let record = json!({"name": name, "mode": mode.as_str()});
    write(&state_file_on("modes", machine, pane), &record.to_string());
}

/// The mode of the agent `name` in `pane`. No record, an unknown mode or one
/// written for another agent: headless, preserving older children. New spawns
/// explicitly record headed by default (or headless with `spawn --headless`).
pub fn mode_of(machine: Option<&str>, pane: &str, name: &str) -> Mode {
    state_file_on("modes", machine, pane)
        .and_then(|f| std::fs::read_to_string(f).ok())
        .map_or(Mode::Headless, |t| parse_mode(&t, name))
}

/// Positive mode evidence for new ownership-sensitive attention. Unlike mode_of,
/// a missing, corrupt or reused-name record does not assume legacy headless mode.
pub fn known_mode_of(machine: Option<&str>, pane: &str, name: &str) -> Option<Mode> {
    let file = state_file_on("modes", machine, pane)?;
    let v: Value = serde_json::from_str(&std::fs::read_to_string(file).ok()?).ok()?;
    if v["name"].as_str() != Some(name) {
        return None;
    }
    Mode::parse(v["mode"].as_str()?)
}

fn parse_mode(text: &str, name: &str) -> Mode {
    let v: Value = serde_json::from_str(text).unwrap_or_default();
    match (v["name"].as_str(), v["mode"].as_str().and_then(Mode::parse)) {
        (Some(n), Some(m)) if n == name => m,
        _ => Mode::Headless,
    }
}

/// Whether an attention is addressed to the owner, not the parent: a headed
/// pane's question (`asking`), idle turn end (`silent`) or dialog (`input`).
/// The owner works in that pane and sees them; the parent is not woken, and
/// it must not answer an approval or trust dialog anyway. `exited`,
/// `failure`, `quota`, `completed` and `decided` still go to the parent.
pub fn for_user(a: Attention, mode: Mode) -> bool {
    mode == Mode::Headed
        && matches!(
            a,
            Attention::Asking | Attention::Unanswered | Attention::Silent | Attention::Input
        )
}

/// Whether the ask-parent hook sends a dialog to the parent: only a headless
/// child with a parent; without one the owner drives the session.
pub fn redirects_questions(parent: Option<&Parent>, mode: Mode) -> bool {
    parent.is_some() && mode == Mode::Headless
}

/// A child spawned from this machine: the `parents` record of one pane.
#[derive(Debug, PartialEq)]
pub struct Child {
    /// `None`: this machine; else the `--machine` label it was spawned on.
    pub machine: Option<String>,
    pub pane: String,
    pub name: String,
    pub parent: Parent,
}

/// Every parent record on this machine. The pane id comes back from the file
/// name (`<machine>_<w>_<p>`).
pub fn children() -> Vec<Child> {
    let mut out: Vec<Child> = records("parents")
        .into_iter()
        .filter_map(|(machine, pane, text)| {
            let (name, parent) = parse_record(&text)?;
            Some(Child {
                machine,
                pane,
                name,
                parent,
            })
        })
        .collect();
    out.sort_by(|a, b| (&a.machine, &a.pane).cmp(&(&b.machine, &b.pane)));
    out
}

/// Every record of `kind` on this machine: (machine, pane, text).
fn records(kind: &str) -> Vec<(Option<String>, String, String)> {
    let Some(dir) =
        state_file_on(kind, None, "x:x").and_then(|f| f.parent().map(Path::to_path_buf))
    else {
        return Vec::new();
    };
    std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let file = e.file_name().into_string().ok()?;
            let (machine, pane) = split_key(&file)?;
            Some((machine, pane, std::fs::read_to_string(e.path()).ok()?))
        })
        .collect()
}

/// `local_w3V_p2` -> (None, `w3V:p2`); `gany_mede_w1_p1` -> (Some(gany_mede), `w1:p1`).
fn split_key(file: &str) -> Option<(Option<String>, String)> {
    let mut parts = file.rsplitn(3, '_');
    let (p, w, m) = (parts.next()?, parts.next()?, parts.next()?);
    let machine = (m != "local").then(|| m.to_string());
    Some((machine, format!("{w}:{p}")))
}

/// How a child finished, from its `vega post done`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Outcome {
    Succeeded,
    Failed,
}

impl Outcome {
    pub fn parse(s: &str) -> Option<Outcome> {
        match s {
            "succeeded" => Some(Outcome::Succeeded),
            "failed" => Some(Outcome::Failed),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Outcome::Succeeded => "succeeded",
            Outcome::Failed => "failed",
        }
    }
}

/// The child's report: outcome and one-line summary.
#[derive(Debug, Clone, PartialEq)]
pub struct Done {
    pub outcome: Outcome,
    pub summary: String,
}

/// Records the done report of a child (relay, on the parent's machine). A
/// later report replaces it.
pub fn save_done(machine: Option<&str>, pane: &str, done: &Done) {
    let record = json!({"outcome": done.outcome.as_str(), "summary": done.summary});
    write(&state_file_on("done", machine, pane), &record.to_string());
}

pub fn done_of(machine: Option<&str>, pane: &str) -> Option<Done> {
    let text = std::fs::read_to_string(state_file_on("done", machine, pane)?).ok()?;
    let v: Value = serde_json::from_str(&text).ok()?;
    Some(Done {
        outcome: Outcome::parse(v["outcome"].as_str()?)?,
        summary: v["summary"].as_str().unwrap_or("").to_string(),
    })
}

/// The parent gave the child new work: its last report, reply, silence and
/// park no longer stand. Only the parent settles; a child working on its own
/// (a background shell finishing) does not.
pub fn settle(machine: Option<&str>, pane: &str) {
    for kind in ["done", "replied", "silent", "parked"] {
        remove(&state_file_on(kind, machine, pane));
    }
}

/// The child sent its parent a message since its last new work: it is not
/// silent, whatever channel it used.
pub fn set_replied(machine: Option<&str>, pane: &str) {
    write(&state_file_on("replied", machine, pane), "");
}

pub fn has_replied(machine: Option<&str>, pane: &str) -> bool {
    state_file_on("replied", machine, pane).is_some_and(|f| f.exists())
}

/// The relay saw the child go idle after its prompt without a done report.
pub fn set_silent(machine: Option<&str>, pane: &str, silent: bool) {
    let file = state_file_on("silent", machine, pane);
    if silent {
        write(&file, "");
    } else {
        remove(&file);
    }
}

pub fn is_silent(machine: Option<&str>, pane: &str) -> bool {
    state_file_on("silent", machine, pane).is_some_and(|f| f.exists())
}

/// What a silent child's finished turn ended with, read from its screen.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Silence {
    /// The question the turn ended with: the child waits for an answer.
    pub question: Option<String>,
    /// The last 1-2 output lines of the turn.
    pub last: String,
    /// The turn ended on a provider abort (rate limit, overload, quota).
    pub quota: Option<Quota>,
}

/// A provider abort that ended a child's turn.
#[derive(Debug, Clone, PartialEq)]
pub struct Quota {
    /// Overloaded or rate limited: retry soon. Else the quota is used up
    /// until it resets.
    pub transient: bool,
    /// The provider's error, one line.
    pub error: String,
    /// When it resets, as shown (`10-10 14:05`, `3pm (Europe/Berlin)`);
    /// `(token-burn)` marks one the error did not name.
    pub resets: Option<String>,
}

impl Quota {
    /// `overloaded: <error>` or `quota exhausted, resets <t>: <error>`.
    pub fn summary(&self) -> String {
        let what = match (&self.resets, self.transient) {
            (_, true) => "overloaded".to_string(),
            (Some(r), false) => format!("quota exhausted, resets {r}"),
            (None, false) => "quota exhausted".to_string(),
        };
        format!("{what}: {}", self.error)
    }

    /// The literal next step: retry soon, or after the reset, or move the
    /// work to another kind or provider.
    pub fn next(&self, pane: &str, mflag: &str) -> String {
        let retry = format!("vega mux{mflag} send {pane} \"continue\"");
        match (&self.resets, self.transient) {
            (_, true) => format!("retry soon: {retry}"),
            (Some(r), false) => {
                format!("retry after {r}: {retry}, or move the work to another kind/provider")
            }
            (None, false) => {
                format!("move the work to another kind/provider, or retry later: {retry}")
            }
        }
    }

    fn to_json(&self) -> Value {
        json!({"transient": self.transient, "error": self.error, "resets": self.resets})
    }

    /// From `to_json`; `None` for anything else.
    pub fn from_json(v: &Value) -> Option<Quota> {
        Some(Quota {
            transient: v["transient"].as_bool()?,
            error: v["error"].as_str()?.to_string(),
            resets: v["resets"].as_str().map(String::from),
        })
    }
}

/// `set_silent(.., true)` with what the turn ended with.
pub fn save_silence(machine: Option<&str>, pane: &str, s: &Silence) {
    let record = json!({
        "question": s.question,
        "last": s.last,
        "quota": s.quota.as_ref().map(Quota::to_json),
    });
    write(&state_file_on("silent", machine, pane), &record.to_string());
}

/// The silent record; an empty one (no screen was read) gives the default.
pub fn silence_of(machine: Option<&str>, pane: &str) -> Option<Silence> {
    let text = std::fs::read_to_string(state_file_on("silent", machine, pane)?).ok()?;
    Some(parse_silence(&text))
}

fn parse_silence(text: &str) -> Silence {
    let v: Value = serde_json::from_str(text).unwrap_or_default();
    Silence {
        question: v["question"].as_str().map(String::from),
        last: v["last"].as_str().unwrap_or("").to_string(),
        quota: Quota::from_json(&v["quota"]),
    }
}

/// The quota record for the registry link (`None` without one).
pub fn quota_json(machine: Option<&str>, pane: &str) -> Option<Value> {
    silence_of(machine, pane)?
        .quota
        .as_ref()
        .map(Quota::to_json)
}

/// A child parked on a decision bead (`vega uac park`): it waits, so vegad
/// does not wake its parent for it until the bead closes.
#[derive(Debug, Clone, PartialEq)]
pub struct Park {
    pub bead: String,
    pub title: String,
    /// Where `bd show <bead>` finds the bead: the checkout `park` ran in.
    pub dir: String,
    /// The `bd` binary `park` found: vegad's PATH may lack it.
    pub bd: String,
    /// vegad saw the bead closed.
    pub closed: bool,
}

pub fn save_park(machine: Option<&str>, pane: &str, p: &Park) {
    let record = json!({
        "bead": p.bead, "title": p.title, "dir": p.dir, "bd": p.bd, "closed": p.closed
    });
    write(&state_file_on("parked", machine, pane), &record.to_string());
}

pub fn park_of(machine: Option<&str>, pane: &str) -> Option<Park> {
    parse_park(&std::fs::read_to_string(state_file_on("parked", machine, pane)?).ok()?)
}

/// Removes the park; false if there was none.
pub fn unpark(machine: Option<&str>, pane: &str) -> bool {
    state_file_on("parked", machine, pane).is_some_and(|f| std::fs::remove_file(f).is_ok())
}

/// Every parked child on this machine: (machine, pane, park).
pub fn parks() -> Vec<(Option<String>, String, Park)> {
    records("parked")
        .into_iter()
        .filter_map(|(m, pane, text)| Some((m, pane, parse_park(&text)?)))
        .collect()
}

fn parse_park(text: &str) -> Option<Park> {
    let v: Value = serde_json::from_str(text).ok()?;
    Some(Park {
        bead: v["bead"].as_str()?.to_string(),
        title: v["title"].as_str().unwrap_or("").to_string(),
        dir: v["dir"].as_str().unwrap_or("").to_string(),
        bd: v["bd"].as_str().unwrap_or("bd").to_string(),
        closed: v["closed"] == true,
    })
}

/// What a parent must look at for one child. Variant order is precedence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Attention {
    /// The pane closed or the agent left it.
    Exited,
    /// herdr status `blocked`: an approval or other UI.
    Input,
    /// Parked on a decision bead that closed: resume it.
    Decided,
    /// Parked on an open decision bead: it waits, its parent is not woken.
    Parked,
    /// Its turn ended with a question (text or question dialog) and it waits.
    Asking,
    /// Idle while an unresolved post ask explicitly names it as a target.
    Unanswered,
    /// Its turn ended on a provider abort: rate limit, overload or quota.
    Quota,
    /// `done --outcome failed`.
    Failure,
    /// Idle without `done`, but `vega ci watch` will mail it the verdict:
    /// it waits, its parent is not woken.
    Waiting,
    /// Idle since its last prompt without `done`.
    Silent,
    /// `done --outcome succeeded`, waiting for the parent to settle it.
    Completed,
}

impl Attention {
    pub fn as_str(self) -> &'static str {
        match self {
            Attention::Exited => "exited",
            Attention::Input => "input",
            Attention::Decided => "decided",
            Attention::Parked => "parked",
            Attention::Asking => "asking",
            Attention::Unanswered => "unanswered",
            Attention::Quota => "quota",
            Attention::Failure => "failure",
            Attention::Waiting => "waiting",
            Attention::Silent => "silent",
            Attention::Completed => "completed",
        }
    }

    /// The literal next command for the parent.
    pub fn next(self, pane: &str, mflag: &str) -> String {
        match self {
            Attention::Exited => format!("vega mux{mflag} close {pane}"),
            Attention::Input | Attention::Silent => format!("vega mux{mflag} show {pane}"),
            Attention::Decided => {
                format!("bd show the bead, then vega mux{mflag} send {pane} \"<next step>\"")
            }
            Attention::Parked => {
                format!("wait for the decision, or vega uac unpark {pane}{mflag}")
            }
            Attention::Asking => {
                format!("answer it (vega mux{mflag} send {pane}) or ask the Slayer")
            }
            Attention::Unanswered => "vega post log".into(),
            Attention::Quota => format!(
                "retry later (vega mux{mflag} send {pane} \"continue\") or move the work to another kind/provider"
            ),
            Attention::Failure => {
                format!("vega mux{mflag} show {pane}, then send a fix or close it")
            }
            Attention::Waiting => format!("wait: the ci verdict mail wakes {pane}"),
            Attention::Completed => {
                format!("vega mux{mflag} show {pane}, then land or close it")
            }
        }
    }
}

/// Every attention, in precedence order.
pub const ATTENTIONS: [Attention; 11] = [
    Attention::Exited,
    Attention::Input,
    Attention::Decided,
    Attention::Parked,
    Attention::Asking,
    Attention::Unanswered,
    Attention::Quota,
    Attention::Failure,
    Attention::Waiting,
    Attention::Silent,
    Attention::Completed,
];

/// Attention from proof only: `alive` and `status` are `None` when they
/// cannot be checked (absence proves nothing). `silent`: its silent record;
/// with a question it is asking, ended on a provider abort it is quota. A
/// park hides the rest but exited and input: the child waits on its decision.
pub fn attention(
    alive: Option<bool>,
    status: Option<&str>,
    done: Option<Outcome>,
    silent: Option<&Silence>,
    park: Option<&Park>,
) -> Option<Attention> {
    if alive == Some(false) {
        return Some(Attention::Exited);
    }
    if status == Some("blocked") {
        return Some(Attention::Input);
    }
    if let Some(p) = park {
        return Some(if p.closed {
            Attention::Decided
        } else {
            Attention::Parked
        });
    }
    if silent.is_some_and(|s| s.question.is_some()) {
        return Some(Attention::Asking);
    }
    if silent.is_some_and(|s| s.quota.is_some()) {
        return Some(Attention::Quota);
    }
    match done {
        Some(Outcome::Failed) => Some(Attention::Failure),
        _ if silent.is_some() => Some(Attention::Silent),
        Some(Outcome::Succeeded) => Some(Attention::Completed),
        None => None,
    }
}

/// A silent child that watches CI (`vega ci watch`) is waiting: the verdict
/// mail wakes it. Once the watch ends, silent again.
pub fn waiting(a: Option<Attention>, watching: bool) -> Option<Attention> {
    match a {
        Some(Attention::Silent) if watching => Some(Attention::Waiting),
        a => a,
    }
}

/// The task `spawn --prompt` saved for `pane`.
pub fn saved_task(pane: &str) -> Option<String> {
    std::fs::read_to_string(state_file("tasks", pane)?).ok()
}

/// Saves the task for `pane`, or removes a stale one when there is none.
pub fn save_task(pane: &str, task: Option<&str>) {
    match task {
        Some(t) => write(&state_file("tasks", pane), t),
        None => remove(&state_file("tasks", pane)),
    }
}

/// A brief `spawn` could not deliver, with the agent it was meant for: its
/// name, herdr kind and cwd (empty in records written before they were kept),
/// and whether the spawn recorded no parent (`spawn --no-parent`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct KeptBrief {
    pub name: String,
    pub kind: String,
    pub cwd: String,
    pub brief: String,
    pub no_parent: bool,
}

/// Keeps the brief of a spawn that stopped before delivering it, for
/// `spawn --resume`; false if it could not be written.
pub fn keep_brief(pane: &str, kept: &KeptBrief) -> bool {
    let file = state_file("undelivered", pane);
    write(&file, &brief_record(kept));
    file.is_some_and(|f| f.exists())
}

fn brief_record(kept: &KeptBrief) -> String {
    json!({"name": kept.name, "kind": kept.kind, "cwd": kept.cwd, "brief": kept.brief, "no_parent": kept.no_parent})
        .to_string()
}

/// The brief `keep_brief` kept for `pane`.
pub fn kept_brief(pane: &str) -> Option<KeptBrief> {
    parse_brief(&std::fs::read_to_string(state_file("undelivered", pane)?).ok()?)
}

fn parse_brief(text: &str) -> Option<KeptBrief> {
    let v: Value = serde_json::from_str(text).ok()?;
    let field = |k: &str| v[k].as_str().unwrap_or_default().to_string();
    Some(KeptBrief {
        name: v["name"].as_str()?.into(),
        kind: field("kind"),
        cwd: field("cwd"),
        brief: v["brief"].as_str()?.into(),
        no_parent: v["no_parent"].as_bool().unwrap_or(false),
    })
}

/// Removes the saved brief of `pane` once delivered.
pub fn drop_kept_brief(pane: &str) {
    remove(&state_file("undelivered", pane));
}

/// Durable spawn identity, retained when supervision records are forgotten.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SpawnRecord {
    pub pane: String,
    pub name: String,
    pub kind: String,
    pub cwd: String,
    pub parent: Option<Parent>,
    pub mode: String,
    pub task: Option<String>,
    pub session: Option<String>,
    pub extra: Vec<String>,
}

pub fn save_spawn(record: &SpawnRecord) -> std::io::Result<()> {
    let file = state_file("spawns", &record.pane)
        .ok_or_else(|| std::io::Error::other("no state directory"))?;
    std::fs::create_dir_all(file.parent().expect("state file has parent"))?;
    let temporary = file.with_extension(format!("{}.tmp", std::process::id()));
    std::fs::write(&temporary, serde_json::to_vec(record)?)?;
    std::fs::rename(temporary, file)
}

pub fn spawn_records() -> Vec<SpawnRecord> {
    records("spawns")
        .into_iter()
        .filter(|(m, _, _)| m.as_deref() == machine())
        .filter_map(|(_, _, text)| serde_json::from_str(&text).ok())
        .collect()
}

pub fn restore_parent(pane: &str, name: &str, parent: Option<&Parent>) {
    match parent {
        Some(p) => write(
            &state_file("parents", pane),
            &json!({"name": name, "parent": p.pane, "host": p.host}).to_string(),
        ),
        None => remove(&state_file("parents", pane)),
    }
}

/// Deletes every record of `pane` (on `close`).
pub fn forget(pane: &str) {
    for kind in KINDS {
        remove(&state_file(kind, pane));
    }
}

fn write(file: &Option<PathBuf>, text: &str) {
    if let Some(f) = file {
        let _ = f.parent().map(std::fs::create_dir_all);
        let _ = std::fs::write(f, text);
    }
}

fn remove(file: &Option<PathBuf>) {
    if let Some(f) = file {
        let _ = std::fs::remove_file(f);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_mode_requires_a_valid_current_agent_record() {
        if std::env::var_os("VEGA_MODE_TEST_CHILD").is_none() {
            let dir = tempfile::tempdir().unwrap();
            let result = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "state::tests::known_mode_requires_a_valid_current_agent_record",
                ])
                .env("VEGA_MODE_TEST_CHILD", "1")
                .env("XDG_STATE_HOME", dir.path())
                .output()
                .unwrap();
            assert!(
                result.status.success(),
                "{}{}",
                String::from_utf8_lossy(&result.stdout),
                String::from_utf8_lossy(&result.stderr)
            );
            return;
        }
        assert_eq!(known_mode_of(None, "pane", "current"), None);
        write(&state_file_on("modes", None, "pane"), "not json");
        assert_eq!(known_mode_of(None, "pane", "current"), None);
        save_mode(None, "pane", "previous", Mode::Headed);
        assert_eq!(known_mode_of(None, "pane", "current"), None);
        for mode in [Mode::Headed, Mode::Headless] {
            save_mode(None, "pane", "current", mode);
            assert_eq!(known_mode_of(None, "pane", "current"), Some(mode));
        }
        write(
            &state_file_on("modes", None, "pane"),
            r#"{"name":"current","mode":"unknown"}"#,
        );
        assert_eq!(known_mode_of(None, "pane", "current"), None);
    }

    #[test]
    fn inbox_only_list_is_tokens() {
        assert_eq!(
            parse_hosts("Deimos # corp\n\n  eris.local\n#x"),
            vec!["deimos".to_string(), "eris_local".to_string()]
        );
        assert!(parse_hosts("").is_empty());
    }

    #[test]
    fn kept_briefs_keep_the_no_parent_choice() {
        let kept = KeptBrief {
            name: "gate-x".into(),
            kind: "codex".into(),
            cwd: "/c".into(),
            brief: "review".into(),
            no_parent: true,
        };
        assert_eq!(parse_brief(&brief_record(&kept)), Some(kept));
        // Records from before the field: a parent, as then.
        let old = parse_brief(r#"{"name":"fix","brief":"b"}"#).unwrap();
        assert!(!old.no_parent);
    }

    #[test]
    fn state_file_is_per_kind_machine_and_pane() {
        std::env::set_var("XDG_STATE_HOME", "/tmp/vega-state");
        assert_eq!(
            state_file("parents", "w3V:p2").unwrap(),
            Path::new("/tmp/vega-state/vega/parents/local_w3V_p2")
        );
    }

    #[test]
    fn parent_record_is_ignored_for_another_agent_in_a_reused_pane() {
        let text = r#"{"name":"fix","parent":"w3V:p1","host":"orrery"}"#;
        let p = parse_parent(text, "fix").unwrap();
        assert_eq!(p.pane, "w3V:p1");
        assert_eq!(p.label("orrery"), "w3V:p1");
        assert_eq!(p.label("deimos"), "w3V:p1 on orrery");
        assert_eq!(parse_parent(text, "other"), None);
        assert_eq!(parse_parent(text, "-"), None);
        assert_eq!(parse_parent("w3V:p1", "fix"), None);
    }

    #[test]
    fn only_a_headless_child_with_a_parent_is_redirected() {
        let p = Parent {
            pane: "w3V:p1".into(),
            host: "orrery".into(),
        };
        let headed = r#"{"name":"fix","mode":"headed"}"#;
        assert_eq!(parse_mode(headed, "fix"), Mode::Headed);
        // Another agent in a reused pane, an unknown mode, garbage, no record:
        // headless, so the old behaviour (redirect when a parent exists) holds.
        assert_eq!(parse_mode(headed, "other"), Mode::Headless);
        assert_eq!(
            parse_mode(r#"{"name":"fix","mode":"x"}"#, "fix"),
            Mode::Headless
        );
        assert_eq!(parse_mode("headed", "fix"), Mode::Headless);
        assert!(redirects_questions(Some(&p), Mode::Headless));
        assert!(!redirects_questions(Some(&p), Mode::Headed));
        assert!(!redirects_questions(None, Mode::Headless));
        assert!(!redirects_questions(None, Mode::Headed));
    }

    #[test]
    fn headed_questions_silence_and_dialogs_are_for_the_user() {
        use Attention::*;
        for a in [Asking, Silent, Input] {
            assert!(for_user(a, Mode::Headed), "{a:?}");
            assert!(!for_user(a, Mode::Headless), "{a:?}");
        }
        for a in [Exited, Failure, Quota, Completed, Decided, Parked, Waiting] {
            assert!(!for_user(a, Mode::Headed), "{a:?}");
        }
    }

    #[test]
    fn silence_records_round_trip_with_quota() {
        let s = Silence {
            question: None,
            last: "You've hit your limit".into(),
            quota: Some(Quota {
                transient: false,
                error: "You've hit your limit · resets 3pm (Europe/Berlin)".into(),
                resets: Some("3pm (Europe/Berlin)".into()),
            }),
        };
        let q = s.quota.as_ref().unwrap();
        let v = json!({"question": s.question, "last": s.last, "quota": q.to_json()});
        assert_eq!(parse_silence(&v.to_string()), s);
        // Older records and an empty one (no screen read): no quota.
        assert_eq!(parse_silence(r#"{"last":"x"}"#).quota, None);
        assert_eq!(parse_silence(""), Silence::default());
        assert_eq!(
            q.summary(),
            "quota exhausted, resets 3pm (Europe/Berlin): You've hit your limit · resets 3pm (Europe/Berlin)"
        );
        assert_eq!(
            q.next("w1:p2", ""),
            "retry after 3pm (Europe/Berlin): vega mux send w1:p2 \"continue\", or move the work to another kind/provider"
        );
        let t = Quota {
            transient: true,
            error: "API Error: 529 Overloaded.".into(),
            resets: None,
        };
        assert_eq!(t.summary(), "overloaded: API Error: 529 Overloaded.");
        assert_eq!(
            t.next("w1:p2", " --machine gany"),
            "retry soon: vega mux --machine gany send w1:p2 \"continue\""
        );
    }

    #[test]
    fn park_records_round_trip() {
        let p = Park {
            bead: "sk-w8zx.7".into(),
            title: "Golden or patterns?".into(),
            dir: "/Users/x/src/spellkave".into(),
            bd: "/opt/homebrew/bin/bd".into(),
            closed: true,
        };
        let v = json!({"bead": p.bead, "title": p.title, "dir": p.dir, "bd": p.bd, "closed": true});
        assert_eq!(parse_park(&v.to_string()), Some(p));
        // An old or partial record: bd from PATH, still open.
        let q = parse_park(r#"{"bead":"sk-1"}"#).unwrap();
        assert_eq!((q.bd.as_str(), q.closed), ("bd", false));
        assert_eq!(parse_park("{}"), None);
    }

    #[test]
    fn keys_split_back_into_machine_and_pane() {
        assert_eq!(split_key("local_w3V_p2"), Some((None, "w3V:p2".into())));
        assert_eq!(
            split_key("gany_mede_w1_p1"),
            Some((Some("gany_mede".into()), "w1:p1".into()))
        );
        assert_eq!(split_key("w1_p1"), None);
    }

    #[test]
    fn attention_acts_on_proof_only() {
        use Attention::*;
        let ok = Some(Outcome::Succeeded);
        let failed = Some(Outcome::Failed);
        let silence = Silence::default();
        let quiet = Some(&silence);
        let asks = Silence {
            question: Some("Should I also cover Hamburg?".into()),
            ..Silence::default()
        };
        let aborted = Silence {
            quota: Some(super::Quota {
                transient: true,
                error: "API Error: 529 Overloaded.".into(),
                resets: None,
            }),
            ..Silence::default()
        };
        assert_eq!(
            attention(Some(true), Some("idle"), None, Some(&aborted), None),
            Some(Attention::Quota)
        );
        assert_eq!(attention(None, None, None, None, None), None);
        assert_eq!(attention(Some(true), Some("idle"), None, None, None), None);
        assert_eq!(
            attention(Some(false), Some("idle"), ok, None, None),
            Some(Exited)
        );
        assert_eq!(
            attention(Some(true), Some("blocked"), ok, None, None),
            Some(Input)
        );
        assert_eq!(
            attention(Some(true), Some("idle"), failed, quiet, None),
            Some(Failure)
        );
        assert_eq!(
            attention(Some(true), Some("idle"), None, quiet, None),
            Some(Silent)
        );
        assert_eq!(attention(None, None, ok, None, None), Some(Completed));
        assert_eq!(
            attention(Some(true), Some("idle"), None, Some(&asks), None),
            Some(Asking)
        );
        assert_eq!(
            attention(Some(true), Some("blocked"), None, Some(&asks), None),
            Some(Input)
        );
        // Parked: only exited and input still count; a closed bead decides.
        let mut park = Park {
            bead: "sk-1".into(),
            title: "Which colour?".into(),
            dir: "/repo".into(),
            bd: "bd".into(),
            closed: false,
        };
        assert_eq!(
            attention(Some(true), Some("idle"), ok, quiet, Some(&park)),
            Some(Parked)
        );
        assert_eq!(
            attention(Some(true), Some("idle"), None, Some(&asks), Some(&park)),
            Some(Parked)
        );
        assert_eq!(
            attention(Some(false), None, ok, None, Some(&park)),
            Some(Exited)
        );
        assert_eq!(
            attention(Some(true), Some("blocked"), ok, None, Some(&park)),
            Some(Input)
        );
        park.closed = true;
        assert_eq!(
            attention(Some(true), Some("idle"), ok, None, Some(&park)),
            Some(Decided)
        );
    }

    #[test]
    fn a_ci_watch_turns_only_silence_into_waiting() {
        use Attention::*;
        assert_eq!(waiting(Some(Silent), true), Some(Waiting));
        assert_eq!(waiting(Some(Silent), false), Some(Silent));
        for a in [Exited, Input, Parked, Asking, Failure, Completed] {
            assert_eq!(waiting(Some(a), true), Some(a));
        }
        assert_eq!(waiting(None, true), None);
    }
}
