//! The Slayer's badges: unread notes, open decisions and panes waiting on
//! the Slayer, drawn with the theme's symbols (crate::theme: Nerd Font
//! glyphs, emoji or the user's own). vegad counts them per project, labels
//! the UAC pane (`<atom> UAC <email> 1 ...`) and saves them here; `vega
//! board` reads the saved counts for its top bar, so label and board come
//! from one place (docs/design/vegad.md, Badges).

use std::path::PathBuf;

use serde_json::{json, Value};

use crate::theme::{Symbols, Topic, DECISIONS, NOTES, WAITING};

/// Label of the project's one UAC tracker epic.
pub const TRACKER_LABEL: &str = "uac-tracker";
/// Label of a decision for the Slayer under the tracker.
pub const DECISION_LABEL: &str = "slayer-decision";

/// A saved count older than this is vegad's past, not its present.
pub const STALE_SECS: i64 = 60;

/// A headed pane waiting on the Slayer, as vegad labels it: what the
/// board's Inbox lists (docs/design/board-ui.md).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Waiting {
    pub pane: String,
    /// The agent's display name.
    pub name: String,
    /// `asking`, `silent` or `input`.
    pub attention: String,
    /// The pane's sidebar title (the question, the dialog), maybe empty.
    pub title: String,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Badges {
    pub notes: usize,
    pub decisions: usize,
    pub waiting: usize,
}

impl Badges {
    pub fn is_zero(&self) -> bool {
        *self == Badges::default()
    }

    /// The non-zero badges in order notes, decisions, waiting, each its
    /// symbol, a space and the count (`<email> 1 <scale> 2`); empty when all
    /// are zero.
    pub fn text(&self, s: &Symbols) -> String {
        self.parts(s)
            .into_iter()
            .map(|(_, t)| t)
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// The non-zero badges as in `text`, one per topic, so a caller can place
    /// each one (the board makes them clickable).
    pub fn parts(&self, s: &Symbols) -> Vec<(&'static Topic, String)> {
        [
            (&NOTES, self.notes),
            (&DECISIONS, self.decisions),
            (&WAITING, self.waiting),
        ]
        .into_iter()
        .filter(|(_, n)| *n > 0)
        .map(|(t, n)| (t, format!("{}{n}", s.icon(t))))
        .collect()
    }

    /// `name` plus the badges after a space, if any.
    pub fn after(&self, name: &str, s: &Symbols) -> String {
        match self.text(s) {
            t if t.is_empty() => name.to_string(),
            t => format!("{name} {t}"),
        }
    }

    /// The UAC pane's sidebar name: `<atom> UAC <email> 1 ...`.
    pub fn uac_label(&self, s: &Symbols) -> String {
        self.after(&s.uac_name(), s)
    }

    /// `herdr pane report-metadata` args for the UAC pane's sidebar tokens,
    /// one per topic so a herdr `[ui.sidebar.agents]` row can colour each
    /// (docs/design/theme-symbols.md): `uac=<atom> UAC` always, `notes`,
    /// `decisions`, `waiting` as `<symbol> <count>`, a zero one cleared.
    pub fn uac_tokens(&self, s: &Symbols) -> Vec<String> {
        let mut args = vec!["--token".into(), format!("uac={}", s.uac_name())];
        args.extend(self.count_tokens(s));
        args
    }

    /// The `notes`, `decisions` and `waiting` tokens as `<symbol> <count>`,
    /// a zero one cleared: for the UAC pane and its project's workspace.
    pub fn count_tokens(&self, s: &Symbols) -> Vec<String> {
        let mut args = Vec::new();
        for (t, n) in [
            (&NOTES, self.notes),
            (&DECISIONS, self.decisions),
            (&WAITING, self.waiting),
        ] {
            match n {
                0 => args.extend(["--clear-token".into(), t.name.to_string()]),
                n => args.extend(["--token".into(), format!("{}={}{n}", t.name, s.icon(t))]),
            }
        }
        args
    }
}

/// An open decision for the Slayer: what the daily hygiene list ages.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Decision {
    pub id: String,
    pub title: String,
    /// `created_at` as Unix seconds; 0 when bd gave none.
    pub created: i64,
}

/// Open decisions from `bd list --label uac-tracker,slayer-decision` with
/// `--label-any` (open beads only): the open `slayer-decision` children of
/// the one open `uac-tracker` epic, as `vega board`'s decisions view counts
/// them. `None` without exactly one tracker or for output that is no list.
pub fn decision_count(json: &str) -> Option<usize> {
    decisions(json).map(|d| d.len())
}

