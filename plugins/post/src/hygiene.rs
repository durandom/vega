//! vegad's hygiene pass (docs/design/vegad.md, Hygiene): badges pile up
//! when nobody settles them. vegad mails the project's UAC (a) once per
//! waiting state, about a headed pane that has waited on the Slayer over
//! WAITING_AFTER, and (b) at most once a day and once per list, the stale
//! items: decisions older than DECISION_AFTER, panes waiting, unread mail
//! and notes over WAITING_AFTER. vegad only reports; the UAC asks the
//! Slayer (vega uac --skill) and never cleans up on its own.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};
use vega_herdr::badges::{Decision, Waiting};
use vega_herdr::capacity::duration;

/// The `Vega-From-Agent` of hygiene mail: a vega tool, no reply path.
pub const AGENT: &str = "tool vega-hygiene";

/// A pane waiting on the Slayer, unread mail or a note gets stale after this.
pub const WAITING_AFTER: i64 = 24 * 3600;
/// An open decision gets stale after this.
pub const DECISION_AFTER: i64 = 24 * 3600;
/// At most one hygiene list per UAC in this time.
pub const DAILY: i64 = 24 * 3600;

/// One project with a UAC pane on this host, as vegad sees it now.
#[derive(Debug, Clone, Default)]
pub struct Project {
    pub key: String,
    /// The UAC pane; `None` while it is not live (nothing is sent).
    pub uac: Option<String>,
    /// Its headed panes waiting on the Slayer (badges::Waiting).
    pub waiting: Vec<Waiting>,
    pub decisions: Vec<Decision>,
    /// Its panes with unread mail: (pane, oldest arrival in Unix s).
    pub mail: Vec<(String, i64)>,
    /// Its unread notes: (`<host>=<id>`, Unix s, title).
    pub notes: Vec<(String, i64, String)>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Since {
    attention: String,
    since: i64,
    mailed: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Daily {
    at: i64,
    digest: String,
}

/// What vegad remembers across restarts: when each pane started waiting
/// and whether its UAC heard, and each UAC's last list.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct State {
    waiting: BTreeMap<String, Since>,
    daily: BTreeMap<String, Daily>,
}

/// `$XDG_STATE_HOME/vega/hygiene.json`.
pub fn file(state: &Path) -> PathBuf {
    state.join("hygiene.json")
}

/// vegad's pass: the memory on disk, saved when it changed.
pub struct Pass {
    path: Option<PathBuf>,
    state: Option<State>,
}

impl Pass {
    pub fn new(state: Option<&Path>) -> Pass {
        let path = state.map(file);
        let state = match path
            .as_deref()
            .map(State::load)
            .unwrap_or_else(|| Ok(State::default()))
        {
            Ok(state) => Some(state),
            Err(error) => {
                crate::log(&format!("hygiene: paused: {error}; preserve hygiene.json for the journaled cutover or repair"));
                None
            }
        };
        Pass { path, state }
    }

    /// The mails due at `now`, as (UAC pane, text).
    pub fn run(&mut self, now: i64, projects: &[Project]) -> Vec<(String, String)> {
        let Some(state) = self.state.as_mut() else {
            return Vec::new();
        };
        let before = state.clone();
        let out = state.tick(now, projects);
        if let Some(path) = self.path.as_deref().filter(|_| *state != before) {
            if let Err(e) = state.save(path) {
                crate::log(&format!("hygiene: cannot save {}: {e}", path.display()));
            }
        }
        out
    }
}

impl State {
    pub fn load(path: &Path) -> Result<State, String> {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(State::default()),
            Err(_) => return Err("cannot read hygiene state".into()),
        };
        let v: Value = serde_json::from_str(&text).map_err(|_| "invalid hygiene JSON")?;
        if v["schema"] != 2 {
            return Err("hygiene state needs the v2 migration".into());
        }
        if !v["waiting"].is_object() || !v["daily"].is_object() {
            return Err("invalid hygiene state maps".into());
        }
        let obj = |k: &str| v[k].as_object().cloned().unwrap_or_default();
        let s = |x: &Value, k: &str| x[k].as_str().unwrap_or_default().to_string();
        Ok(State {
            waiting: obj("waiting")
                .into_iter()
                .map(|(p, x)| {
                    let w = Since {
                        attention: s(&x, "attention"),
                        since: x["since"].as_i64().unwrap_or(0),
                        mailed: x["mailed"].as_bool().unwrap_or(false),
                    };
                    (p, w)
                })
                .collect(),
            daily: obj("daily")
                .into_iter()
                .map(|(k, x)| {
                    let d = Daily {
                        at: x["at"].as_i64().unwrap_or(0),
                        digest: s(&x, "digest"),
                    };
                    (k, d)
                })
                .collect(),
        })
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        let waiting: serde_json::Map<String, Value> = self
            .waiting
            .iter()
            .map(|(p, w)| {
                let v = json!({"attention": w.attention, "since": w.since, "mailed": w.mailed});
                (p.clone(), v)
            })
            .collect();
        let daily: serde_json::Map<String, Value> = self
            .daily
            .iter()
            .map(|(k, d)| (k.clone(), json!({"at": d.at, "digest": d.digest})))
            .collect();
        let body = json!({"schema": 2, "waiting": waiting, "daily": daily});
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension(format!("tmp{}", std::process::id()));
        std::fs::write(&tmp, body.to_string())?;
        std::fs::rename(&tmp, path)
    }

