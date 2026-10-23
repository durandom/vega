//! The top bar's badges (unread notes, open decisions, panes waiting on the
//! Slayer): the counts vegad saved for this repo's project, the same ones
//! the UAC pane's label shows (vega_herdr::badges; docs/design/vegad.md),
//! and the waiting panes behind the count, which the Inbox lists.

use std::time::{Duration, Instant};
use vega_herdr::badges::{self, Badges, Waiting};

/// How often the board re-reads vegad's counts (vegad writes every 15 s and
/// at once after a note changed).
const EVERY: Duration = Duration::from_secs(2);

#[derive(Default)]
pub struct Watch {
    /// This repo's project key; looked up on the first poll.
    key: Option<Option<String>>,
    pub now: Option<Badges>,
    /// The panes behind the waiting count, for the Inbox.
    pub waiting: Vec<Waiting>,
    next: Option<Instant>,
}

impl Watch {
    /// Re-reads the counts when due; true if they changed.
    pub fn poll(&mut self) -> bool {
        if self.next.is_some_and(|t| Instant::now() < t) {
            return false;
        }
        self.next = Some(Instant::now() + EVERY);
        let key = self.key.get_or_insert_with(project_key);
        let (now, waiting) = key
            .as_deref()
            .and_then(|k| badges::load(k, crate::decisions::unix_now()))
            .map_or((None, Vec::new()), |(b, w)| (Some(b), w));
        let changed = now != self.now || waiting != self.waiting;
        self.now = now;
        self.waiting = waiting;
        changed
    }
}

/// Use the shared offline resolver; the board only inspects its local cwd.
fn project_key() -> Option<String> {
    match vega_herdr::lanes::project_key(&std::env::current_dir().ok()?) {
        Ok(id) => Some(id),
        Err(e) => {
            eprintln!("board project: {}: {}; {}", e.code, e.message, e.hint);
            None
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn decision_beads_wear_the_decisions_badge_symbol() {
        use vega_herdr::theme::{Symbols, DECISIONS, DECISION_BEAD};
        for set in vega_herdr::theme::SETS {
            let s = Symbols::of(set);
            assert_eq!(s.get(&DECISION_BEAD), s.get(&DECISIONS), "{set}");
        }
    }

    /// Badges wear the label's colours: the default, or theme.toml's.
    #[test]
    fn badges_wear_the_symbols_colors() {
        use crate::ui::widgets::badge_style;
        use ratatui::style::Color;
        use vega_herdr::theme::{parse, Symbols, NOTES, WAITING};
        let d = Symbols::default();
        let own = parse("[symbols.colors]\nnotes = \"#010203\"\n");
        assert_eq!(badge_style(&own, &NOTES).fg, Some(Color::Rgb(1, 2, 3)));
        let c = d.color(&WAITING).unwrap();
        assert_eq!(
            badge_style(&own, &WAITING).fg,
            Some(Color::Rgb(c.0, c.1, c.2))
        );
    }

    /// The layout measures symbols: a Nerd Font glyph is one cell, a wide
    /// emoji two, an override anything.
    #[test]
    fn layout_measures_symbol_widths() {
        use crate::ui::theme::cells;
        use vega_herdr::theme::{Symbols, PINNED};
        assert_eq!(cells(&Symbols::of("nerd").icon(&PINNED)), 2);
        assert_eq!(cells(&Symbols::of("emoji").icon(&PINNED)), 3);
        assert_eq!(cells("[x] "), 4);
        // Every emoji, U+FE0F sequences included, is two cells; a Nerd Font
        // glyph one.
        let (nerd, emoji) = (Symbols::of("nerd"), Symbols::of("emoji"));
        for t in vega_herdr::theme::TOPICS {
            assert_eq!(cells(nerd.get(t)), 1, "nerd {}", t.name);
            assert_eq!(
                cells(emoji.get(t)),
                2,
                "emoji {} {:?}",
                t.name,
                emoji.get(t)
            );
        }
    }
}
