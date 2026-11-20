//! Whether an owner pane still exists (design, decision 4). `None` means
//! this host cannot tell; callers then keep the session (fail safe: cleanup
//! never kills what it cannot attribute to a dead owner).

use std::time::Duration;

use vega_herdr::{hc_local, herdr_on};

/// The pane id of a record on this host. hc ids (`t7`) resolve against the
/// hc daemon, herdr ids (`w1:p2`) against the herdr socket.
pub fn alive(pane: &str) -> Option<bool> {
    let pane = pane
        .strip_prefix("herdr:")
        .or_else(|| pane.strip_prefix("hc:"))
        .unwrap_or(pane);
    if !vega_shim::is_pane(pane) || pane.contains('/') || pane.matches(':').count() > 1 {
        return None;
    }
    if vega_herdr::pane::is_hc_id(pane) {
        match hc_local(&["get", pane]) {
            // The terminal still exists; it may sit at an idle, kept shell.
            Ok(_) => Some(true),
            Err(f) if f.code == "pane_not_found" => Some(false),
            Err(_) => None,
        }
    } else {
        match herdr_on(None, &["pane", "list"], Duration::from_secs(5)) {
            Ok(list) => {
                let panes = list["result"]["panes"].as_array()?;
                if panes.iter().any(|p| !p["pane_id"].is_string()) {
                    return None;
                }
                Some(panes.iter().any(|p| p["pane_id"].as_str() == Some(pane)))
            }
            Err(_) => None,
        }
    }
}
