//! How an ask is shown to the Slayer (docs/design/uber-uac.md §5): the
//! one AskUserQuestion question for it, and `shown_sha256` over that.
//!
//! Both hosts render the same: `vega uber answer` on the Uber host compares
//! the captured AskUserQuestion input with `render` and sends `sha256`; the
//! UAC-host vegad renders its own copy of the ask and compares the hash
//! (§4). So nothing here may depend on the clock or the host: the deadline
//! is printed in its own offset, and `overdue` is not part of it.

use serde::{Deserialize, Serialize};
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

use crate::{Ask, Level};

/// The suffix of the recommended option's label.
pub const RECOMMENDED: &str = " (Recommended)";

/// AskUserQuestion's limit on `header`.
pub const HEADER_MAX: usize = 12;

/// One AskUserQuestion option as shown.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShownOption {
    pub label: String,
    pub description: String,
}

/// One AskUserQuestion question as shown; the field order is the hash's.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Shown {
    pub header: String,
    pub question: String,
    pub options: Vec<ShownOption>,
}

/// `github.com/durandom/vega` → `vega`, at most 12 characters.
pub fn header(project: &str) -> String {
    let short = project
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or(project);
    short.chars().take(HEADER_MAX).collect()
}

/// ` · due 10-26 14:00` in the deadline's own offset; empty for `no-rush`.
fn due(deadline: &str) -> String {
    match OffsetDateTime::parse(deadline, &Rfc3339) {
        Ok(t) => format!(
            " · due {:02}-{:02} {:02}:{:02}",
            u8::from(t.month()),
            t.day(),
            t.hour(),
            t.minute()
        ),
        Err(_) => String::new(),
    }
}

/// The ask as one AskUserQuestion question under `tag` (`#a7`):
/// `#a7 <question> · L4 · due 10-26 14:00`, the context on a second line,
/// the recommended option first with ` (Recommended)` and `why` after its
/// detail, then the others in the ask's order.
pub fn render(ask: &Ask, tag: &str) -> Shown {
    let level = match ask.level {
        Level::L3 => "L3",
        Level::L4 => "L4",
    };
    let mut question = format!("{tag} {} · {level}{}", ask.question, due(&ask.deadline));
    if !ask.context.is_empty() {
        question.push('\n');
        question.push_str(&ask.context);
    }
    let mut options = Vec::with_capacity(ask.options.len());
    if let Some(r) = ask.options.get(ask.recommend) {
        let description = match (r.detail.is_empty(), ask.why.is_empty()) {
            (_, true) => r.detail.clone(),
            (true, false) => ask.why.clone(),
            (false, false) => format!("{} — {}", r.detail, ask.why),
        };
        options.push(ShownOption {
            label: format!("{}{RECOMMENDED}", r.label),
            description,
        });
    }
    for (i, o) in ask.options.iter().enumerate() {
        if i != ask.recommend {
            options.push(ShownOption {
                label: o.label.clone(),
                description: o.detail.clone(),
            });
        }
    }
    Shown {
        header: header(&ask.project),
        question,
        options,
    }
}

impl Shown {
    /// The ask's option label for a label as shown (the recommended one
    /// carries the suffix); `None` for anything else, such as "Other" text.
    pub fn pick(&self, ask: &Ask, shown_label: &str) -> Option<String> {
        let i = self.options.iter().position(|o| o.label == shown_label)?;
        let label = if i == 0 {
            shown_label.strip_suffix(RECOMMENDED)?
        } else {
            shown_label
        };
        ask.options
            .iter()
            .find(|o| o.label == label)
            .map(|o| o.label.clone())
    }
}

/// `shown_sha256`: sha256 of the rendering's JSON, 64 lowercase hex digits.
pub fn sha256(ask: &Ask, tag: &str) -> String {
    let json = serde_json::to_vec(&render(ask, tag)).unwrap_or_default();
    hex256(&json)
}

/// sha256 of `data`, 64 lowercase hex digits.
pub fn hex256(data: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(data)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Opt;

    fn ask() -> Ask {
        Ask {
            id: "vega-lhc.209~AbCdEfGhIjKlMnOpQrStUv".into(),
            project: "github.com/durandom/vega".into(),
            bead: "vega-lhc.209".into(),
            level: Level::L4,
            urgent: None,
            question: "Push main?".into(),
            context: "Gate green.".into(),
            options: vec![
                Opt {
                    label: "no".into(),
                    detail: "wait".into(),
                },
                Opt {
                    label: "yes".into(),
                    detail: "push".into(),
                },
            ],
            recommend: 1,
            why: "Tested.".into(),
            deadline: "2026-10-26T14:00:00+01:00".into(),
        }
    }

    #[test]
    fn renders_the_section_5_mapping() {
        let s = render(&ask(), "#a7");
        assert_eq!(s.header, "vega");
        assert_eq!(
            s.question,
            "#a7 Push main? · L4 · due 10-26 14:00\nGate green."
        );
        let labels: Vec<_> = s.options.iter().map(|o| o.label.as_str()).collect();
        assert_eq!(labels, ["yes (Recommended)", "no"]);
        assert_eq!(s.options[0].description, "push — Tested.");
        assert_eq!(s.options[1].description, "wait");
    }

    #[test]
    fn no_rush_empty_context_and_long_project() {
        let mut a = ask();
        a.deadline = "no-rush".into();
        a.context.clear();
        a.why.clear();
        a.level = Level::L3;
        a.project = "github.com/acme/a-very-long-name".into();
        let s = render(&a, "#b2");
        assert_eq!(s.question, "#b2 Push main? · L3");
        assert_eq!(s.header, "a-very-long-");
        assert_eq!(s.options[0].description, "push");
    }

    #[test]
    fn pick_maps_shown_labels_back() {
        let a = ask();
        let s = render(&a, "#a7");
        assert_eq!(s.pick(&a, "yes (Recommended)").as_deref(), Some("yes"));
        assert_eq!(s.pick(&a, "no").as_deref(), Some("no"));
        assert_eq!(s.pick(&a, "yes"), None);
        assert_eq!(s.pick(&a, "Pizza please"), None);
    }

    #[test]
    fn hash_is_stable_and_depends_on_what_is_shown() {
        let a = ask();
        let h = sha256(&a, "#a7");
        assert_eq!(h.len(), 64);
        assert_eq!(h, sha256(&a.clone(), "#a7"));
        assert_ne!(h, sha256(&a, "#a8"));
        let mut b = a.clone();
        b.options[0].detail = "wait a bit".into();
        assert_ne!(h, sha256(&b, "#a7"));
        // The id is not shown: a resend under a new id hashes the same.
        b = a.clone();
        b.id = "vega-lhc.209~ZZCdEfGhIjKlMnOpQrStUv".into();
        assert_eq!(h, sha256(&b, "#a7"));
    }
}
