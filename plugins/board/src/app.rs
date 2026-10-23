//! Central application state and all board logic (bd-backed). Rendering lives
//! in `ui`/`views`; this module owns state, navigation, and mutations.

use crate::bd;
use crate::bd::events::Record;
use crate::bd::types::Bead;
use crate::decisions::{self, Preset, Tracker};
use crate::form::CreateForm;
use crate::input::{Input, InputKind};
use crate::model::{status_rank, Mode, Scope, SortKey, View, STATUS_ORDER};
use crate::writer::Writer;

/// The status line while a reload runs in the background.
pub const LOADING: &str = "loading...";
/// Detail rows per wheel notch.
const DETAIL_STEP: i32 = 3;
use ratatui::layout::Rect;
use std::collections::{HashMap, HashSet, VecDeque};

/// A clickable id in the detail view.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Link {
    /// Select that bead.
    Bead(String),
    /// Focus that herdr pane, as written (`w3V:p1`, `io:w1:p2`).
    Pane(String),
}

/// A clickable element of the tab row (docs/design/board-ui.md, Mouse).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bar {
    /// A badge: the Inbox filtered to its kind; again: all kinds.
    Badge(crate::inbox::Kind),
    /// The `lane|all` chip: toggles the preset like `D`.
    Preset,
    /// The `repo|global` chip: toggles the scope like `S`.
    Scope,
    /// The `shown/total` chip of a bead tab: the filter prompt like `/`.
    Count,
    /// The Inbox's kind chip and count: back to all kinds.
    Kind,
}

/// What the scroll wheel scrolls under the pointer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Area {
    /// The list or table body: the wheel moves the selection.
    List,
    /// A kanban column, by index into `App::columns`.
    Column(usize),
    /// The detail text (side pane or modal).
    Detail,
    /// The Inbox rows.
    Inbox,
}

/// Rects captured during render for mouse hit-testing. Later entries are
/// drawn on top, so lookups take the last hit.
#[derive(Default)]
pub struct Hits {
    pub tabs: Vec<(View, Rect)>,
    pub rows: Vec<(String, Rect)>,
    pub links: Vec<(Link, Rect)>,
    pub bar: Vec<(Bar, Rect)>,
    pub areas: Vec<(Area, Rect)>,
}

impl Hits {
    pub fn clear(&mut self) {
        *self = Hits::default();
    }
}

pub struct App {
    #[allow(dead_code)]
    pub mode: Mode,
    pub scope: Scope,
    pub view: View,
    pub beads: Vec<Bead>,
    pub selected: Option<String>,
    pub collapsed: HashSet<String>,
    pub show_closed: bool,
    /// What the bead tabs are filtered to (see `decisions`); `D` toggles.
    pub preset: Preset,
    /// The tracker epic among the loaded beads, refreshed when they change.
    pub tracker: Tracker,
    pub sort: SortKey,
    pub filter: String,
    pub move_mode: bool,
    pub show_detail: bool,
    pub detail_modal: bool,
    pub detail_cache: HashMap<String, Bead>,
    pub input: Option<Input>,
    pub create_form: Option<CreateForm>,
    pub show_help: bool,
    /// When true, digits 1-9 set the selected bead's status (see `pick_status`).
    pub status_pick: bool,
    /// A `g` was pressed and we are waiting for the second key of `gg`.
    pub g_pending: bool,
    /// Rows of the last rendered body, so page motions match what you see.
    pub viewport_rows: u16,
    pub status_msg: String,
    /// bd's events journal is being followed, so the board updates by itself.
    pub live: bool,
    /// Records applied recently, replayed on top of a background reload.
    pub recent: VecDeque<Record>,
    /// The board needs a full reload, done in the background.
    pub reload_wanted: bool,
    /// A bead whose `bd show` the detail pane is waiting for.
    pub detail_wanted: Option<String>,
    pub detail_loading: HashSet<String>,
    /// The notes the Inbox lists (`crate::notes`).
    pub notes: crate::notes::Notes,
    /// The notes should load again once the Inbox shows.
    pub notes_wanted: bool,
    /// vegad's unread count when the notes last loaded.
    pub notes_unread: Option<usize>,
    /// The selected Inbox row (`inbox::Item::key`).
    pub inbox_sel: Option<String>,
    /// The Inbox shows only this kind (a badge click).
    pub inbox_kind: Option<crate::inbox::Kind>,
    /// The bead tab the Inbox was opened from, for Enter on a decision.
    pub last_bead_view: View,
    /// vegad on this host: heads the Inbox, `V` starts it.
    pub vegad: crate::vegad::Watch,
    /// vegad's badge counts for this repo's project (top bar).
    pub badges: crate::badges::Watch,
    /// Runs bd writes in the background.
    pub writer: Writer,
    pub should_quit: bool,
    pub hits: Hits,
    /// Rows the detail text is scrolled by, for the bead in `detail_scroll_id`.
    pub detail_scroll: usize,
    pub detail_scroll_id: Option<String>,
    /// A pane jump running in the background (see `crate::jump`).
    pub pane_jump: Option<std::sync::mpsc::Receiver<crate::jump::Outcome>>,
}