/// The decisions `decision_count` counts.
pub fn decisions(json: &str) -> Option<Vec<Decision>> {
    let v: Value = serde_json::from_str(json.trim()).ok()?;
    let beads = v.as_array()?;
    let open = |b: &&Value| b["status"] != "closed";
    let has = |b: &Value, l: &str| {
        b["labels"]
            .as_array()
            .is_some_and(|ls| ls.iter().any(|x| x == l))
    };
    let trackers: Vec<&str> = beads
        .iter()
        .filter(open)
        .filter(|b| b["issue_type"] == "epic" && has(b, TRACKER_LABEL))
        .filter_map(|b| b["id"].as_str())
        .collect();
    let [tracker] = trackers[..] else {
        return None;
    };
    let s = |b: &Value, k: &str| b[k].as_str().unwrap_or_default().to_string();
    Some(
        beads
            .iter()
            .filter(open)
            .filter(|b| b["parent"] == tracker && has(b, DECISION_LABEL))
            .map(|b| Decision {
                id: s(b, "id"),
                title: s(b, "title"),
                created: b["created_at"]
                    .as_str()
                    .and_then(crate::capacity::parse_rfc3339)
                    .unwrap_or(0),
            })
            .collect(),
    )
}

/// Versioned project-ID storage; pane-keyed records keep their spelling.
fn file(key: &str) -> Option<PathBuf> {
    crate::state::project_file("badges", key)
}

/// vegad: the counts of project ID `key` at
/// `now`, plus the waiting panes behind the `waiting` count.
pub fn save(key: &str, b: &Badges, waiting: &[Waiting], now: i64) {
    let Some(f) = file(key) else {
        return;
    };
    let panes: Vec<Value> = waiting
        .iter()
        .map(
            |w| json!({"pane": w.pane, "name": w.name, "attention": w.attention, "title": w.title}),
        )
        .collect();
    let body = json!({
        "schema": 2, "project": key, "notes": b.notes, "decisions": b.decisions,
        "waiting": b.waiting, "waiting_panes": panes, "at": now,
    });
    let tmp = f.with_extension(format!("tmp{}", std::process::id()));
    let _ = f
        .parent()
        .map(std::fs::create_dir_all)
        .and_then(Result::ok)
        .and_then(|_| std::fs::write(&tmp, body.to_string()).ok())
        .and_then(|_| std::fs::rename(&tmp, &f).ok());
}

/// The board: project `key`'s counts, `None` when vegad has not saved them
/// within STALE_SECS of `now`.
pub fn load(key: &str, now: i64) -> Option<(Badges, Vec<Waiting>)> {
    load_at(key, Some(now))
}

/// Last saved v2 counts, regardless of age. Daemon failures retain these;
/// ordinary board reads still use `load` and its freshness window.
pub fn retained(key: &str) -> Option<(Badges, Vec<Waiting>)> {
    load_at(key, None)
}

fn load_at(key: &str, now: Option<i64>) -> Option<(Badges, Vec<Waiting>)> {
    let text = std::fs::read_to_string(file(key)?).ok()?;
    let v: Value = serde_json::from_str(&text).ok()?;
    if v["schema"] != 2 || v["project"] != key {
        return None;
    }
    parse(&text, now.unwrap_or(v["at"].as_i64()?))
}

