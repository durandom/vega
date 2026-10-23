use serde_json::Value;
use std::{collections::BTreeSet, path::Path, process::Command, time::Duration};
use vega_herdr::{cell, state, Failure};
use vega_registry::View;
#[path = "runtime.rs"]
mod runtime;

#[derive(Debug)]
pub struct Row {
    pub id: String,
    pub bead: String,
    pub waiter: String,
    pub owner: String,
    pub reason: String,
    pub age: String,
    pub source: String,
    pub freshness: String,
    pub next: String,
}

pub fn json_files(dir: &Path) -> Result<Vec<(String, Value, i64)>, String> {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) => return Err(format!("unknown ({}: {e})", dir.display())),
    };
    let mut out = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|_| "unknown (directory unreadable)")?;
        if !entry
            .file_type()
            .map_err(|_| "unknown (metadata unreadable)")?
            .is_file()
        {
            continue;
        }
        let value = serde_json::from_str(
            &std::fs::read_to_string(entry.path()).map_err(|_| "unknown (record unreadable)")?,
        )
        .map_err(|_| "unknown (invalid record)")?;
        let at = entry
            .metadata()
            .ok()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_secs() as i64);
        out.push((entry.file_name().to_string_lossy().into_owned(), value, at));
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

pub fn text(v: &Value, key: &str) -> String {
    v[key].as_str().unwrap_or("unknown").into()
}
fn age(at: i64, now: i64) -> String {
    if at == 0 {
        "unknown".into()
    } else {
        format!("{}s", (now - at).max(0))
    }
}

fn host_key(host: &str) -> String {
    vega_registry::token(&host.to_lowercase())
}

/// Membership comes from the owner's resolved ID, never remote filesystem paths.
fn belongs(a: &Value, project: &str) -> bool {
    a["schema"] == 2 && a["project"].as_str() == Some(project)
}

/// Only positive project membership from a fresh registry entry is usable.
fn members(view: &View, project: &str) -> BTreeSet<(String, String)> {
    view.agents()
        .into_iter()
        .filter(|(_, a)| belongs(a, project))
        .map(|(h, a)| (host_key(&h), text(&a, "pane")))
        .collect()
}