impl App {
    pub fn new(mode: Mode, scope: Scope) -> Self {
        let mut app = App {
            mode,
            scope,
            view: View::Inbox,
            beads: Vec::new(),
            selected: None,
            collapsed: HashSet::new(),
            show_closed: false,
            preset: Preset::All,
            tracker: Tracker::None,
            sort: SortKey::Status,
            filter: String::new(),
            move_mode: false,
            show_detail: mode == Mode::Popup,
            detail_modal: false,
            detail_cache: HashMap::new(),
            input: None,
            create_form: None,
            show_help: false,
            status_pick: false,
            g_pending: false,
            viewport_rows: 20,
            status_msg: String::new(),
            live: false,
            recent: VecDeque::new(),
            reload_wanted: false,
            detail_wanted: None,
            detail_loading: HashSet::new(),
            notes: crate::notes::Notes::default(),
            notes_wanted: true,
            notes_unread: None,
            inbox_sel: None,
            inbox_kind: None,
            last_bead_view: View::List,
            vegad: crate::vegad::Watch::default(),
            badges: crate::badges::Watch::default(),
            writer: Writer::start(),
            should_quit: false,
            hits: Hits::default(),
            detail_scroll: 0,
            detail_scroll_id: None,
            pane_jump: None,
        };
        app.reload();
        app
    }

    /// The status line for a failed load.
    pub(crate) fn load_error(scope: Scope, error: &str) -> String {
        let first = error.lines().next().unwrap_or("");
        if scope == Scope::Global && (first.contains("shared-server") || first.contains("--global"))
        {
            "global needs a shared-server bd DB (not configured) - S = repo".into()
        } else {
            format!("bd: {}", first.chars().take(90).collect::<String>())
        }
    }

    // ---------------------------------------------------------- data

    /// Reload the board in the background (see `App::apply_snapshot`).
    pub fn reload(&mut self) {
        self.reload_wanted = true;
        self.status_msg = LOADING.into();
        self.detail_cache.clear();
        self.vegad.refresh();
        self.notes_wanted = true;
    }

    fn passes_filter(&self, b: &Bead) -> bool {
        self.filter.is_empty() || b.haystack().contains(&self.filter.to_lowercase())
    }

    fn is_visible(&self, b: &Bead) -> bool {
        if self.preset == Preset::Lane {
            return self.passes_filter(b) && decisions::is_lane(b);
        }
        self.passes_filter(b) && (self.show_closed || !b.is_closed())
    }

    /// The `shown/total` chip: beads the tab shows, beads loaded.
    pub fn shown_total(&self) -> (usize, usize) {
        let shown = self.beads.iter().filter(|b| self.is_visible(b)).count();
        (shown, self.beads.len())
    }

    /// Find the tracker again after the beads changed.
    pub(crate) fn update_tracker(&mut self) {
        self.tracker = Tracker::find(&self.beads);
    }

    /// Toggle lane and all beads.
    pub fn cycle_preset(&mut self) {
        self.set_preset(self.preset.next());
    }

    pub fn set_preset(&mut self, p: Preset) {
        self.preset = p;
        self.status_msg = match self.preset {
            Preset::All => "all beads".into(),
            Preset::Lane => format!(
                "lane: {}",
                decisions::lane_summary(&self.beads, decisions::unix_now())
            ),
        };
        self.ensure_selected();
        self.refresh_detail();
    }

    /// Board statuses in column order: known board statuses (minus closed unless
    /// shown) plus any custom statuses present among visible beads.
    pub fn board_statuses(&self) -> Vec<String> {
        let mut v: Vec<String> = STATUS_ORDER
            .iter()
            .filter(|s| self.show_closed || **s != "closed")
            .map(|s| s.to_string())
            .collect();
        for b in &self.beads {
            if self.is_visible(b) && !v.iter().any(|s| s == &b.status) {
                v.push(b.status.clone());
            }
        }
        v
    }

