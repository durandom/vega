//! GitHub CI watch (docs/design/ci-watch.md): one poller per host in vegad
//! instead of a `gh run watch` loop per agent.
//!
//! `vega ci watch` writes a watch file per (target, pane) and returns; vegad
//! runs [`poll::cycle`] every few seconds: it polls each watched target with
//! conditional requests (a 304 costs no rate limit), caches the state for
//! `vega ci`, and returns the mails to send on a new verdict.

pub mod checks;
pub mod forge;
pub mod gh;
pub mod poll;
pub mod store;
pub mod target;
pub mod tea;

pub use checks::{Bucket, Check, Summary, Verdict};
pub use forge::Forge;
pub use target::Target;

pub fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}
