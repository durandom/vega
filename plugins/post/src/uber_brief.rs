//! The Uber host's side of the brief in vegad (docs/design/uber-uac.md §3,
//! vega-lhc.209.8, `vega_uber_msg::brief`):
//! - a reply on a brief's thread from one of its UACs is stored, so `vega
//!   uber brief --show` renders it; the reply still reaches the inbox;
//! - the digest wake: while the Slayer talks to the Uber, one mail to the
//!   Uber pane when the oldest waiting ask is 4 h old, at most once per 4 h.

use std::path::PathBuf;

use vega_uber_msg::brief;

use crate::{log, ToolMail};

/// `$XDG_STATE_HOME/vega/uber`, the claim file's directory.
fn uber_dir() -> Option<PathBuf> {
    Some(vega_registry::uber_state::file()?.parent()?.to_path_buf())
}

/// Mail on `thread` from `from` on `from_host` to this host's pane `to`.
pub fn reply(thread: &str, to: &str, from_host: &str, from: &str, body: &[u8]) {
    let Some(dir) = uber_dir() else { return };
    match brief::store_reply(&dir, thread, to, from_host, from, body) {
        Ok(true) => log(&format!(
            "uber: brief {thread}: reply of {from} on {from_host} stored"
        )),
        Ok(false) => {}
        Err(e) => log(&format!(
            "uber: brief {thread}: reply of {from} not stored: {e}"
        )),
    }
}

/// The digest mail for the live Uber on `host`, if one is due now.
pub fn digest(view: Option<&vega_registry::View>, host: &str, now: i64) -> Option<ToolMail> {
    let view = view?;
    let uber = view.live_uber()?;
    if vega_registry::token(&uber.host) != vega_registry::token(host) {
        return None;
    }
    let dir = uber_dir()?;
    let waiting: Vec<i64> = brief::open_decisions(&dir).iter().map(|d| d.at).collect();
    let engaged = view.uber_engaged(&uber, now).on();
    let text = brief::digest_due(&waiting, brief::last_digest(&dir), engaged, now)?;
    if let Err(e) = brief::save_digest(&dir, now) {
        log(&format!("uber: digest not recorded, skipped: {e}"));
        return None;
    }
    log(&format!(
        "uber: digest to {}: {} asks waiting",
        uber.pane,
        waiting.len()
    ));
    Some((brief::DIGEST_AGENT.to_string(), uber.pane, text))
}