    /// Visible bead ids in a status, sorted by priority then most-recent;
    /// lane beads oldest first, like the backlog line of `vega uac status`.
    pub fn ids_in(&self, status: &str) -> Vec<String> {
        let mut v: Vec<&Bead> = self
            .beads
            .iter()
            .filter(|b| b.status == status && self.is_visible(b))
            .collect();
        if self.preset == Preset::Lane {
            v.sort_by_key(|b| {
                let t = b.created_at.as_deref().and_then(decisions::parse_rfc3339);
                (t.is_none(), t)
            });
            return v.into_iter().map(|b| b.id.clone()).collect();
        }
        v.sort_by(|a, b| {
            a.priority
                .cmp(&b.priority)
                .then_with(|| b.updated_at.cmp(&a.updated_at))
        });
        v.into_iter().map(|b| b.id.clone()).collect()
    }

    /// Kanban columns: (status, ids), empty columns included.
    pub fn columns(&self) -> Vec<(String, Vec<String>)> {
        self.board_statuses()
            .into_iter()
            .map(|s| {
                let ids = self.ids_in(&s);
                (s, ids)
            })
            .collect()
    }

    /// Linear order for list/table navigation and selection resolution.
    pub fn flat_order(&self) -> Vec<String> {
        match self.view {
            View::Table => {
                let mut v: Vec<&Bead> = self.beads.iter().filter(|b| self.is_visible(b)).collect();
                match self.sort {
                    SortKey::Status => v.sort_by(|a, b| {
                        status_rank(&a.status)
                            .cmp(&status_rank(&b.status))
                            .then_with(|| a.priority.cmp(&b.priority))
                    }),
                    SortKey::Priority => v.sort_by(|a, b| {
                        a.priority
                            .cmp(&b.priority)
                            .then_with(|| b.updated_at.cmp(&a.updated_at))
                    }),
                    SortKey::Changed => v.sort_by(|a, b| b.updated_at.cmp(&a.updated_at)),
                }
                v.into_iter().map(|b| b.id.clone()).collect()
            }
            _ => {
                // List/Kanban: grouped by status, collapsed groups omit members.
                let mut out = Vec::new();
                for s in self.board_statuses() {
                    if self.collapsed.contains(&s) {
                        continue;
                    }
                    out.extend(self.ids_in(&s));
                }
                out
            }
        }
    }

    // ---------------------------------------------------------- selection

    /// Where the selection sits in the current order.
    pub(crate) fn selected_pos(&self) -> Option<usize> {
        let id = self.selected.as_ref()?;
        self.flat_order().iter().position(|o| o == id)
    }

    /// Keep the selection after the beads changed. When its bead left the
    /// view (closed, deleted, filtered), take the bead now at its place
    /// instead of jumping back to the top.
    pub(crate) fn reselect_near(&mut self, pos: Option<usize>) {
        let order = self.flat_order();
        if self.selected.as_ref().is_some_and(|id| order.contains(id)) {
            return;
        }
        let last = order.len().saturating_sub(1);
        self.selected = pos
            .and_then(|p| order.get(p.min(last)))
            .or_else(|| order.first())
            .cloned();
    }

    pub(crate) fn ensure_selected(&mut self) {
        let order = self.flat_order();
        let ok = self
            .selected
            .as_ref()
            .map(|id| order.iter().any(|o| o == id))
            .unwrap_or(false);
        if !ok {
            self.selected = order.first().cloned();
        }
    }

    pub fn selected_bead(&self) -> Option<&Bead> {
        let id = self.selected.as_ref()?;
        self.beads.iter().find(|b| &b.id == id)
    }

    /// The bead to show in the detail pane, enriched by `bd show` when cached.
    pub fn detail_bead(&self) -> Option<Bead> {
        let id = self.selected.as_ref()?;
        let board = self.selected_bead();
        let mut b = self
            .detail_cache
            .get(id)
            .cloned()
            .or_else(|| board.cloned())?;
        // `bd show` has no blocked state; the board's copy does.
        if let Some(board) = board {
            b.is_blocked = board.is_blocked;
            b.blocked_by = board.blocked_by.clone();
        }
        Some(b)
    }

    /// Ask for the selected bead's `bd show` when the detail pane needs it;
    /// it loads in the background and lands in `detail_cache`.
    pub(crate) fn refresh_detail(&mut self) {
        if !(self.show_detail || self.detail_modal) {
            return;
        }
        if let Some(id) = self.selected.clone() {
            if !self.detail_cache.contains_key(&id) && self.detail_loading.insert(id.clone()) {
                self.detail_wanted = Some(id);
            }
        }
    }