    /// The mails due at `now`, as (UAC pane, text); updates the memory.
    pub fn tick(&mut self, now: i64, projects: &[Project]) -> Vec<(String, String)> {
        let mut out = Vec::new();
        // A pane leaves the memory when it stops waiting; a new attention
        // is a new state with its own clock.
        let mut seen = BTreeMap::new();
        for w in projects.iter().flat_map(|p| &p.waiting) {
            if seen.contains_key(&w.pane) {
                continue;
            }
            let s = match self.waiting.remove(&w.pane) {
                Some(s) if s.attention == w.attention => s,
                _ => Since {
                    attention: w.attention.clone(),
                    since: now,
                    mailed: false,
                },
            };
            seen.insert(w.pane.clone(), s);
        }
        self.waiting = seen;
        for p in projects {
            let Some(uac) = &p.uac else {
                continue;
            };
            for w in &p.waiting {
                let Some(s) = self.waiting.get_mut(&w.pane) else {
                    continue;
                };
                if !s.mailed && now - s.since >= WAITING_AFTER {
                    s.mailed = true;
                    out.push((uac.clone(), waiting_mail(w, now - s.since)));
                }
            }
            let items = self.items(now, p, uac);
            let digest = items
                .iter()
                .map(|(k, _)| k.as_str())
                .collect::<Vec<_>>()
                .join(" ");
            let last = self.daily.entry(p.key.clone()).or_default();
            if items.is_empty() {
                // A list that comes back after it emptied is a new state.
                last.digest.clear();
            } else if digest != last.digest && now - last.at >= DAILY {
                *last = Daily { at: now, digest };
                let lines: Vec<String> = items.into_iter().map(|(_, l)| l).collect();
                out.push((uac.clone(), list_mail(&p.key, &lines)));
            }
        }
        self.daily
            .retain(|k, _| projects.iter().any(|p| &p.key == k));
        out
    }

    /// The stale items of `p`, sorted: (dedupe key, mail line).
    fn items(&self, now: i64, p: &Project, uac: &str) -> Vec<(String, String)> {
        let mut items = Vec::new();
        for d in &p.decisions {
            if d.created > 0 && now - d.created >= DECISION_AFTER {
                items.push((
                    format!("decision:{}", d.id),
                    format!(
                        "decision {} ({}): {}",
                        d.id,
                        duration(now - d.created),
                        d.title
                    ),
                ));
            }
        }
        for w in &p.waiting {
            let Some(s) = self.waiting.get(&w.pane) else {
                continue;
            };
            if now - s.since >= WAITING_AFTER {
                items.push((
                    format!("waiting:{}:{}", w.pane, s.since),
                    format!(
                        "waiting {} {} ({}, {}): {}",
                        w.pane,
                        w.name,
                        w.attention,
                        duration(now - s.since),
                        w.title
                    ),
                ));
            }
        }
        for (pane, since) in &p.mail {
            if pane != uac && now - since >= WAITING_AFTER {
                items.push((
                    format!("mail:{pane}"),
                    format!("mail {pane} (unread {})", duration(now - since)),
                ));
            }
        }
        for (id, at, title) in &p.notes {
            if now - at >= WAITING_AFTER {
                items.push((
                    format!("note:{id}"),
                    format!("note {id} ({}): {title}", duration(now - at)),
                ));
            }
        }
        items.sort();
        items
    }
}