pub fn asks(
    rows: &mut Vec<Row>,
    files: &[(String, Value, i64)],
    panes: &BTreeSet<(String, String)>,
    host: &str,
    now: i64,
) {
    for (file, a, at) in files {
        let asker = text(a, "asker");
        if !panes.contains(&(host.into(), asker.clone())) {
            continue;
        }
        for target in a["targets"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
        {
            if a["answered"]
                .as_array()
                .is_some_and(|xs| xs.iter().any(|x| x == target))
            {
                continue;
            }
            rows.push(Row {
                id: file.trim_start_matches("local_").into(),
                bead: "unknown".into(),
                waiter: format!("{asker}@{host}"),
                owner: target.into(),
                reason: match a["question"].as_str().filter(|q| !q.is_empty()) {
                    Some(q) => format!(
                        "unanswered ask {}: {}",
                        file.trim_start_matches("local_"),
                        preview(q)
                    ),
                    None => format!("unanswered ask {}", file.trim_start_matches("local_")),
                },
                age: age(a["created_ms"].as_i64().unwrap_or(0) / 1000, now),
                source: "post-ask (local)".into(),
                freshness: format!("persisted thread; source update {}", age(*at, now)),
                next: "vega post log".into(),
            });
        }
    }
}

fn preview(s: &str) -> String {
    let mut text: String = s.chars().take(600).collect();
    if s.chars().count() > 600 {
        text.push('…');
    }
    text
}

use vega_herdr::hosts::registry_for_machine as machine_registry_host;

fn link_registry_host(link: &Value, machines: &Value, local: &str) -> Option<String> {
    let parent = host_key(link["parent_host"].as_str()?);
    let child = link["child_host"].as_str()?;
    if host_key(child) == parent {
        return Some(parent);
    }
    // Another publisher's machine aliases cannot be interpreted with this host's map.
    if parent != local {
        return None;
    }
    machine_registry_host(child, machines, local)
}

fn question_rows(
    view: &View,
    panes: &BTreeSet<(String, String)>,
    machines: &Value,
    local: &str,
    mode: impl Fn(&Value) -> Option<state::Mode>,
) -> (Vec<Row>, usize) {
    let mut rows = Vec::new();
    let mut unknown = 0;
    for link in view.links() {
        let Some(q) = link["question"].as_str() else {
            continue;
        };
        if !view.fresh(&host_key(&text(&link, "parent_host"))) {
            unknown += 1;
            continue;
        }
        let Some(h) = link_registry_host(&link, machines, local) else {
            unknown += 1;
            continue;
        };
        let p = text(&link, "pane");
        if view.agent(&h, &p).is_none() {
            unknown += 1;
            continue;
        }
        if !panes.contains(&(h.clone(), p.clone())) {
            continue;
        }
        rows.push(Row {
            id: format!("question:{p}@{h}"),
            bead: "unknown".into(),
            waiter: format!("{p}@{h}"),
            owner: question_owner(&link, mode(&link)),
            reason: q.into(),
            age: "unknown".into(),
            source: "registry-question".into(),
            freshness: "fresh host; question age unknown".into(),
            next: inspect_next(&h, &p, local),
        });
    }
    (rows, unknown)
}

fn attention_age(waiting: &Value, now: i64) -> String {
    age(waiting["since"].as_i64().unwrap_or(0) / 1000, now)
}

fn inspect_next(host: &str, pane: &str, local_host: &str) -> String {
    if host == local_host {
        format!("vega mux show {pane}")
    } else {
        "vega post log".into()
    } // Registry host is not a herdr machine label.
}

fn mode_record(record: Option<&str>, name: &str) -> Option<state::Mode> {
    let v: Value = serde_json::from_str(record?).ok()?;
    if v["name"] != name {
        return None;
    }
    state::Mode::parse(v["mode"].as_str()?)
}

fn question_owner(link: &Value, mode: Option<state::Mode>) -> String {
    match mode {
        Some(state::Mode::Headed) => "Slayer".into(),
        Some(state::Mode::Headless) => {
            match (link["parent"].as_str(), link["parent_host"].as_str()) {
                (Some(p), Some(h)) if !p.is_empty() && !h.is_empty() => format!("{p}@{h}"),
                _ => "unknown".into(),
            }
        }
        None => "unknown".into(),
    }
}

/// Only read records owned by this host. A registry host never names a local path.
fn local_link_mode(link: &Value, local_host: &str) -> Option<state::Mode> {
    if host_key(&text(link, "parent_host")) != host_key(local_host) {
        return None;
    }
    let pane = link["pane"].as_str()?;
    let name = link["name"].as_str()?;
    let child_host = host_key(link["child_host"].as_str()?);
    let machine = if child_host == local_host {
        None
    } else {
        let child = state::children().into_iter().find(|c| {
            c.pane == pane
                && c.name == name
                && c.parent.pane == text(link, "parent")
                && c.machine
                    .as_ref()
                    .is_some_and(|m| host_key(m) == child_host)
        })?;
        child.machine
    };
    let f = state::state_file_on("modes", machine.as_deref(), pane)?;
    let record = std::fs::read_to_string(f).ok()?;
    mode_record(Some(&record), name)
}

pub fn run(parent: Option<&str>) -> Result<(), Failure> {
    let cwd = std::env::current_dir().map_err(|_| {
        Failure::new(
            "project",
            "working directory unavailable",
            "run inside the project checkout",
        )
    })?;
    let project = vega_herdr::lanes::project_key(&cwd)?;
    let host = host_key(&state::post_host());
    let now = vega_ci_watch::unix_now();
    println!(
        "bin: {}",
        cell(&vega_herdr::collapse_home(
            &std::env::current_exe()
                .unwrap_or_default()
                .display()
                .to_string()
        ))
    );
    println!("description: Project waits and evidence for supervised coordination");
    println!("project: {}", cell(&project));
    let registry = vega_registry::read_blocking(Duration::from_secs(3));
    let mut rows = Vec::new();
    let panes = registry
        .as_ref()
        .map(|v| members(v, &project))
        .unwrap_or_default();
    if let Ok(view) = &registry {
        for (h, p) in &panes {
            let Some(a) = view.agent(h, p) else {
                continue;
            };
            if let Some(w) = a.get("waiting") {
                rows.push(Row {
                    id: format!("attention:{p}@{h}"),
                    bead: "unknown".into(),
                    waiter: format!("{p}@{h}"),
                    owner: "Slayer".into(),
                    reason: text(w, "status"),
                    age: attention_age(w, now),
                    source: "registry-attention".into(),
                    freshness: format!("host beat {}", age(view.beat_at(h).unwrap_or(0), now)),
                    next: inspect_next(h, p, &host),
                });
            }
        }
        let machines =
            vega_herdr::herdr_on(None, &["machine", "list", "--json"], Duration::from_secs(2))
                .unwrap_or(Value::Null);
        let (questions, unknown) = question_rows(view, &panes, &machines, &host, |link| {
            local_link_mode(link, &host)
        });
        rows.extend(questions);
        if unknown > 0 {
            println!("registry_questions: unknown ({} sources have stale publishers or unresolved host/agent mappings; project membership unknown)",unknown);
        } else {
            println!(
                "registry_questions: snapshot; missing or remote mode records leave owner unknown"
            );
        }
        let mut unknown_parks = 0;
        for (m, p, park) in state::parks() {
            let h = match m.as_deref() {
                None => host.clone(),
                Some(label) => match machine_registry_host(label, &machines, &host) {
                    Some(h) => h,
                    None => {
                        unknown_parks += 1;
                        continue;
                    }
                },
            };
            if view.agent(&h, &p).is_none() {
                unknown_parks += 1;
                continue;
            }
            if !panes.contains(&(h.clone(), p.clone())) {
                continue;
            }
            rows.push(park_row(m.as_deref(), &p, &h, &park));
        }
        println!("uac_parks: local snapshot; remote parks unknown; {unknown_parks} unresolved host/agent mappings with unknown project membership");
        // Existing watch receipts only; never create a watcher from a view.
        let ci_dir = vega_ci_watch::store::dir().map(|d| d.join("watch"));
        if ci_dir.as_ref().is_some_and(|d| json_files(d).is_ok()) {
            for w in vega_ci_watch::store::watches() {
                if !panes.contains(&(host.clone(), w.pane.clone())) {
                    continue;
                }
                let s = vega_ci_watch::store::load_state(&w.target);
                rows.push(ci_row(&w, s.as_ref(), &host, now));
            }
            println!("ci: local snapshot; remote receipts unknown");
        } else {
            println!("ci: unknown (local receipts unavailable)");
        }
        println!(
            "registry: snapshot; only hosts with a beat younger than 90s; stale hosts unknown"
        );
    } else {
        println!(
            "registry: unknown (unavailable); project membership and remote ownership unknown"
        );
        println!("registry_questions: unknown (registry unavailable)");
        println!("uac_parks: unknown (project membership unavailable)");
        println!("ci: unknown (project membership unavailable)");
    }
    runtime::rows(&mut rows, registry.as_ref().ok(), &panes, &cwd, &host, now);
    runtime::questions(&mut rows, registry.as_ref().ok(), &panes, &host, now);
    let scope_known = if let Some(parent) = parent {
        match parent_scope(&cwd, parent) {
            Ok(ids) => {
                rows.retain(|r| ids.contains(&r.bead));
                println!("scope: parent and all descendants, including closed beads; unassociated evidence omitted");
                true
            }
            Err(_) => {
                println!("scope: unknown (parent missing or bd unavailable); showing unfiltered project evidence");
                false
            }
        }
    } else {
        true
    };
    print_rows(&rows);
    if scope_known {
        ready(&cwd, parent);
    } else {
        println!("ready: unknown (parent scope unresolved)");
    }
    println!("help: vega skills orchestration");
    Ok(())
}

fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\"'\"'"))
}