    fn locate_kanban(&self, id: &str) -> Option<(usize, usize)> {
        for (ci, (_s, ids)) in self.columns().iter().enumerate() {
            if let Some(ri) = ids.iter().position(|x| x == id) {
                return Some((ci, ri));
            }
        }
        None
    }

    // ---------------------------------------------------------- navigation

    pub fn nav_vert(&mut self, delta: i32) {
        if self.view == View::Inbox {
            self.inbox_nav(delta);
            return;
        }
        if self.view == View::Kanban {
            let cols = self.columns();
            let (ci, ri) = self
                .selected
                .as_ref()
                .and_then(|id| self.locate_kanban(id))
                .unwrap_or((0, 0));
            if let Some((_s, ids)) = cols.get(ci) {
                if !ids.is_empty() {
                    let ni = (ri as i32 + delta).clamp(0, ids.len() as i32 - 1) as usize;
                    self.selected = Some(ids[ni].clone());
                }
            }
        } else {
            let order = self.flat_order();
            if order.is_empty() {
                return;
            }
            let cur = self
                .selected
                .as_ref()
                .and_then(|id| order.iter().position(|o| o == id))
                .unwrap_or(0);
            let ni = (cur as i32 + delta).clamp(0, order.len() as i32 - 1) as usize;
            self.selected = Some(order[ni].clone());
        }
        self.refresh_detail();
    }

    pub fn nav_horiz(&mut self, dir: i32) {
        match self.view {
            View::Kanban => {
                let cols = self.columns();
                if cols.is_empty() {
                    return;
                }
                let (ci, ri) = self
                    .selected
                    .as_ref()
                    .and_then(|id| self.locate_kanban(id))
                    .unwrap_or((0, 0));
                let nci = (ci as i32 + dir).clamp(0, cols.len() as i32 - 1) as usize;
                let (_s, ids) = &cols[nci];
                if !ids.is_empty() {
                    let nri = ri.min(ids.len() - 1);
                    self.selected = Some(ids[nri].clone());
                }
                self.refresh_detail();
            }
            View::List => {
                // Collapse (h) / expand (l) the selected bead's group.
                if let Some(b) = self.selected_bead() {
                    let s = b.status.clone();
                    if dir < 0 {
                        self.collapsed.insert(s);
                        self.ensure_selected();
                    } else {
                        self.collapsed.remove(&s);
                    }
                }
            }
            View::Table | View::Inbox => {}
        }
    }

    pub fn jump_group(&mut self, n: usize) {
        let statuses = self.board_statuses();
        if let Some(s) = statuses.get(n) {
            if let Some(first) = self.ids_in(s).first() {
                self.selected = Some(first.clone());
                self.refresh_detail();
            }
        }
    }

    /// Jump to the first bead of the previous/next non-empty status group.
    pub fn jump_group_rel(&mut self, dir: i32) {
        let statuses = self.board_statuses();
        if statuses.is_empty() {
            return;
        }
        let cur = self.selected_bead().map(|b| b.status.clone());
        let cur_idx = cur
            .and_then(|s| statuses.iter().position(|x| *x == s))
            .unwrap_or(0) as i32;
        let n = statuses.len() as i32;
        for step in 1..=n {
            let i = (cur_idx + dir * step).rem_euclid(n) as usize;
            if let Some(first) = self.ids_in(&statuses[i]).first() {
                self.selected = Some(first.clone());
                self.refresh_detail();
                return;
            }
        }
    }

    // ---------------------------------------------------------- mutations

    /// Run a bd write in the background, showing `pending` until it is done
    /// (see `finish_writes`).
    fn write(
        &mut self,
        pending: String,
        job: impl FnOnce(Scope) -> anyhow::Result<String> + Send + 'static,
    ) {
        self.status_msg = pending;
        let scope = self.scope;
        self.writer.submit(move || job(scope), None);
    }

    /// Take finished writes. With live updates on, the journal brings the
    /// change; otherwise the board reloads in the background.
    pub fn finish_writes(&mut self) {
        self.finish_pane_jump();
        self.notes.poll();
        self.badges.poll();
        // New notes while the Inbox shows: load them (which marks them read).
        let unread = self.badges.now.map(|b| b.notes);
        if unread != self.notes_unread {
            self.notes_unread = unread;
            self.notes_wanted = true;
        }
        if self.notes_wanted && self.view == View::Inbox {
            self.notes_wanted = false;
            self.notes.reload();
        }
        if self.vegad.poll() {
            if let Some(said) = self.vegad.started.take() {
                self.status_msg = said;
            }
        }
        for done in self.writer.poll() {
            match done.result {
                Ok(msg) => {
                    self.status_msg = msg;
                    // The write may have added a comment the detail lists.
                    if let Some(id) = self.selected.clone() {
                        self.detail_cache.remove(&id);
                        self.detail_loading.remove(&id);
                        self.refresh_detail();
                    }
                    if self.scope == Scope::Global || !self.live {
                        self.reload_wanted = true;
                    }
                }
                Err(e) => {
                    self.status_msg = format!("error: {e}");
                    if done.form.is_some() {
                        self.create_form = done.form;
                    }
                }
            }
        }
    }

