//! The host's last human input in herdr, for the Uber-UAC's engaged window
//! (docs/design/uber-uac.md §2, vega-lhc.209.12). herdr 0.9.5 reports no
//! keystrokes; the cheapest signal is `pane.focused` (any client, no
//! source field). vega's own focus calls are human commands (`vega mux
//! next`, `vega board`, `vega open --focus`) and `vega mux` spawns with
//! `--no-focus`. A focus right after a pane closed may be herdr moving
//! the focus on, so it does not count.
//!
//! vegad writes the time to `uber/human.json`; the beat carries it as
//! `human_at` (`vega_registry::beat`).

use std::time::{Duration, Instant};

use vega_registry::uber_engaged;

/// A focus this soon after a pane closed is herdr's, not the Slayer's.
const AFTER_GONE: Duration = Duration::from_secs(3);

#[derive(Default)]
pub struct Human {
    gone_at: Option<Instant>,
}

impl Human {
    pub fn gone(&mut self, now: Instant) {
        self.gone_at = Some(now);
    }

    /// Whether a focus at `now` counts as human input.
    pub fn counts(&self, now: Instant) -> bool {
        self.gone_at
            .is_none_or(|t| now.saturating_duration_since(t) > AFTER_GONE)
    }

    /// A pane got the focus: records it when it counts.
    pub fn focused(&self) {
        if !self.counts(Instant::now()) {
            return;
        }
        let Some(file) = uber_engaged::human_file() else {
            return;
        };
        let now = (crate::now_ms() / 1000) as i64;
        if let Err(e) = uber_engaged::write_at(&file, now) {
            crate::log(&format!("human: cannot save {}: {e}", file.display()));
        }
    }
}

/// On the Uber host (a local claim file): the newest `human_at` of any
/// host, for the capture hook's notice (`uber_engaged::SEEN`).
pub fn seen(host: &str, view: &vega_registry::View) {
    let Some(claim) = vega_registry::uber_state::file() else {
        return;
    };
    if vega_registry::uber_state::local(&claim, host).is_none() {
        return;
    }
    if let Some((h, at)) = view.human_at() {
        let file = claim.with_file_name(uber_engaged::SEEN);
        if let Err(e) = uber_engaged::write_seen(&file, h, at) {
            crate::log(&format!("human: cannot save {}: {e}", file.display()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_focus_right_after_a_pane_closed_does_not_count() {
        let mut h = Human::default();
        let t = Instant::now();
        assert!(h.counts(t));
        h.gone(t);
        assert!(!h.counts(t + Duration::from_secs(1)));
        assert!(h.counts(t + Duration::from_secs(4)));
    }
}
