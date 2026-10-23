//! Is the target pane still there, and who coordinates its project
//! (vega-lhc.129, docs/design/vegad.md, Reachability).
//!
//! - `send` reads the target host's beat and the target's agent entry (two
//!   direct gets, one round trip): a fresh host without the pane means the
//!   pane is gone (`pane_gone`); a stale or unknown host proves nothing, so
//!   send warns and mails anyway.
//! - `send --uac <project>` resolves the project's UAC across hosts: the
//!   UAC registrations of fresh hosts whose pane is still open.
//! - The hint of a gone pane names the project's current UAC; vegad's
//!   bounce of mail a closed pane never read carries the same hint.
//!
//! No I/O here but `fetch` and `read`.

use std::time::Duration;

use vega_herdr::Failure;
use vega_registry::{self as reg, View};

/// How long send waits for the registry before it mails unchecked.
pub const WAIT: Duration = Duration::from_secs(2);

#[derive(Debug, PartialEq)]
pub enum Reach {
    /// The host is fresh and lists the pane.
    Open,
    /// The host is fresh and does not list the pane.
    Gone,
    /// The registry cannot tell (why): send warns and mails anyway.
    Unknown(String),
}

/// The target's state as the registry sees it.
pub fn reach(view: &View, host: &str, pane: &str) -> Reach {
    let h = reg::token(host);
    let Some(at) = view.beat_at(&h) else {
        return Reach::Unknown(format!("the registry has no entry for host {host}"));
    };
    if !view.fresh(&h) {
        let ago = (unix_now() - at).max(0);
        return Reach::Unknown(format!(
            "the registry entry of host {host} is stale (last beat {} ago)",
            human(ago)
        ));
    }
    match view.status(&h, pane) {
        Some(Some(_)) => Reach::Open,
        _ => Reach::Gone,
    }
}

/// The keys `reach` needs: the host's beat and the pane's agent entry.
pub fn keys(host: &str, pane: &str) -> [String; 2] {
    [reg::beat_key(host), reg::agent_key(host, pane)]
}

/// `keys` by direct gets, at once: one round trip on the open connection.
pub async fn fetch(client: &async_nats::Client, host: &str, pane: &str) -> Result<View, String> {
    let [beat, agent] = keys(host, pane);
    let (b, a) = futures_util::join!(reg::get_one(client, &beat), reg::get_one(client, &agent));
    let mut view = View::default();
    for (k, v) in [(beat, b?), (agent, a?)] {
        if let Some(v) = v {
            view.keys.push(k.clone());
            view.entries.insert(k, v);
        }
    }
    Ok(view)
}

/// Beats, agents and UAC registrations of every host.
pub async fn read(client: &async_nats::Client) -> Result<View, String> {
    let kv = reg::bucket(client).await?;
    reg::read_kinds(client, &kv, Some(&["beat", "agent", "uac"])).await
}

/// A project key's folder name, stripping `/.git` or `/.bare`.
pub fn project_name(key: &str) -> &str {
    let dir = key.trim_end_matches('/');
    let dir = dir
        .strip_suffix("/.git")
        .or_else(|| dir.strip_suffix("/.bare"))
        .unwrap_or(dir);
    dir.rsplit('/').next().unwrap_or(dir)
}

/// The project's v2 UAC registration on that host, else the agent's
/// schema-2 project ID. Missing/empty project or schema never falls back to
/// repo_key: it is location metadata only. Legacy UAC keys do not participate.
pub fn project_of(view: &View, host: &str, pane: &str) -> Option<String> {
    let h = reg::token(host);
    view.uac_entries()
        .into_iter()
        .find(|(eh, v)| *eh == h && v["pane"] == pane)
        .and_then(|(_, v)| v["project"].as_str().map(String::from))
        .or_else(|| {
            let agent = view.entries.get(&reg::agent_key(host, pane))?;
            (agent["schema"] == 2).then_some(())?;
            agent["project"]
                .as_str()
                .filter(|k| !k.is_empty())
                .map(String::from)
        })
}

/// (host, pane) of `project`'s UACs on fresh hosts whose pane is open.
pub fn live_uacs(view: &View, project: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = view
        .uacs(project)
        .iter()
        .filter_map(|v| {
            let pane = v["pane"].as_str()?;
            let host = v["host"].as_str()?;
            view.agent(&reg::token(host), pane)
                .map(|_| (host.to_string(), pane.to_string()))
        })
        .collect();
    out.sort();
    out.dedup();
    out
}