    pub fn claim_selected(&mut self) {
        let Some(id) = self.selected.clone() else {
            return;
        };
        self.write("claiming...".into(), move |scope| {
            bd::claim(scope, &id).map(|_| "claimed".into())
        });
    }

    /// `V`: start vegad unless it runs (`vega post relay start`).
    pub fn start_vegad(&mut self) {
        self.status_msg = "starting vegad (vega post relay start)...".into();
        self.vegad.start();
    }

    /// Toggle this pane between docked and fullscreen via herdr's native zoom.
    /// herdr owns the zoom state; we run it on the pane we live in (HERDR_PANE_ID),
    /// falling back to the focused pane when the env var is absent.
    pub fn toggle_zoom(&mut self) {
        let bin = std::env::var("HERDR_BIN_PATH").unwrap_or_else(|_| "herdr".to_string());
        let pane = std::env::var("HERDR_PANE_ID").unwrap_or_default();
        let mut cmd = std::process::Command::new(&bin);
        cmd.args(["pane", "zoom", "--toggle"]);
        if pane.is_empty() {
            cmd.arg("--current");
        } else {
            cmd.args(["--pane", &pane]);
        }
        // Silent on success (no status message); only surface real failures.
        match cmd.output() {
            Ok(o) if o.status.success() => {}
            Ok(o) => {
                let err = String::from_utf8_lossy(&o.stderr);
                self.status_msg =
                    format!("zoom: {}", err.trim().chars().take(80).collect::<String>());
            }
            Err(e) => self.status_msg = format!("zoom: {e}"),
        }
    }

    pub fn retag(&mut self, dir: i32) {
        let Some(id) = self.selected.clone() else {
            return;
        };
        let Some(cur) = self.selected_bead().map(|b| b.status.clone()) else {
            return;
        };
        let statuses = self.board_statuses();
        let Some(idx) = statuses.iter().position(|s| *s == cur) else {
            return;
        };
        let ni = (idx as i32 + dir).clamp(0, statuses.len() as i32 - 1) as usize;
        if ni == idx {
            return;
        }
        let target = statuses[ni].clone();
        self.write(format!("→ {target}..."), move |scope| {
            bd::set_status(scope, &id, &target).map(|_| format!("→ {target}"))
        });
    }

    // ---------------------------------------------------------- input

    pub fn open_create_form(&mut self) {
        let epics: Vec<(String, String)> = self
            .beads
            .iter()
            .filter(|b| b.issue_type == "epic" && !b.is_closed())
            .map(|b| (b.id.clone(), b.title.clone()))
            .collect();
        self.create_form = Some(CreateForm::new(epics));
    }

    /// Reopen the create form pre-filled from the selected bead to edit it.
    /// Saving sends only the fields that changed (see `bd::update_bead`).
    pub fn open_edit_form(&mut self) {
        let Some(b) = self.selected_bead().cloned() else {
            return;
        };
        let epics: Vec<(String, String)> = self
            .beads
            .iter()
            .filter(|e| e.issue_type == "epic" && !e.is_closed() && e.id != b.id)
            .map(|e| (e.id.clone(), e.title.clone()))
            .collect();
        let mut f = CreateForm::new(epics);
        f.title = b.title.clone();
        f.description = b.description.clone();
        f.assignee = b.assigned.clone().unwrap_or_default();
        f.labels = b.labels.join(",");
        f.epic_idx = b
            .parent
            .as_ref()
            .and_then(|p| f.epics.iter().position(|(id, _)| id == p))
            .map_or(0, |i| i + 1);
        f.type_idx = crate::form::TYPES
            .iter()
            .position(|t| *t == b.issue_type)
            .unwrap_or(0);
        f.priority = b.priority.min(4);
        f.deferred = b.status == "deferred";
        f.edit_id = Some(b.id.clone());
        f.before = Some(f.new_bead());
        self.create_form = Some(f);
    }