fn parse(text: &str, now: i64) -> Option<(Badges, Vec<Waiting>)> {
    let v: Value = serde_json::from_str(text).ok()?;
    if now - v["at"].as_i64()? > STALE_SECS {
        return None;
    }
    let n = |k: &str| v[k].as_u64().unwrap_or(0) as usize;
    let s = |w: &Value, k: &str| w[k].as_str().unwrap_or_default().to_string();
    let waiting = v["waiting_panes"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter(|w| w["pane"].is_string())
                .map(|w| Waiting {
                    pane: s(w, "pane"),
                    name: s(w, "name"),
                    attention: s(w, "attention"),
                    title: s(w, "title"),
                })
                .collect()
        })
        .unwrap_or_default();
    Some((
        Badges {
            notes: n("notes"),
            decisions: n("decisions"),
            waiting: n("waiting"),
        },
        waiting,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn label_shows_only_non_zero_badges_one_space_after_each_icon() {
        let nerd = Symbols::of("nerd");
        let emoji = Symbols::of("emoji");
        let b = |notes, decisions, waiting| Badges {
            notes,
            decisions,
            waiting,
        };
        assert!(b(0, 0, 0).is_zero() && !b(0, 0, 1).is_zero());
        assert_eq!(b(0, 0, 0).uac_label(&nerd), "\u{F0768} UAC");
        assert_eq!(b(1, 0, 0).uac_label(&nerd), "\u{F0768} UAC \u{F01EE} 1");
        assert_eq!(b(0, 2, 0).uac_label(&nerd), "\u{F0768} UAC \u{F05D1} 2");
        assert_eq!(b(0, 0, 3).uac_label(&nerd), "\u{F0768} UAC \u{F256} 3");
        assert_eq!(
            b(1, 2, 3).uac_label(&nerd),
            "\u{F0768} UAC \u{F01EE} 1 \u{F05D1} 2 \u{F256} 3"
        );
        assert_eq!(b(0, 0, 0).after("vega board", &nerd), "vega board");
        assert_eq!(
            b(4, 0, 1).after("vega board", &nerd),
            "vega board \u{F01EE} 4 \u{F256} 1"
        );
        // The renderer adds the space after every symbol, emoji too.
        assert_eq!(b(0, 0, 0).uac_label(&emoji), "\u{2622}\u{FE0F} UAC");
        assert_eq!(
            b(1, 2, 3).uac_label(&emoji),
            "\u{2622}\u{FE0F} UAC \u{1F514} 1 \u{1F5F3}\u{FE0F} 2 \u{270B} 3"
        );
    }

    #[test]
    fn uac_tokens_set_non_zero_badges_and_clear_zero_ones() {
        let nerd = Symbols::of("nerd");
        let b = Badges {
            notes: 2,
            decisions: 0,
            waiting: 1,
        };
        assert_eq!(
            b.uac_tokens(&nerd),
            [
                "--token",
                "uac=\u{F0768} UAC",
                "--token",
                "notes=\u{F01EE} 2",
                "--clear-token",
                "decisions",
                "--token",
                "waiting=\u{F256} 1",
            ]
        );
        let zero = Badges::default().uac_tokens(&nerd);
        assert_eq!(
            zero,
            [
                "--token",
                "uac=\u{F0768} UAC",
                "--clear-token",
                "notes",
                "--clear-token",
                "decisions",
                "--clear-token",
                "waiting",
            ]
        );
        let own = crate::theme::parse("[symbols.icons]\nnotes = \"N\"\n");
        assert!(Badges {
            notes: 7,
            ..Default::default()
        }
        .uac_tokens(&own)
        .contains(&"notes=N 7".to_string()));
    }

    #[test]
    fn decisions_are_open_children_of_the_one_tracker() {
        let json = r#"[
          {"id": "v-1", "status": "open", "issue_type": "epic", "labels": ["uac-tracker"]},
          {"id": "v-1.1", "status": "open", "issue_type": "decision", "parent": "v-1", "labels": ["slayer-decision"]},
          {"id": "v-1.2", "status": "in_progress", "issue_type": "decision", "parent": "v-1", "labels": ["slayer-decision", "x"]},
          {"id": "v-1.3", "status": "closed", "issue_type": "decision", "parent": "v-1", "labels": ["slayer-decision"]},
          {"id": "v-1.4", "status": "open", "issue_type": "bug", "parent": "v-1", "labels": ["uac-tracker"]},
          {"id": "v-2.1", "status": "open", "issue_type": "decision", "parent": "v-2", "labels": ["slayer-decision"]}
        ]"#;
        assert_eq!(decision_count(json), Some(2));
        let d = decisions(json).unwrap();
        assert_eq!(d[0].id, "v-1.1");
        assert_eq!(d[0].created, 0, "no created_at");
        let dated = r#"[
          {"id": "v-1", "status": "open", "issue_type": "epic", "labels": ["uac-tracker"]},
          {"id": "v-1.1", "title": "tabs?", "status": "open", "parent": "v-1",
           "labels": ["slayer-decision"], "created_at": "2026-10-08T21:43:22Z"}
        ]"#;
        assert_eq!(
            decisions(dated).unwrap(),
            vec![Decision {
                id: "v-1.1".into(),
                title: "tabs?".into(),
                created: 1_791_495_802,
            }]
        );
        let none = r#"[{"id": "v-2.1", "status": "open", "parent": "v-2", "labels": ["slayer-decision"]}]"#;
        assert_eq!(decision_count(none), None);
        let two = r#"[
          {"id": "a", "status": "open", "issue_type": "epic", "labels": ["uac-tracker"]},
          {"id": "b", "status": "open", "issue_type": "epic", "labels": ["uac-tracker"]}
        ]"#;
        assert_eq!(decision_count(two), None);
        assert_eq!(decision_count("[]"), None);
        assert_eq!(decision_count("oops"), None);
    }

    #[test]
    fn saved_counts_go_stale() {
        let text = r#"{"notes": 1, "decisions": 2, "waiting": 0, "at": 1000}"#;
        let (b, w) = parse(text, 1000 + STALE_SECS).unwrap();
        assert_eq!((b.notes, b.decisions, b.waiting), (1, 2, 0));
        assert!(w.is_empty(), "a file from before waiting_panes");
        assert_eq!(parse(text, 1001 + STALE_SECS), None);
    }

    #[test]
    fn waiting_panes_round_trip() {
        let w = Waiting {
            pane: "w3V:p2".into(),
            name: "flist".into(),
            attention: "asking".into(),
            title: "asks: tabs?".into(),
        };
        let body = json!({"notes": 0, "decisions": 0, "waiting": 1, "at": 5,
            "waiting_panes": [{"pane": w.pane, "name": w.name, "attention": w.attention, "title": w.title}, {"name": "no pane"}]});
        let (b, got) = parse(&body.to_string(), 5).unwrap();
        assert_eq!(b.waiting, 1);
        assert_eq!(got, vec![w]);
    }
}