/// The project keys `query` names: the key itself, `here` (the caller's
/// project) for `.`, else keys whose folder or published origin name is `query`.
pub fn projects(view: &View, query: &str, here: Option<&str>) -> Vec<String> {
    if query == "." {
        return here.map(String::from).into_iter().collect();
    }
    let mut keys: Vec<String> = view
        .uac_entries()
        .into_iter()
        .filter_map(|(_, v)| {
            let key = v["project"].as_str()?;
            (key == query
                || v["folder_name"].as_str() == Some(query)
                || project_name(key) == query
                || v["origin_name"].as_str() == Some(query)
                || v["aliases"]
                    .as_array()
                    .is_some_and(|a| a.iter().any(|v| v.as_str() == Some(query))))
            .then(|| key.to_string())
        })
        .collect();
    keys.sort();
    keys.dedup();
    keys
}

/// `send --uac <query>`: (host, pane, project) of the one live UAC.
pub fn resolve(
    view: &View,
    query: &str,
    here: Option<&str>,
) -> Result<(String, String, String), Failure> {
    let keys = projects(view, query, here);
    let mut found: Vec<(String, String, String)> = keys
        .iter()
        .flat_map(|k| {
            live_uacs(view, k)
                .into_iter()
                .map(move |(h, p)| (h, p, k.clone()))
        })
        .collect();
    match found.len() {
        1 => Ok(found.remove(0)),
        0 => Err(Failure::new(
            "no_uac",
            match keys.as_slice() {
                [] if query == "." => "this directory is in no git project".to_string(),
                [] => format!("no project named {query} has a UAC in the registry"),
                ks => format!("{} has no UAC with an open pane on a host the registry sees", ks.join(", ")),
            },
            "run `vega uac` to see the coordinators; ask the user who coordinates it",
        )),
        _ => Err(Failure::new(
            "ambiguous_uac",
            format!(
                "{query} matches several UACs: {}",
                found
                    .iter()
                    .map(|(h, p, k)| format!("{p} on {h} ({k})"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            "pass the full project key to --uac, or send to one: vega post send --host <host> <pane> \"<text>\"",
        )),
    }
}

/// What to do instead of mailing the gone pane: its project's UAC.
pub fn gone_hint(view: Option<&View>, project: Option<&str>, gone: (&str, &str)) -> String {
    let fallback =
        "run `vega uac` to find who coordinates the work now; a closed pane never reads its mail";
    let (Some(view), Some(project)) = (view, project) else {
        return fallback.into();
    };
    let uacs: Vec<(String, String)> = live_uacs(view, project)
        .into_iter()
        .filter(|(h, p)| {
            (reg::token(h).as_str(), p.as_str()) != (reg::token(gone.0).as_str(), gone.1)
        })
        .collect();
    let name = project_name(project);
    match uacs.as_slice() {
        [(h, p)] => {
            format!("the UAC of {name} is {p} on {h} now: vega post send --uac {name} \"<text>\"")
        }
        [] => format!("{name} has no UAC with an open pane; {fallback}"),
        many => format!(
            "{name} has several UACs ({}); {fallback}",
            many.iter()
                .map(|(h, p)| format!("{p} on {h}"))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

/// `1h 5m`, `3m`, `40s`.
fn human(secs: i64) -> String {
    match secs {
        s if s >= 3600 => format!("{}h {}m", s / 3600, s % 3600 / 60),
        s if s >= 60 => format!("{}m", s / 60),
        s => format!("{s}s"),
    }
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    /// A beat `ago` seconds old.
    fn beat(ago: i64) -> Value {
        json!({"schema": 2,"at": unix_now() - ago})
    }

    const VEGA: &str = "github.com/durandom/vega";

    /// proteus fresh with its UAC w3V:p1 open and an old registration of
    /// the closed w30:p1; ganymede fresh with an agent; titan stale.
    fn view() -> View {
        let mut v = View::default();
        let mut put = |k: String, val: Value| {
            v.keys.push(k.clone());
            v.entries.insert(k, val);
        };
        put(reg::beat_key("proteus"), beat(5));
        put(reg::beat_key("ganymede"), beat(5));
        put(reg::beat_key("titan"), beat(600));
        put(
            reg::agent_key("proteus", "w3V:p1"),
            json!({"schema": 2,"pane": "w3V:p1", "status": "idle", "project": VEGA}),
        );
        put(
            reg::agent_key("ganymede", "w5S:p1"),
            json!({"schema": 2,"pane": "w5S:p1", "status": "working", "project": "github.com/team/spellkave"}),
        );
        put(
            reg::agent_key("titan", "w1:p1"),
            json!({"schema": 2,"pane": "w1:p1", "status": "idle"}),
        );
        put(
            reg::uac_key("proteus", VEGA),
            json!({"schema": 2,"pane": "w3V:p1", "host": "proteus", "project": VEGA}),
        );
        put(
            reg::uac_key("proteus", "github.com/team/spellkave"),
            json!({"schema": 2,"pane": "w30:p1", "host": "proteus", "project": "github.com/team/spellkave"}),
        );
        v
    }

    #[test]
    fn a_fresh_host_without_the_pane_proves_it_gone() {
        let v = view();
        assert_eq!(reach(&v, "proteus", "w3V:p1"), Reach::Open);
        assert_eq!(reach(&v, "proteus", "w30:p1"), Reach::Gone);
    }

    #[test]
    fn a_stale_or_unknown_host_proves_nothing() {
        let v = view();
        let Reach::Unknown(why) = reach(&v, "titan", "w9:p9") else {
            panic!("stale host must not prove a pane gone");
        };
        assert!(why.contains("stale (last beat 10m ago)"), "{why}");
        assert_eq!(
            reach(&v, "deimos", "w1:p1"),
            Reach::Unknown("the registry has no entry for host deimos".into())
        );
        // A view without a beat (registry off, the fetch found nothing).
        assert!(matches!(
            reach(&View::default(), "proteus", "w3V:p1"),
            Reach::Unknown(_)
        ));
    }

    #[test]
    fn project_names_and_queries() {
        assert_eq!(project_name(VEGA), "vega");
        assert_eq!(project_name("~/src/x"), "x");
        assert_eq!(project_name("~/src/x/.bare/"), "x");
        assert_eq!(project_name("/home/other/src/x/.git/"), "x");
        assert_eq!(project_name("~/src/x.git"), "x.git");
        let v = view();
        assert_eq!(projects(&v, "vega", None), vec![VEGA.to_string()]);
        assert_eq!(projects(&v, VEGA, None), vec![VEGA.to_string()]);
        assert_eq!(projects(&v, ".", Some(VEGA)), vec![VEGA.to_string()]);
        assert!(projects(&v, ".", None).is_empty());
        assert!(projects(&v, "nope", None).is_empty());
    }

    #[test]
    fn layout_and_origin_names_resolve_without_guessing() {
        let mut v = view();
        for (host, pane, key, origin) in [
            (
                "ganymede",
                "w5S:p1",
                "~/src/durandom/spellkave/.bare",
                "spellkave",
            ),
            (
                "proteus",
                "w43:p1",
                "~/src/durandom/orrery/main/.git",
                "orrery",
            ),
        ] {
            v.entries.insert(
                reg::agent_key(host, pane),
                json!({"schema": 2,"pane": pane}),
            );
            v.entries.insert(
                reg::uac_key(host, key),
                json!({"schema": 2,
                    "host": host, "pane": pane, "project": key, "origin_name": origin,
                }),
            );
            for query in [origin, key, project_name(key)] {
                assert_eq!(
                    resolve(&v, query, None).unwrap(),
                    (host.into(), pane.into(), key.into())
                );
            }
            assert_eq!(resolve(&v, ".", Some(key)).unwrap().1, pane);
        }
        // Old hosts do not publish origin_name; .bare folder names still work.
        v.entries
            .get_mut(&reg::uac_key("ganymede", "~/src/durandom/spellkave/.bare"))
            .unwrap()
            .as_object_mut()
            .unwrap()
            .remove("origin_name");
        assert_eq!(resolve(&v, "spellkave", None).unwrap().1, "w5S:p1");
        assert!(gone_hint(
            Some(&v),
            Some("~/src/durandom/spellkave/.bare"),
            ("proteus", "w30:p1")
        )
        .contains("--uac spellkave"));

        // A folder name colliding with another project's origin is ambiguous.
        v.entries.get_mut(&reg::uac_key("proteus", VEGA)).unwrap()["origin_name"] =
            json!("spellkave");
        let error = resolve(&v, "spellkave", None).unwrap_err();
        assert_eq!(error.code, "ambiguous_uac");
        assert!(error.message.contains("w5S:p1 on ganymede"));
        assert!(error.message.contains("w3V:p1 on proteus"));
        assert_eq!(resolve(&v, VEGA, None).unwrap().1, "w3V:p1");
    }

    #[test]
    fn uac_resolves_across_hosts_to_an_open_pane() {
        let v = view();
        assert_eq!(
            resolve(&v, "vega", None).unwrap(),
            ("proteus".into(), "w3V:p1".into(), VEGA.into())
        );
        // spellkave's only UAC pane is closed: no live UAC.
        let f = resolve(&v, "spellkave", None).unwrap_err();
        assert_eq!(f.code, "no_uac");
        assert_eq!(resolve(&v, "nope", None).unwrap_err().code, "no_uac");
    }

    #[test]
    fn several_live_uacs_are_listed_not_guessed() {
        let mut v = view();
        v.entries.insert(
            reg::agent_key("ganymede", "w2:p1"),
            json!({"schema": 2,"pane": "w2:p1", "status": "idle"}),
        );
        v.entries.insert(
            reg::uac_key("ganymede", VEGA),
            json!({"schema": 2,"pane": "w2:p1", "host": "ganymede", "project": VEGA}),
        );
        let f = resolve(&v, "vega", None).unwrap_err();
        assert_eq!(f.code, "ambiguous_uac");
        assert!(f.message.contains("w2:p1 on ganymede"), "{}", f.message);
        assert!(f.message.contains("w3V:p1 on proteus"), "{}", f.message);
    }

    #[test]
    fn a_uac_on_a_stale_host_does_not_count() {
        let mut v = view();
        v.entries.insert(
            reg::uac_key("titan", "~/src/t/.git"),
            json!({"schema": 2,"pane": "w1:p1", "host": "titan", "project": "~/src/t/.git"}),
        );
        assert_eq!(resolve(&v, "t", None).unwrap_err().code, "no_uac");
    }

    #[test]
    fn the_gone_hint_names_the_current_uac() {
        let mut v = view();
        // The closed w30:p1 was spellkave's UAC; a new one runs on ganymede.
        assert_eq!(
            project_of(&v, "proteus", "w30:p1").as_deref(),
            Some("github.com/team/spellkave")
        );
        v.entries.insert(
            reg::uac_key("ganymede", "github.com/team/spellkave"),
            json!({"schema": 2,"pane": "w5S:p1", "host": "ganymede", "project": "github.com/team/spellkave"}),
        );
        let hint = gone_hint(
            Some(&v),
            Some("github.com/team/spellkave"),
            ("proteus", "w30:p1"),
        );
        assert_eq!(
            hint,
            "the UAC of spellkave is w5S:p1 on ganymede now: vega post send --uac spellkave \"<text>\""
        );
        let none = gone_hint(
            Some(&view()),
            Some("github.com/team/spellkave"),
            ("proteus", "w30:p1"),
        );
        assert!(
            none.starts_with("spellkave has no UAC with an open pane"),
            "{none}"
        );
        assert!(gone_hint(None, None, ("proteus", "w30:p1")).contains("vega uac"));
    }

    #[test]
    fn v2_routes_by_id_and_published_alias_without_remote_path_io() {
        let mut v = view();
        // Different host-local paths and folder aliases, same remote identity.
        v.entries.insert(reg::agent_key("ganymede", "w2:p1"), json!({"schema":2,"pane":"w2:p1","status":"idle","project":VEGA,"repo_key":"/srv/repos/unavailable/.bare"}));
        v.entries.insert(reg::uac_key("ganymede", VEGA), json!({"schema":2,"host":"ganymede","pane":"w2:p1","project":VEGA,"folder_name":"different-folder","origin_name":"vega"}));
        assert_eq!(projects(&v, "vega", None), vec![VEGA.to_string()]);
        assert_eq!(resolve(&v, VEGA, None).unwrap_err().code, "ambiguous_uac");
        v.entries.remove(&reg::agent_key("proteus", "w3V:p1"));
        for query in [VEGA, "different-folder", "vega"] {
            assert_eq!(resolve(&v, query, None).unwrap().1, "w2:p1");
        }
        assert_eq!(project_of(&v, "ganymede", "w2:p1").as_deref(), Some(VEGA));
        // Legacy-only publications never participate, even with a v2 payload.
        v.entries.remove(&reg::uac_key("ganymede", VEGA));
        v.entries.insert(
            "ganymede.uac.old".into(),
            json!({"schema":2,"host":"ganymede","pane":"w2:p1","project":VEGA}),
        );
        assert_eq!(resolve(&v, VEGA, None).unwrap_err().code, "no_uac");
        let agent = v
            .entries
            .get_mut(&reg::agent_key("ganymede", "w2:p1"))
            .unwrap();
        agent.as_object_mut().unwrap().remove("project");
        assert_eq!(project_of(&v, "ganymede", "w2:p1"), None);
    }

    #[test]
    fn fetch_keys_are_the_beat_and_the_agent() {
        assert_eq!(
            keys("proteus", "w30:p1"),
            [
                "proteus.beat".to_string(),
                "proteus.agent.w30_p1".to_string()
            ]
        );
    }
}