    /// Set the selected bead's status to the n-th board status (status picker).
    pub fn pick_status(&mut self, n: usize) {
        self.status_pick = false;
        let statuses = self.board_statuses();
        let Some(status) = statuses.get(n).cloned() else {
            return;
        };
        let Some(id) = self.selected.clone() else {
            return;
        };
        self.write(format!("-> {status}..."), move |scope| {
            bd::set_status(scope, &id, &status).map(|_| format!("-> {status}"))
        });
    }

    /// Focus the selected bead's status group: collapse every other group, or
    /// expand all again when already focused. A no-op-friendly solo toggle.
    pub fn focus_status(&mut self) {
        let Some(cur) = self.selected_bead().map(|b| b.status.clone()) else {
            return;
        };
        let all = self.board_statuses();
        let others_collapsed = all
            .iter()
            .filter(|s| **s != cur)
            .all(|s| self.collapsed.contains(s));
        if others_collapsed {
            self.collapsed.clear();
            self.status_msg = "showing all".into();
        } else {
            self.collapsed = all.into_iter().filter(|s| s != &cur).collect();
            self.status_msg = format!("focus {cur}");
        }
        self.ensure_selected();
    }

    pub fn clear_filter(&mut self) {
        self.filter.clear();
        self.ensure_selected();
    }

    pub fn submit_create_form(&mut self) {
        let Some(f) = self.create_form.take() else {
            return;
        };
        if f.title.trim().is_empty() {
            self.status_msg = "title required".into();
            self.create_form = Some(f);
            return;
        }
        let scope = self.scope;
        let nb = f.new_bead();
        let before = f.before.clone();
        let pending = match &f.edit_id {
            Some(id) => format!("updating {id}..."),
            None => "creating...".to_string(),
        };
        let edit_id = f.edit_id.clone();
        self.status_msg = pending;
        self.writer.submit(
            move || match (edit_id, before) {
                (Some(id), Some(before)) => {
                    bd::update_bead(scope, &id, &nb, &before).map(|saved| {
                        if saved {
                            format!("updated {id}")
                        } else {
                            "no changes".to_string()
                        }
                    })
                }
                _ => bd::create(scope, &nb).map(|_| format!("created {}", nb.issue_type)),
            },
            Some(f),
        );
    }

    pub fn open_input(&mut self, kind: InputKind) {
        let title = match &kind {
            InputKind::Filter => "Filter".into(),
            InputKind::CloseReason(_) => "Close reason (required)".into(),
            InputKind::Note(_) => "Note".into(),
            InputKind::Priority(_) => "Priority 0-4".into(),
            InputKind::Comment(id) => format!("Answer {id} (vega uac answer; Enter sends)"),
            InputKind::Dismiss(id) => format!("Dismiss {id}: reason (required; closes it)"),
        };
        let buffer = if let InputKind::Filter = kind {
            self.filter.clone()
        } else {
            String::new()
        };
        self.input = Some(Input {
            kind,
            title,
            buffer,
        });
    }

    pub fn submit_input(&mut self) {
        let Some(inp) = self.input.take() else { return };
        let buf = inp.buffer.trim().to_string();
        match inp.kind {
            InputKind::Filter => {
                self.filter = buf;
                self.ensure_selected();
            }
            InputKind::CloseReason(id) => {
                if buf.is_empty() {
                    // bd rule: reason must not be blank. Re-open the prompt.
                    self.status_msg = "close needs a reason".into();
                    self.open_input(InputKind::CloseReason(id));
                    return;
                }
                // The bead leaves the board, so its detail popup goes too.
                if self.selected.as_deref() == Some(id.as_str()) {
                    self.detail_modal = false;
                }
                self.write("closing...".into(), move |scope| {
                    bd::close(scope, &id, &buf).map(|_| "closed".into())
                });
            }
            InputKind::Note(id) => {
                if !buf.is_empty() {
                    self.write("noting...".into(), move |scope| {
                        bd::add_note(scope, &id, &buf).map(|_| "noted".into())
                    });
                }
            }
            InputKind::Priority(id) => match buf.parse::<u8>() {
                Ok(p) if p <= 4 => {
                    self.write(format!("priority {p}..."), move |scope| {
                        bd::set_priority(scope, &id, p).map(|_| format!("priority {p}"))
                    });
                }
                _ => self.status_msg = "priority must be 0-4".into(),
            },
            InputKind::Comment(id) => {
                if !buf.is_empty() {
                    self.write("answering...".into(), move |_scope| {
                        crate::uac::answer(&id, &buf)
                    });
                }
            }
            InputKind::Dismiss(id) => {
                if buf.is_empty() {
                    self.status_msg = "dismiss needs a reason".into();
                    self.open_input(InputKind::Dismiss(id));
                    return;
                }
                self.write("dismissing...".into(), move |scope| {
                    bd::close(scope, &id, &bd::dismiss_reason(&buf))
                        .map(|_| format!("dismissed {id}"))
                });
            }
        }
    }