fn park_row(machine: Option<&str>, pane: &str, host: &str, park: &state::Park) -> Row {
    let next = if park.closed {
        format!(
            "vega uac unpark {}{}",
            quote(pane),
            machine
                .map(|m| format!(" --machine {}", quote(m)))
                .unwrap_or_default()
        )
    } else {
        format!("bd show {}", quote(&park.bead))
    };
    Row {
        id: format!("park:{pane}@{host}"),
        bead: park.bead.clone(),
        waiter: format!("{pane}@{host}"),
        owner: if park.closed {
            "unknown (inspect the closed decision)".into()
        } else {
            "Slayer".into()
        },
        reason: format!(
            "decision {}: {}",
            if park.closed { "closed" } else { "open" },
            park.title
        ),
        age: "unknown".into(),
        source: "uac-park (local)".into(),
        freshness: "local decision snapshot".into(),
        next,
    }
}

fn ci_row(
    w: &vega_ci_watch::store::Watch,
    s: Option<&vega_ci_watch::store::State>,
    host: &str,
    now: i64,
) -> Row {
    Row {
        id: format!("ci:{}", w.target.label()),
        bead: "unknown".into(),
        waiter: format!("{}@{host}", w.pane),
        owner: w.target.label(),
        reason: s.map_or("unknown".into(), |s| {
            format!("last observed {}", s.summary().verdict.as_str())
        }),
        age: age(w.since, now),
        source: "ci-watch (local)".into(),
        freshness: s.map_or("unknown (no poll)".into(), |s| {
            format!(
                "poll {}; head {}; {}",
                age(s.polled, now),
                s.head.as_deref().unwrap_or("unknown"),
                if s.error.is_some() {
                    "latest poll failed"
                } else {
                    "cached observation"
                }
            )
        }),
        next: "vega ci".into(),
    }
}

