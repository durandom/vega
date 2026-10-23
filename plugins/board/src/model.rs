//! View-independent state: the views (three bead views and the Inbox), the scope (repo vs global), the
//! launch mode (docked vs floating popup), the table sort key, and the bd
//! status vocabulary that drives kanban columns and list groups.

/// The status columns/groups, in board order. Mirrors bd 1.1.0 built-ins.
/// Unknown/custom statuses are appended after these at render time.
pub const STATUS_ORDER: &[&str] = &[
    "open",
    "in_progress",
    "blocked",
    "hooked",
    "deferred",
    "pinned",
    "closed",
];

/// The theme's symbol for a status, one space after it.
pub fn status_glyph(status: &str) -> String {
    use vega_herdr::theme as t;
    crate::ui::theme::icon(match status {
        "open" => &t::OPEN,
        "in_progress" => &t::IN_PROGRESS,
        "blocked" => &t::BLOCKED,
        "deferred" => &t::DEFERRED,
        "closed" => &t::CLOSED,
        "pinned" => &t::PINNED,
        "hooked" => &t::HOOKED,
        _ => &t::OTHER_STATUS,
    })
}

pub fn status_label(status: &str) -> String {
    match status {
        "open" => "Open".into(),
        "in_progress" => "In Progress".into(),
        "blocked" => "Blocked".into(),
        "deferred" => "Deferred".into(),
        "closed" => "Closed".into(),
        "pinned" => "Pinned".into(),
        "hooked" => "Hooked".into(),
        other => {
            // Title-case a custom status for display.
            let mut c = other.chars();
            match c.next() {
                Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
                None => other.into(),
            }
        }
    }
}

/// Rank a status for ordering; known statuses first (board order), unknowns last.
pub fn status_rank(status: &str) -> usize {
    STATUS_ORDER
        .iter()
        .position(|s| *s == status)
        .unwrap_or(STATUS_ORDER.len())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    List,
    Table,
    Kanban,
    /// What needs the Slayer: decisions, waiting panes, notes (`crate::inbox`).
    Inbox,
}

impl View {
    pub const ALL: [View; 4] = [View::List, View::Table, View::Kanban, View::Inbox];

    pub fn title(self) -> &'static str {
        match self {
            View::List => "List",
            View::Table => "Table",
            View::Kanban => "Kanban",
            View::Inbox => "Inbox",
        }
    }

    pub fn next(self) -> View {
        match self {
            View::List => View::Table,
            View::Table => View::Kanban,
            View::Kanban => View::Inbox,
            View::Inbox => View::List,
        }
    }

    pub fn prev(self) -> View {
        match self {
            View::List => View::Inbox,
            View::Table => View::List,
            View::Kanban => View::Table,
            View::Inbox => View::Kanban,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    Repo,
    Global,
}

impl Scope {
    pub fn label(self) -> &'static str {
        match self {
            Scope::Repo => "repo",
            Scope::Global => "global",
        }
    }

    pub fn toggled(self) -> Scope {
        match self {
            Scope::Repo => Scope::Global,
            Scope::Global => Scope::Repo,
        }
    }
}

/// How the board was launched: a narrow docked side panel, or a wide floating
/// popup. Affects layout density only: both start on the Inbox.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Dock,
    Popup,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortKey {
    Status,
    Priority,
    Changed,
}

impl SortKey {
    pub fn label(self) -> &'static str {
        match self {
            SortKey::Status => "status",
            SortKey::Priority => "priority",
            SortKey::Changed => "changed",
        }
    }

    pub fn next(self) -> SortKey {
        match self {
            SortKey::Status => SortKey::Priority,
            SortKey::Priority => SortKey::Changed,
            SortKey::Changed => SortKey::Status,
        }
    }
}