    // ---------------------------------------------------------- scope/view

    pub fn toggle_scope(&mut self) {
        self.scope = self.scope.toggled();
        // The other scope's beads must not show under this one's label.
        self.beads.clear();
        self.update_tracker();
        self.detail_loading.clear();
        self.reload();
    }

    pub fn toggle_closed(&mut self) {
        self.show_closed = !self.show_closed;
        self.reload();
    }

    pub fn set_view(&mut self, v: View) {
        if v == View::Inbox && self.view != View::Inbox {
            self.last_bead_view = self.view;
            self.notes_wanted = true;
            self.vegad.refresh();
        }
        self.view = v;
        self.ensure_selected();
        self.refresh_detail();
    }

    pub fn select(&mut self, id: String) {
        self.selected = Some(id);
        self.refresh_detail();
    }

    /// A clicked bead id: select it, showing all beads when the current
    /// preset hides it and expanding its status group.
    pub fn jump_to(&mut self, id: String) {
        let status = self
            .beads
            .iter()
            .find(|b| b.id == id)
            .map(|b| b.status.clone());
        match status {
            Some(status) => {
                if !self.flat_order().contains(&id) && self.preset != Preset::All {
                    self.preset = Preset::All;
                    self.status_msg = format!("all beads, to show {id}");
                }
                self.collapsed.remove(&status);
                if !self.flat_order().contains(&id) {
                    self.status_msg = format!("{id} is {status}, not on the board");
                }
            }
            None => self.status_msg = format!("{id} is not on the board (closed or another scope)"),
        }
        self.select(id);
    }

    /// Scroll the wheel by `delta` notches over `area`.
    pub fn scroll(&mut self, area: Area, delta: i32) {
        match area {
            Area::List => self.nav_vert(delta),
            Area::Column(ci) => self.scroll_column(ci, delta),
            Area::Detail => {
                self.detail_scroll =
                    (self.detail_scroll as i32 + delta * DETAIL_STEP).max(0) as usize;
            }
            Area::Inbox => self.inbox_nav(delta),
        }
    }

    /// The wheel over a kanban column moves the selection in that column;
    /// over another column than the selection's, it starts at its top card.
    pub fn scroll_column(&mut self, ci: usize, delta: i32) {
        let cols = self.columns();
        let Some((_, ids)) = cols.get(ci) else { return };
        if ids.is_empty() {
            return;
        }
        let here = self
            .selected
            .as_ref()
            .and_then(|id| ids.iter().position(|x| x == id));
        let ni = match here {
            Some(r) => (r as i32 + delta).clamp(0, ids.len() as i32 - 1) as usize,
            None => 0,
        };
        self.select(ids[ni].clone());
    }

    /// A click in the tab row. A badge toggles its filter: a second click
    /// shows all kinds again.
    pub fn click_bar(&mut self, b: Bar) {
        match b {
            Bar::Badge(k) if self.view == View::Inbox && self.inbox_kind == Some(k) => {
                self.inbox_kind = None
            }
            Bar::Badge(k) => self.open_inbox(Some(k)),
            Bar::Preset => self.cycle_preset(),
            Bar::Scope => self.toggle_scope(),
            Bar::Count => self.open_input(InputKind::Filter),
            Bar::Kind => self.inbox_kind = None,
        }
    }

    /// Focus a clicked pane id in the background (herdr calls can take
    /// seconds); `finish_writes` reports the outcome.
    pub fn jump_pane(&mut self, text: String) {
        self.status_msg = format!("jumping to {text}...");
        self.pane_jump = Some(crate::jump::start(text));
    }

    /// Take a finished pane jump. A local focus closes the popup board so
    /// the pane shows.
    pub(crate) fn finish_pane_jump(&mut self) {
        let Some(rx) = &self.pane_jump else { return };
        let Ok(out) = rx.try_recv() else { return };
        self.pane_jump = None;
        match out {
            Ok((msg, local)) => {
                self.status_msg = msg;
                if local && self.mode == Mode::Popup {
                    self.should_quit = true;
                }
            }
            Err(e) => self.status_msg = e,
        }
    }

    pub fn open_detail(&mut self) {
        self.detail_modal = true;
        self.refresh_detail();
    }

    /// Half a screen, the way vim's Ctrl-d moves. Never zero, or the key
    /// would look broken on a very short pane.
    pub fn half_page(&self) -> i32 {
        ((self.viewport_rows / 2) as i32).max(1)
    }