pub fn print_rows(rows: &[Row]) {
    println!("wait_count: {}", rows.len());
    if rows.is_empty() {
        println!("waits: 0 observed waits in available scoped sources; unknown sources may contain waits");
        return;
    }
    println!(
        "waits[{}]{{id,bead,waiter,awaited_owner,reason,age,source,freshness,next}}:",
        rows.len()
    );
    for r in rows {
        println!(
            "  {}",
            [
                &r.id,
                &r.bead,
                &r.waiter,
                &r.owner,
                &r.reason,
                &r.age,
                &r.source,
                &r.freshness,
                &r.next
            ]
            .map(|s| cell(s))
            .join(",")
        );
    }
}

/// Read every status once, then walk actual parent edges, never ID prefixes.
fn all_beads(cwd: &Path) -> Result<Vec<Value>, ()> {
    let mut cmd = Command::new("bd");
    cmd.current_dir(cwd).args([
        "list",
        "--all",
        "--include-ephemeral",
        "--include-gates",
        "--brief",
        "--json",
        "--limit",
        "0",
    ]);
    let out = vega_herdr::output_within(&mut cmd, Duration::from_secs(5))
        .map_err(|_| ())?
        .ok_or(())?;
    if !out.status.success() {
        return Err(());
    }
    let issues: Value = serde_json::from_str(&vega_plugin::unwrap_bd_envelope(
        &String::from_utf8_lossy(&out.stdout),
    ))
    .map_err(|_| ())?;
    issues.as_array().cloned().ok_or(())
}

fn parent_scope(cwd: &Path, parent: &str) -> Result<BTreeSet<String>, ()> {
    scope_ids(parent, &all_beads(cwd)?)
}

fn scope_ids(parent: &str, issues: &[Value]) -> Result<BTreeSet<String>, ()> {
    if !issues.iter().any(|i| i["id"] == parent) {
        return Err(());
    }
    let mut ids = BTreeSet::from([parent.to_string()]);
    loop {
        let before = ids.len();
        for i in issues {
            let Some(id) = i["id"].as_str() else {
                return Err(());
            };
            let in_scope = i["parent"].as_str().is_some_and(|p| ids.contains(p))
                || i["dependencies"].as_array().into_iter().flatten().any(|d| {
                    d["type"] == "parent-child"
                        && d["depends_on_id"].as_str().is_some_and(|p| ids.contains(p))
                });
            if in_scope {
                ids.insert(id.into());
            }
        }
        if ids.len() == before {
            return Ok(ids);
        }
    }
}

