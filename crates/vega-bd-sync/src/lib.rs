//! Beads sync (docs/design/beads-sync.md): every provisioned project keeps
//! its own local Dolt server (bd server mode) and replicates to one database
//! on its configured Dolt remotesapi; vegad pulls on an interval and pushes
//! after local writes. Provisioning reads `VEGA_BD_REMOTE` or
//! `~/.config/vega/bd.remote`; environment wins, with no compiled endpoint.
//! Existing projects retain their recorded remote.
//!
//! `vega bd init` provisions a project and registers it here; vegad runs
//! [`sync::cycle`] every few seconds; `vega bd status` and `vega uac status`
//! read what vegad recorded.
//!
//! [`git`]: vegad also fetches the same projects' git repos, fast-forwards
//! their clean main checkout and records how far worktrees are behind
//! (docs/design/git-sync.md).

pub mod adopt;
pub mod bd;
pub mod cutover;
pub mod doctor;
pub mod git;
pub mod store;
pub mod sync;

pub use store::{Project, State};

/// Configured Dolt remotesapi; unset means no remote, never an estate default.
pub fn remote_base() -> String {
    vega_herdr::service_config::get("VEGA_BD_REMOTE", "bd.remote")
        .unwrap_or_default()
        .trim_end_matches('/')
        .to_string()
}

/// SQL endpoint used only to create missing databases.
pub fn sql_endpoint() -> String {
    vega_herdr::service_config::get("VEGA_BD_SQL", "bd.sql").unwrap_or_default()
}

/// The SQL user that holds only the global CREATE grant.
pub const SQL_USER: &str = "vega";

pub fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// `12s ago`, `4m ago`, `3h ago`, `2d ago`.
pub fn ago(secs: i64) -> String {
    match secs.max(0) {
        s if s < 60 => format!("{s}s ago"),
        s if s < 3600 => format!("{}m ago", s / 60),
        s if s < 86400 => format!("{}h ago", s / 3600),
        s => format!("{}d ago", s / 86400),
    }
}