    pub fn page(&self) -> i32 {
        (self.viewport_rows as i32).max(1)
    }

    /// Jump to the first or last column of the board. Kanban only; the other
    /// views have a single column, where it would be a no-op.
    pub fn nav_edge_column(&mut self, last: bool) {
        if self.view != View::Kanban {
            return;
        }
        let cols = self.columns().len() as i32;
        self.nav_horiz(if last { cols } else { -cols });
    }

    /// Copy the selected bead id to the system clipboard, so it can be pasted
    /// into a commit message or a `bd` command.
    pub fn yank_id(&mut self) {
        let Some(id) = self.selected.clone() else {
            self.status_msg = "nothing selected".into();
            return;
        };
        let candidates: [(&str, &[&str]); 3] = [
            ("pbcopy", &[]),
            ("wl-copy", &[]),
            ("xclip", &["-selection", "clipboard"]),
        ];
        for (bin, args) in candidates {
            let mut child = match std::process::Command::new(bin)
                .args(args)
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
            {
                Ok(c) => c,
                Err(_) => continue,
            };
            if let Some(mut stdin) = child.stdin.take() {
                use std::io::Write;
                let _ = stdin.write_all(id.as_bytes());
            }
            let _ = child.wait();
            self.status_msg = format!("yanked {id}");
            return;
        }
        self.status_msg = "no clipboard tool (pbcopy, wl-copy, xclip)".into();
    }

    pub fn toggle_detail(&mut self) {
        if self.view == View::Inbox {
            return;
        }
        if self.view == View::Kanban {
            self.detail_modal = !self.detail_modal;
        } else {
            self.show_detail = !self.show_detail;
        }
        self.refresh_detail();
    }

    /// The opt-in marker the tab.created hook looks for. herdr hands every
    /// plugin command its own config directory, and that is the only place a
    /// plugin may keep user-editable settings.
    fn auto_dock_marker() -> Option<std::path::PathBuf> {
        let dir = std::env::var("HERDR_PLUGIN_CONFIG_DIR").ok()?;
        if dir.is_empty() {
            return None;
        }
        Some(std::path::Path::new(&dir).join("auto-dock"))
    }

    pub fn auto_dock_enabled(&self) -> bool {
        Self::auto_dock_marker().is_some_and(|p| p.exists())
    }

    /// Turn auto-dock on or off. The hook reads the marker on each tab.created,
    /// so the change applies to the next tab with no reload.
    pub fn toggle_auto_dock(&mut self) {
        let Some(path) = Self::auto_dock_marker() else {
            self.status_msg = "auto-dock needs herdr (no plugin config dir)".into();
            return;
        };
        if path.exists() {
            match std::fs::remove_file(&path) {
                Ok(()) => self.status_msg = "auto-dock off".into(),
                Err(e) => self.status_msg = format!("auto-dock: {e}"),
            }
            return;
        }
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        match std::fs::write(&path, "") {
            Ok(()) => self.status_msg = "auto-dock on: new tabs open the dock".into(),
            Err(e) => self.status_msg = format!("auto-dock: {e}"),
        }
    }
}

#[cfg(test)]
mod auto_dock_tests {
    use super::*;
    use crate::model::{Mode, Scope};

    /// One test, not two: HERDR_PLUGIN_CONFIG_DIR is process-global, so
    /// parallel tests that set and clear it race each other.
    ///
    /// The marker is the contract between this toggle and the tab.created
    /// hook, so the round trip has to leave the directory as it found it.
    #[test]
    fn toggles_the_marker_and_reports_when_unconfigured() {
        let dir = std::env::temp_dir().join(format!("hb-auto-dock-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("HERDR_PLUGIN_CONFIG_DIR", &dir);

        let mut app = App::new(Mode::Dock, Scope::Repo);
        assert!(!app.auto_dock_enabled(), "starts off");

        app.toggle_auto_dock();
        assert!(app.auto_dock_enabled(), "on after the first press");
        assert!(dir.join("auto-dock").exists());

        app.toggle_auto_dock();
        assert!(!app.auto_dock_enabled(), "off after the second");
        assert!(!dir.join("auto-dock").exists());

        // Outside herdr there is no config dir, so the toggle has to say so
        // instead of silently doing nothing.
        std::env::remove_var("HERDR_PLUGIN_CONFIG_DIR");
        app.toggle_auto_dock();
        assert!(!app.auto_dock_enabled());
        assert!(app.status_msg.contains("needs herdr"));

        std::fs::remove_dir_all(&dir).ok();
    }
}