fn waiting_mail(w: &Waiting, waited: i64) -> String {
    let title = match w.title.as_str() {
        "" => String::new(),
        t => format!(": {t}"),
    };
    format!(
        "Pane {} ({}) has waited on the Slayer for {} ({}){title}.\n\
         Record its open question as a slayer-decision bead under your tracker, quoting it, \
         then close the pane: vega mux close {}. Its worktree stays.",
        w.pane,
        w.name,
        duration(waited),
        w.attention,
        w.pane
    )
}

fn list_mail(key: &str, lines: &[String]) -> String {
    let mut m = format!(
        "Hygiene for {}: {} stale item{}.\n\
         Ask the Slayer once, as one batched question (close / keep?), and act only on the \
         answer; never clean up on your own.",
        key,
        lines.len(),
        if lines.len() == 1 { "" } else { "s" }
    );
    for l in lines {
        m.push_str(&format!("\n  {l}"));
    }
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    const H: i64 = 3600;
    const T0: i64 = 1_000_000_000;

    fn waiting(pane: &str, attention: &str) -> Waiting {
        Waiting {
            pane: pane.into(),
            name: "flist".into(),
            attention: attention.into(),
            title: "asks: tabs?".into(),
        }
    }

    fn project(waiting: Vec<Waiting>) -> Project {
        Project {
            key: "github.com/team/vega".into(),
            uac: Some("w1:p1".into()),
            waiting,
            ..Default::default()
        }
    }

    #[test]
    fn waiting_pane_mails_once_after_a_day() {
        let mut s = State::default();
        let p = [project(vec![waiting("w1:p2", "asking")])];
        assert!(s.tick(T0, &p).is_empty());
        assert!(s.tick(T0 + 24 * H - 1, &p).is_empty(), "under the limit");
        let out = s.tick(T0 + 24 * H, &p);
        let pane: Vec<_> = out.iter().filter(|(_, t)| t.starts_with("Pane")).collect();
        assert_eq!(pane.len(), 1);
        assert_eq!(pane[0].0, "w1:p1");
        assert!(pane[0]
            .1
            .contains("w1:p2 (flist) has waited on the Slayer for 1d (asking)"));
        assert!(pane[0].1.contains("vega mux close w1:p2"));
        let later = s.tick(T0 + 30 * H, &p);
        assert!(
            !later.iter().any(|(_, t)| t.starts_with("Pane")),
            "same state"
        );
        // A new attention is a new state with its own clock.
        let q = [project(vec![waiting("w1:p2", "input")])];
        assert!(s.tick(T0 + 31 * H, &q).is_empty());
        let again = s.tick(T0 + 55 * H, &q);
        assert!(again.iter().any(|(_, t)| t.starts_with("Pane")));
        // Gone and back: the clock starts again.
        s.tick(T0 + 56 * H, &[project(vec![])]);
        s.tick(T0 + 57 * H, &q);
        assert_eq!(s.waiting["w1:p2"].since, T0 + 57 * H);
        // In two projects: one clock.
        let mut other = project(vec![waiting("w1:p2", "input")]);
        other.key = "github.com/team/sk".into();
        s.tick(T0 + 58 * H, &[q[0].clone(), other]);
        assert_eq!(s.waiting["w1:p2"].since, T0 + 57 * H);
    }

    #[test]
    fn no_live_uac_no_mail_until_it_is_back() {
        let mut s = State::default();
        let mut p = project(vec![waiting("w1:p2", "asking")]);
        s.tick(T0, std::slice::from_ref(&p));
        p.uac = None;
        assert!(s.tick(T0 + 25 * H, std::slice::from_ref(&p)).is_empty());
        p.uac = Some("w1:p1".into());
        assert!(!s.tick(T0 + 26 * H, std::slice::from_ref(&p)).is_empty());
    }

    #[test]
    fn list_holds_stale_items_only_and_waits_a_day_and_a_change() {
        let mut s = State::default();
        let now = T0 + 10 * 24 * H;
        let decision = |id: &str, age: i64| Decision {
            id: id.into(),
            title: format!("{id}?"),
            created: now - age,
        };
        let mut p = project(vec![]);
        p.decisions = vec![
            decision("v-1.1", 24 * H),
            decision("v-1.2", 24 * H - 1),
            Decision {
                id: "v-1.3".into(),
                ..Default::default()
            },
        ];
        p.mail = vec![
            ("w1:p3".into(), now - 24 * H),
            ("w1:p4".into(), now - 24 * H + 1),
            ("w1:p1".into(), now - 48 * H),
        ];
        p.notes = vec![
            ("proteus=7".into(), now - 25 * H, "bd sync stopped".into()),
            ("proteus=8".into(), now - H, "fresh".into()),
        ];
        let out = s.tick(now, std::slice::from_ref(&p));
        assert_eq!(out.len(), 1);
        let (to, text) = &out[0];
        assert_eq!(to, "w1:p1");
        assert!(
            text.starts_with("Hygiene for github.com/team/vega: 3 stale items."),
            "{text}"
        );
        assert!(text.contains("one batched question (close / keep?)"));
        assert!(text.contains("\n  decision v-1.1 (1d): v-1.1?"));
        assert!(text.contains("\n  mail w1:p3 (unread 1d)"));
        assert!(text.contains("\n  note proteus=7 (1d1h): bd sync stopped"));
        for fresh in ["v-1.2", "v-1.3", "w1:p4", "proteus=8"] {
            assert!(!text.contains(fresh), "{fresh} is not stale");
        }
        assert!(!text.contains("mail w1:p1"), "the UAC's own mail");
        // A changed list only after a day (v-1.2, w1:p4 and proteus=8 aged).
        assert!(s.tick(now + 20 * H, std::slice::from_ref(&p)).is_empty());
        assert_eq!(s.tick(now + 24 * H, std::slice::from_ref(&p)).len(), 1);
        // The same list never again.
        assert!(s.tick(now + 60 * H, std::slice::from_ref(&p)).is_empty());
        p.decisions.push(decision("v-1.4", 8 * 24 * H));
        assert_eq!(s.tick(now + 61 * H, std::slice::from_ref(&p)).len(), 1);
        // Emptied and back: a new state.
        let empty = project(vec![]);
        assert!(s
            .tick(now + 80 * H, std::slice::from_ref(&empty))
            .is_empty());
        assert_eq!(s.tick(now + 85 * H, std::slice::from_ref(&p)).len(), 1);
    }

    #[test]
    fn list_names_panes_waiting_over_a_day() {
        let mut s = State::default();
        let p = [project(vec![waiting("w1:p2", "silent")])];
        s.tick(T0, &p);
        let out = s.tick(T0 + 24 * H, &p);
        assert_eq!(out.len(), 2, "the pane mail and the list");
        assert!(out[1]
            .1
            .contains("\n  waiting w1:p2 flist (silent, 1d): asks: tabs?"));
    }

    #[test]
    fn state_survives_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let f = file(dir.path());
        let mut s = State::default();
        let mut p = project(vec![waiting("w1:p2", "asking")]);
        p.notes = vec![("proteus=7".into(), T0 - 25 * H, "x".into())];
        s.tick(T0, std::slice::from_ref(&p));
        s.save(&f).unwrap();
        let back = State::load(&f).unwrap();
        assert_eq!(back, s);
        assert_eq!(
            State::load(&dir.path().join("none.json")).unwrap(),
            State::default()
        );
    }

    #[test]
    fn legacy_state_is_not_overwritten_when_a_project_would_dirty_the_pass() {
        let dir = tempfile::tempdir().unwrap();
        let path = file(dir.path());
        let original = br#"{ "waiting": {"p1":{"since":1,"mailed":true}}, "daily": {"~/old/.git":{"at":2,"digest":"keep"}} }
"#;
        std::fs::write(&path, original).unwrap();
        let mut pass = Pass::new(Some(dir.path()));
        let project = project(vec![waiting("p2", "asking")]);
        assert!(pass.run(T0, std::slice::from_ref(&project)).is_empty());
        assert!(pass.run(T0 + WAITING_AFTER + 1, &[project]).is_empty());
        assert_eq!(std::fs::read(&path).unwrap(), original);
    }
}