fn ready(cwd: &Path, parent: Option<&str>) {
    let mut cmd = Command::new("bd");
    cmd.current_dir(cwd)
        .args(["ready", "--json", "--limit", "0"]);
    if let Some(p) = parent {
        cmd.args(["--parent", p]);
    }
    let result = vega_herdr::output_within(&mut cmd, Duration::from_secs(5))
        .ok()
        .flatten()
        .filter(|o| o.status.success())
        .and_then(|o| {
            serde_json::from_str::<Value>(&vega_plugin::unwrap_bd_envelope(
                &String::from_utf8_lossy(&o.stdout),
            ))
            .ok()
        })
        .and_then(|v| v.as_array().cloned());
    match result {
        Some(items) => {
            println!("ready_count: {}", items.len());
            if items.is_empty() {
                println!("ready: 0 ready beads in this scope");
            } else {
                println!("ready[{}]{{id,title,status}}:", items.len());
                for b in items {
                    println!(
                        "  {},{},{}",
                        cell(&text(&b, "id")),
                        cell(&text(&b, "title")),
                        cell(&text(&b, "status"))
                    );
                }
            }
        }
        None => println!("ready: unknown (bd unavailable or unreadable)"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn ask_join_uses_membership_and_answers_not_read_ack() {
        let files = vec![(
            "local_t".into(),
            json!({"asker":"p","targets":["h/a","h/b"],"answered":["h/a"],"read":true}),
            100,
        )];
        let mut rows = Vec::new();
        asks(
            &mut rows,
            &files,
            &BTreeSet::from([("h".into(), "p".into())]),
            "h",
            150,
        );
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].owner, "h/b");
        assert_eq!(rows[0].age, "unknown");
        assert!(rows[0].freshness.contains("50s"));
        let mut other = Vec::new();
        asks(&mut other, &files, &BTreeSet::new(), "h", 150);
        assert!(other.is_empty());
    }

    #[test]
    fn park_rows_preserve_decision_state_and_quote_saved_machine_labels() {
        let mut p = state::Park {
            bead: "b-1".into(),
            title: "choose owner".into(),
            dir: "/project".into(),
            bd: "bd".into(),
            closed: false,
        };
        let open = park_row(None, "w1:p1", "here", &p);
        assert_eq!(open.owner, "Slayer");
        assert_eq!(open.next, "bd show 'b-1'");
        p.closed = true;
        let closed = park_row(Some("Build box"), "w1:p1", "elsewhere", &p);
        assert!(closed.owner.contains("unknown"));
        assert!(closed.reason.contains("closed"));
        assert_eq!(closed.next, "vega uac unpark 'w1:p1' --machine 'Build box'");
        assert_eq!(closed.source, "uac-park (local)");
    }

    #[test]
    fn ci_rows_bind_observations_to_head_and_poll_freshness() {
        let target = vega_ci_watch::Target::pr("github.com", "o/r", 7);
        let w = vega_ci_watch::store::Watch {
            target: target.clone(),
            pane: "w1:p1".into(),
            since: 100,
        };
        let absent = ci_row(&w, None, "here", 200);
        assert_eq!(absent.reason, "unknown");
        assert_eq!(absent.freshness, "unknown (no poll)");
        let mut state = vega_ci_watch::store::State::new(&target, 100);
        state.head = Some("abc123".into());
        state.polled = 180;
        let observed = ci_row(&w, Some(&state), "here", 200);
        assert_eq!(observed.owner, "o/r#7");
        assert_eq!(observed.age, "100s");
        assert!(observed.freshness.contains("20s"));
        assert!(observed.freshness.contains("abc123"));
        state.error = Some("offline".into());
        assert!(ci_row(&w, Some(&state), "here", 200)
            .freshness
            .contains("failed"));
        assert_eq!(observed.next, "vega ci");
    }
    #[test]
    fn published_membership_uses_schema_and_id_independent_of_locations() {
        let key = "github.com/durandom/vega";
        for location in ["~/src/vega/.git", "/srv/other/.bare"] {
            assert!(belongs(
                &json!({"schema":2,"project":key,"repo_key":location}),
                key
            ));
        }
        for a in [
            json!({"repo_key":"~/src/vega/.git","cwd":"~/src/vega"}),
            json!({"schema":1,"project":key}),
            json!({"schema":2,"project":"github.com/fork/vega"}),
        ] {
            assert!(!belongs(&a, key), "{a}");
        }
    }

    #[test]
    fn questions_and_parks_resolve_aliases_or_expose_unknown_sources() {
        let now = vega_ci_watch::unix_now();
        let mut view = View::default();
        view.entries
            .insert(vega_registry::beat_key("here"), json!({"at":now}));
        view.entries
            .insert(vega_registry::beat_key("io-wsl"), json!({"at":now}));
        view.entries.insert(
            vega_registry::agent_key("io-wsl", "fixture:p1"),
            json!({"pane":"fixture:p1","schema":2,"project":"github.com/durandom/vega"}),
        );
        view.entries.insert(vega_registry::link_key("here","io","fixture:p1"),json!({"parent_host":"here","parent":"fixture:p0","child_host":"io","pane":"fixture:p1","question":"reply owed","name":"test"}));
        let panes = members(&view, "github.com/durandom/vega");
        let machines = json!([{"label":"io","target":"ssh://me@io-wsl:2222"}]);
        let (rows, unknown) = question_rows(&view, &panes, &machines, "here", |_| None);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].waiter, "fixture:p1@io-wsl");
        assert_eq!(unknown, 0);
        let (rows, unknown) = question_rows(&view, &panes, &Value::Null, "here", |_| None);
        assert!(rows.is_empty());
        assert_eq!(unknown, 1);
        assert_eq!(
            machine_registry_host("io", &machines, "here").as_deref(),
            Some("io-wsl")
        );
        assert_eq!(machine_registry_host("io", &Value::Null, "here"), None);
    }

    #[test]
    fn attention_timestamps_are_milliseconds_and_routes_do_not_guess_machine_labels() {
        assert_eq!(
            attention_age(&json!({"since": 1_790_000_000_000_i64}), 1_790_000_060),
            "60s"
        );
        assert_eq!(inspect_next("here", "w1:p1", "here"), "vega mux show w1:p1");
        assert_eq!(inspect_next("elsewhere", "w1:p1", "here"), "vega post log");
    }

    #[test]
    fn question_owner_requires_a_matching_readable_mode_record() {
        let link = json!({"parent":"w1:p1","parent_host":"h"});
        let headed = r#"{"name":"child","mode":"headed"}"#;
        let headless = r#"{"name":"child","mode":"headless"}"#;
        assert_eq!(
            question_owner(&link, mode_record(Some(headed), "child")),
            "Slayer"
        );
        assert_eq!(
            question_owner(&link, mode_record(Some(headless), "child")),
            "w1:p1@h"
        );
        for record in [
            None,
            Some("invalid"),
            Some(r#"{"name":"old-child","mode":"headless"}"#),
        ] {
            assert_eq!(
                question_owner(&link, mode_record(record, "child")),
                "unknown"
            );
        }
    }

    #[test]
    fn scope_keeps_root_closed_children_and_nested_descendants() {
        let issues = vec![
            json!({"id":"grandchild", "parent":"closed-child", "status":"open"}),
            json!({"id":"closed-child", "parent":"root", "status":"closed"}),
            json!({"id":"root", "status":"closed"}),
            json!({"id":"root.lookalike", "parent":"other"}),
            json!({"id":"edge-child", "dependencies":[{"type":"parent-child","depends_on_id":"root"}]}),
        ];
        assert_eq!(
            scope_ids("root", &issues).unwrap(),
            BTreeSet::from([
                "root".into(),
                "closed-child".into(),
                "grandchild".into(),
                "edge-child".into()
            ])
        );
        assert!(scope_ids("typo", &issues).is_err());
        assert!(scope_ids("root", &[]).is_err());
    }

    #[test]
    fn corrupt_source_is_unknown_not_empty() {
        let dir = tempfile::tempdir().unwrap();
        assert!(json_files(&dir.path().join("missing")).is_err());
        assert!(json_files(dir.path()).unwrap().is_empty());
        std::fs::write(dir.path().join("bad"), "oops").unwrap();
        assert!(json_files(dir.path()).is_err());
    }
}
