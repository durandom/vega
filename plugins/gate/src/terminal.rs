//! Gate terminal identity; stale context is an error, not absence.
use vega_herdr::pane::Backend;
use vega_herdr::{own_terminal, Terminal};

pub fn own() -> Result<Option<(Backend, String)>, String> {
    if !["HERDR_PANE_ID", "HC_TERMINAL_ID"]
        .iter()
        .any(|k| std::env::var(k).is_ok_and(|s| !s.is_empty()))
    {
        return Ok(None);
    }
    own_terminal()
        .map(|t| {
            Some(match t {
                Terminal::Herdr(p) => (Backend::Herdr, p),
                Terminal::Hc(p) => (Backend::Hc, p),
            })
        })
        .map_err(|e| e.message)
}

pub fn get(pane: &str) -> Result<serde_json::Value, String> {
    vega_herdr::hc_on(
        None,
        &["get", pane],
        Some(std::time::Duration::from_secs(5)),
    )
    .map_err(|e| e.message)
}
