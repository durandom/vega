//! Key and mouse dispatch. Keeps the keymap in one place; App holds the logic.

use crate::app::{App, Area, Bar, Hits, Link};
use crate::input::InputKind;
use crate::model::View;
use ratatui::crossterm::event::{
    KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::Rect;

/// The `?` help; `vega-board --skill` and docs/design/board-ui.md list the
/// same keys.
pub fn help_lines() -> Vec<(&'static str, &'static str)> {
    vec![
        ("K / Tab", "cycle tab (List / Table / Kanban / Inbox)"),
        ("Shift+Tab", "cycle tab backwards"),
        (
            "N",
            "Inbox: decisions, waiting panes, notes (again: back to the bead tab)",
        ),
        (
            "Inbox",
            "Enter jump (decision: its detail · pane: focus) · m answer · x dismiss a decision (closes it with your reason) · c clear a note (and its host's older ones) · Esc all kinds",
        ),
        ("j k ↑ ↓", "move selection"),
        ("gg / G", "jump to the first or last card (Home / End too)"),
        ("Ctrl+d / Ctrl+u", "half page down / up"),
        ("Ctrl+f / Ctrl+b", "page down / up"),
        ("0 / $", "kanban: first / last column"),
        ("y", "yank the selected bead id to the clipboard"),
        ("h l ← →", "kanban: change column · list: collapse/expand"),
        ("1-9", "jump to status group N"),
        ("[ ] { }", "jump to prev / next status group"),
        ("Enter", "open detail"),
        ("d", "toggle detail pane / modal"),
        ("v", "move mode: then h/l retags status"),
        ("c", "claim (in_progress + assign you)"),
        ("x", "close (prompts for reason)"),
        ("p", "set priority (0-4)"),
        ("n", "add note (bd note)"),
        ("m", "answer: vega uac answer (comment + mail to the UAC)"),
        ("D", "toggle lane beads / all beads"),
        ("V", "start vegad on this host unless it runs (vega post relay start)"),
        (
            "a",
            "new bead form (Tab: field · ←→: type/priority · Enter: create)",
        ),
        ("e", "edit selected bead (reopens the form)"),
        ("s", "set status (then pick 1-9)"),
        ("F", "focus: show only this status group"),
        ("o", "table: cycle sort (status/priority/changed)"),
        ("/", "filter  (Esc clears it)"),
        ("S", "toggle scope repo ⇄ global"),
        ("C", "toggle showing closed"),
        ("A", "auto-open the dock in new tabs (toggle)"),
        ("r", "refresh beads, notes and vegad's state"),
        ("f", "zoom pane fullscreen (toggle)"),
        ("? ", "this help"),
        ("q", "quit"),
        ("Esc", "back out a layer (never quits)"),
        (
            "mouse",
            "click a tab, a row, a badge (Inbox on that kind), a chip (lane|all, repo|global, shown/total) or an id in the detail · wheel scrolls what is under it",
        ),
    ]
}

fn hit(r: &Rect, col: u16, row: u16) -> bool {
    col >= r.x
        && col < r.x.saturating_add(r.width)
        && row >= r.y
        && row < r.y.saturating_add(r.height)
}

/// What a mouse event does, from the rects of the last render.
#[derive(Debug, PartialEq, Eq)]
pub enum Action {
    Tab(View),
    Row(String),
    Link(Link),
    Bar(Bar),
    Scroll(Area, i32),
}

/// The last rect in `v` under the pointer: later ones were drawn on top.
fn top<T: Clone>(v: &[(T, Rect)], col: u16, row: u16) -> Option<T> {
    v.iter()
        .rev()
        .find(|(_, r)| hit(r, col, row))
        .map(|(t, _)| t.clone())
}

/// Single left click and the wheel only: no double-click, no drag.
pub fn action(hits: &Hits, ev: &MouseEvent) -> Option<Action> {
    let (c, r) = (ev.column, ev.row);
    match ev.kind {
        MouseEventKind::Down(MouseButton::Left) => {
            // An overlay or the detail text is drawn on top of the rows: a
            // click there must not select the row underneath.
            if let Some(l) = top(&hits.links, c, r) {
                return Some(Action::Link(l));
            }
            if matches!(top(&hits.areas, c, r), Some(Area::Detail)) {
                return None;
            }
            top(&hits.tabs, c, r)
                .map(Action::Tab)
                .or_else(|| top(&hits.bar, c, r).map(Action::Bar))
                .or_else(|| top(&hits.rows, c, r).map(Action::Row))
        }
        MouseEventKind::ScrollDown => top(&hits.areas, c, r).map(|a| Action::Scroll(a, 1)),
        MouseEventKind::ScrollUp => top(&hits.areas, c, r).map(|a| Action::Scroll(a, -1)),
        _ => None,
    }
}

pub fn handle_mouse(app: &mut App, ev: MouseEvent) {
    match action(&app.hits, &ev) {
        Some(Action::Tab(v)) => app.set_view(v),
        Some(Action::Row(key)) if app.view == View::Inbox => app.inbox_sel = Some(key),
        Some(Action::Row(id)) => app.select(id),
        Some(Action::Link(Link::Bead(id))) => app.jump_to(id),
        Some(Action::Link(Link::Pane(p))) => app.jump_pane(p),
        Some(Action::Bar(b)) => app.click_bar(b),
        Some(Action::Scroll(a, d)) => app.scroll(a, d),
        None => {}
    }
}

/// Pasted text goes into an open prompt as one line; a pasted line break
/// must not submit half an answer. Outside a prompt, a paste is ignored.
pub fn handle_paste(app: &mut App, text: &str) {
    if let Some(inp) = app.input.as_mut() {
        for c in text.chars() {
            inp.push(if c == '\n' || c == '\r' { ' ' } else { c });
        }
    }
}

pub fn handle_key(app: &mut App, key: KeyEvent) {
    // 1) Text prompt swallows everything.
    if let Some(inp) = app.input.as_mut() {
        match key.code {
            KeyCode::Enter => app.submit_input(),
            KeyCode::Esc => {
                app.input = None;
            }
            KeyCode::Backspace => inp.backspace(),
            KeyCode::Char(c) => inp.push(c),
            _ => {}
        }
        return;
    }

    // 1b) Create form.
    if let Some(f) = app.create_form.as_mut() {
        match key.code {
            KeyCode::Esc => app.create_form = None,
            KeyCode::Enter => app.submit_create_form(),
            KeyCode::Tab | KeyCode::Down => f.next_field(),
            KeyCode::BackTab | KeyCode::Up => f.prev_field(),
            KeyCode::Left => f.left(),
            KeyCode::Right => f.right(),
            KeyCode::Backspace => f.backspace(),
            KeyCode::Char(c) => f.input_char(c),
            _ => {}
        }
        return;
    }

    // 2) Help overlay: any key closes it.
    if app.show_help {
        app.show_help = false;
        return;
    }

    // 2b) Status picker: a digit sets the selected bead's status; anything else cancels.
    if app.status_pick {
        match key.code {
            KeyCode::Char(c @ '1'..='9') => app.pick_status((c as u8 - b'1') as usize),
            _ => app.status_pick = false,
        }
        return;
    }

    // Ctrl chords first. Without this they fall through to the plain letter,
    // which is how Ctrl+C used to claim a bead instead of doing nothing.
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        app.g_pending = false;
        match key.code {
            KeyCode::Char('d') => app.nav_vert(app.half_page()),
            KeyCode::Char('u') => app.nav_vert(-app.half_page()),
            KeyCode::Char('f') => app.nav_vert(app.page()),
            KeyCode::Char('b') => app.nav_vert(-app.page()),
            KeyCode::Char('c') => app.should_quit = true,
            _ => {}
        }
        return;
    }

    // `gg`: a pending `g` takes the next key. Anything but `g` cancels.
    if app.g_pending {
        app.g_pending = false;
        if key.code == KeyCode::Char('g') {
            app.nav_vert(i32::MIN / 2);
        }
        return;
    }

    if app.view == View::Inbox {
        handle_inbox_key(app, key);
        return;
    }

    let sel = app.selected.clone();

    match key.code {
        KeyCode::Char('q') => app.should_quit = true,
        KeyCode::Esc => {
            // Esc never quits (use q) - it only backs out of a layer.
            if !app.filter.is_empty() {
                app.clear_filter();
            } else if app.move_mode {
                app.move_mode = false;
            } else if app.detail_modal {
                app.detail_modal = false;
            } else if app.show_detail {
                app.show_detail = false;
            }
        }
        KeyCode::Char('A') => app.toggle_auto_dock(),
        KeyCode::Char('D') => app.cycle_preset(),
        KeyCode::Char('N') => app.open_inbox(None),
        KeyCode::Char('V') => app.start_vegad(),
        KeyCode::Char('?') => app.show_help = true,

        KeyCode::Char('K') | KeyCode::Tab => {
            let v = app.view.next();
            app.set_view(v);
        }
        KeyCode::BackTab => {
            let v = app.view.prev();
            app.set_view(v);
        }

        KeyCode::Char('j') | KeyCode::Down => app.nav_vert(1),
        KeyCode::Char('k') | KeyCode::Up => app.nav_vert(-1),
        KeyCode::Char('h') | KeyCode::Left => {
            if app.move_mode {
                app.retag(-1);
            } else {
                app.nav_horiz(-1);
            }
        }
        KeyCode::Char('l') | KeyCode::Right => {
            if app.move_mode {
                app.retag(1);
            } else {
                app.nav_horiz(1);
            }
        }
        KeyCode::Home => app.nav_vert(i32::MIN / 2),
        KeyCode::Char('G') | KeyCode::End => app.nav_vert(i32::MAX / 2),

        KeyCode::Char(c @ '1'..='9') => {
            let n = (c as u8 - b'1') as usize;
            app.jump_group(n);
        }
        KeyCode::Char('[') | KeyCode::Char('{') => app.jump_group_rel(-1),
        KeyCode::Char(']') | KeyCode::Char('}') => app.jump_group_rel(1),
        KeyCode::Char('0') => app.nav_edge_column(false),
        KeyCode::Char('$') => app.nav_edge_column(true),
        KeyCode::Char('y') => app.yank_id(),

        KeyCode::Enter => app.open_detail(),
        KeyCode::Char('d') => app.toggle_detail(),

        KeyCode::Char('v') => app.move_mode = !app.move_mode,
        KeyCode::Char('c') => app.claim_selected(),
        KeyCode::Char('x') => {
            if let Some(id) = sel {
                app.open_input(InputKind::CloseReason(id));
            }
        }
        KeyCode::Char('n') => {
            if let Some(id) = sel {
                app.open_input(InputKind::Note(id));
            }
        }
        KeyCode::Char('m') => {
            if let Some(id) = sel {
                app.open_input(InputKind::Comment(id));
            }
        }
        KeyCode::Char('p') => {
            if let Some(id) = sel {
                app.open_input(InputKind::Priority(id));
            }
        }
        KeyCode::Char('a') => app.open_create_form(),
        KeyCode::Char('e') => app.open_edit_form(),
        KeyCode::Char('s') => {
            if app.selected.is_some() {
                app.status_pick = true;
            }
        }
        KeyCode::Char('F') => app.focus_status(),
        KeyCode::Char('o') => {
            if app.view == View::Table {
                app.sort = app.sort.next();
                app.status_msg = format!("sort: {}", app.sort.label());
            }
        }

        KeyCode::Char('/') => app.open_input(InputKind::Filter),
        KeyCode::Char('g') => app.g_pending = true,
        KeyCode::Char('S') => app.toggle_scope(),
        KeyCode::Char('C') => app.toggle_closed(),
        KeyCode::Char('r') => app.reload(),
        KeyCode::Char('f') => app.toggle_zoom(),
        _ => {}
    }
}

/// The Inbox's keys: its own (Enter, m, x, c, Esc) and the ones every tab has.
/// The bead keys (claim, close, ...) do nothing here.
fn handle_inbox_key(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Enter => app.inbox_enter(),
        KeyCode::Char('m') => app.inbox_answer(),
        KeyCode::Char('c') => app.inbox_clear(),
        KeyCode::Char('x') => app.inbox_dismiss(),
        KeyCode::Esc if app.detail_modal => app.detail_modal = false,
        KeyCode::Esc => app.inbox_kind = None,
        KeyCode::Char('j') | KeyCode::Down => app.nav_vert(1),
        KeyCode::Char('k') | KeyCode::Up => app.nav_vert(-1),
        KeyCode::Home => app.nav_vert(i32::MIN / 2),
        KeyCode::Char('G') | KeyCode::End => app.nav_vert(i32::MAX / 2),
        KeyCode::Char('g') => app.g_pending = true,
        KeyCode::Char('N') => app.leave_inbox(),
        KeyCode::Char('K') | KeyCode::Tab => app.set_view(app.view.next()),
        KeyCode::BackTab => app.set_view(app.view.prev()),
        KeyCode::Char('q') => app.should_quit = true,
        KeyCode::Char('?') => app.show_help = true,
        KeyCode::Char('V') => app.start_vegad(),
        KeyCode::Char('S') => app.toggle_scope(),
        KeyCode::Char('D') => app.cycle_preset(),
        KeyCode::Char('A') => app.toggle_auto_dock(),
        KeyCode::Char('r') => app.reload(),
        KeyCode::Char('f') => app.toggle_zoom(),
        _ => {}
    }
}

#[cfg(test)]
mod key_tests {
    use super::*;
    use crate::decisions::Preset;
    use crate::inbox::Kind;
    use crate::model::{Mode, Scope};

    fn app() -> App {
        App::new(Mode::Dock, Scope::Repo)
    }

    fn press(app: &mut App, c: char) {
        handle_key(app, KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }

    fn ctrl(app: &mut App, c: char) {
        handle_key(app, KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL));
    }

    /// Modifiers used to be ignored, so Ctrl+C reached the plain 'c' arm and
    /// claimed the selected bead: a real write to bd from a key people press
    /// to escape.
    #[test]
    fn ctrl_c_quits_instead_of_claiming() {
        let mut a = app();
        ctrl(&mut a, 'c');
        assert!(a.should_quit);
    }

    #[test]
    fn plain_c_does_not_quit() {
        let mut a = app();
        press(&mut a, 'c');
        assert!(!a.should_quit);
    }

    /// Ctrl+D is a half page now, not the detail pane.
    #[test]
    fn ctrl_d_does_not_toggle_the_detail_pane() {
        let mut a = app();
        let before = a.show_detail;
        ctrl(&mut a, 'd');
        assert_eq!(a.show_detail, before);
    }

    #[test]
    fn g_waits_for_a_second_key() {
        let mut a = app();
        press(&mut a, 'g');
        assert!(a.g_pending, "armed after the first g");
        press(&mut a, 'g');
        assert!(!a.g_pending, "cleared once the pair completes");
    }

    /// A stray key after `g` cancels rather than running: `gx` must not open
    /// the close-with-reason prompt.
    #[test]
    fn g_then_another_key_cancels_quietly() {
        let mut a = app();
        press(&mut a, 'g');
        press(&mut a, 'x');
        assert!(!a.g_pending);
        assert!(a.input.is_none(), "x must not have run");
    }

    #[test]
    fn a_ctrl_chord_also_cancels_the_prefix() {
        let mut a = app();
        press(&mut a, 'g');
        ctrl(&mut a, 'u');
        assert!(!a.g_pending);
    }

    fn bead(id: &str, kind: &str, labels: &[&str], parent: Option<&str>) -> crate::bd::types::Bead {
        crate::bd::types::Bead {
            id: id.into(),
            issue_type: kind.into(),
            status: "open".into(),
            labels: labels.iter().map(|l| l.to_string()).collect(),
            parent: parent.map(String::from),
            ..Default::default()
        }
    }

    /// A board on a bead tab.
    fn list_app() -> App {
        let mut a = app();
        a.view = View::List;
        a
    }

    /// The board starts on the Inbox; the bead tabs show all beads, `D`
    /// toggles the lane beads (oldest first).
    #[test]
    fn d_toggles_lane_and_all_beads() {
        let mut a = app();
        assert_eq!(a.view, View::Inbox);
        a.view = View::List;
        let lane = |id: &str, at: &str| {
            let mut b = bead(id, "task", &["uac-lane"], None);
            b.created_at = Some(at.into());
            b
        };
        a.beads = vec![
            bead("t", "epic", &["uac-tracker"], None),
            bead("t.1", "decision", &["slayer-decision"], Some("t")),
            bead("x", "task", &[], None),
            lane("l-new", "2026-10-09T10:00:00Z"),
            lane("l-old", "2026-10-01T10:00:00Z"),
        ];
        a.update_tracker();
        assert_eq!(a.flat_order().len(), 5);
        press(&mut a, 'D');
        assert_eq!(a.flat_order(), vec!["l-old", "l-new"]);
        assert!(a.status_msg.starts_with("lane: 2 open, oldest l-old"));
        assert_eq!(a.shown_total(), (2, 5));
        press(&mut a, 'D');
        assert_eq!(a.flat_order().len(), 5);
    }

    #[test]
    fn m_opens_the_answer_prompt_and_paste_stays_one_line() {
        let mut a = list_app();
        a.selected = Some("t.1".into());
        press(&mut a, 'm');
        assert!(matches!(
            a.input.as_ref().map(|i| &i.kind),
            Some(InputKind::Comment(id)) if id == "t.1"
        ));
        handle_paste(&mut a, "yes,\noption b");
        assert_eq!(a.input.as_ref().unwrap().buffer, "yes, option b");
    }

    /// The bead keys do nothing on the Inbox: `c` claims nothing, `x`
    /// prompts for no close.
    #[test]
    fn bead_keys_do_not_reach_through_the_inbox() {
        let mut a = app();
        a.selected = Some("t.1".into());
        for c in ['c', 'x', 'n', 'p', 'a', 'e', 's'] {
            press(&mut a, c);
        }
        assert!(a.input.is_none() && a.create_form.is_none() && !a.status_pick);
        assert_ne!(a.status_msg, "claiming...");
    }

    /// `N` opens the Inbox from a bead tab and goes back from it.
    #[test]
    fn n_toggles_the_inbox() {
        let mut a = app();
        a.view = View::Kanban;
        press(&mut a, 'N');
        assert_eq!(a.view, View::Inbox);
        press(&mut a, 'N');
        assert_eq!(a.view, View::Kanban);
    }

    fn mouse(kind: MouseEventKind, column: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        }
    }

    fn click(c: u16, r: u16) -> MouseEvent {
        mouse(MouseEventKind::Down(MouseButton::Left), c, r)
    }

    /// A list body with rows a, b and a detail pane over its right half,
    /// holding a bead link and a pane link; a badge in the top bar.
    fn hits() -> Hits {
        let mut h = Hits::default();
        h.tabs.push((View::Kanban, Rect::new(10, 0, 8, 1)));
        h.bar
            .push((Bar::Badge(Kind::Decision), Rect::new(30, 0, 3, 1)));
        h.areas.push((Area::List, Rect::new(0, 1, 80, 20)));
        h.rows.push(("a".into(), Rect::new(0, 1, 80, 1)));
        h.rows.push(("b".into(), Rect::new(0, 2, 80, 1)));
        h.areas.push((Area::Detail, Rect::new(40, 1, 40, 20)));
        h.links
            .push((Link::Bead("v-1".into()), Rect::new(42, 2, 3, 1)));
        h.links
            .push((Link::Pane("w3V:p1".into()), Rect::new(50, 2, 6, 1)));
        h
    }

    #[test]
    fn clicks_hit_links_over_rows_and_the_detail_swallows_the_rest() {
        let h = hits();
        let at = |c, r| action(&h, &click(c, r));
        assert_eq!(at(43, 2), Some(Action::Link(Link::Bead("v-1".into()))));
        assert_eq!(at(55, 2), Some(Action::Link(Link::Pane("w3V:p1".into()))));
        assert_eq!(at(60, 2), None, "detail text over row b");
        assert_eq!(at(5, 2), Some(Action::Row("b".into())));
        assert_eq!(at(12, 0), Some(Action::Tab(View::Kanban)));
        assert_eq!(at(31, 0), Some(Action::Bar(Bar::Badge(Kind::Decision))));
        assert_eq!(at(25, 0), None);
    }

    #[test]
    fn only_a_single_left_click_acts() {
        let h = hits();
        for kind in [
            MouseEventKind::Down(MouseButton::Right),
            MouseEventKind::Up(MouseButton::Left),
            MouseEventKind::Drag(MouseButton::Left),
            MouseEventKind::Moved,
        ] {
            assert_eq!(action(&h, &mouse(kind, 43, 2)), None, "{kind:?}");
        }
    }

    #[test]
    fn the_wheel_scrolls_the_area_under_the_pointer() {
        let h = hits();
        let wheel = |k, c, r| action(&h, &mouse(k, c, r));
        assert_eq!(
            wheel(MouseEventKind::ScrollDown, 50, 10),
            Some(Action::Scroll(Area::Detail, 1))
        );
        assert_eq!(
            wheel(MouseEventKind::ScrollUp, 5, 10),
            Some(Action::Scroll(Area::List, -1))
        );
        assert_eq!(wheel(MouseEventKind::ScrollDown, 5, 0), None, "top bar");
    }

    fn kanban() -> App {
        let mut a = app();
        let mut done = bead("c", "task", &[], None);
        done.status = "in_progress".into();
        a.beads = vec![
            bead("a", "task", &[], None),
            bead("b", "task", &[], None),
            done,
        ];
        a.view = View::Kanban;
        a
    }

    #[test]
    fn the_wheel_over_a_kanban_column_moves_in_that_column() {
        let mut a = kanban();
        let cols = a.columns();
        let open = cols.iter().position(|(s, _)| s == "open").unwrap();
        let busy = cols.iter().position(|(s, _)| s == "in_progress").unwrap();
        a.selected = Some(cols[open].1[0].clone());
        a.scroll(Area::Column(open), 1);
        assert_eq!(a.selected.as_ref(), Some(&cols[open].1[1]));
        a.scroll(Area::Column(busy), 1);
        assert_eq!(
            a.selected.as_deref(),
            Some("c"),
            "another column: its top card"
        );
    }

    #[test]
    fn the_wheel_scrolls_the_detail_text_without_moving_the_selection() {
        let mut a = kanban();
        a.selected = Some("a".into());
        a.scroll(Area::Detail, 1);
        a.scroll(Area::Detail, 1);
        a.scroll(Area::Detail, -1);
        assert_eq!(a.detail_scroll, 3);
        a.scroll(Area::Detail, -5);
        assert_eq!(a.detail_scroll, 0);
        assert_eq!(a.selected.as_deref(), Some("a"));
    }

    /// A clicked id the lane view hides switches to all beads and opens
    /// its group.
    #[test]
    fn a_bead_link_shows_and_selects_the_bead() {
        let mut a = list_app();
        a.beads = vec![
            bead("l", "task", &["uac-lane"], None),
            bead("x", "task", &[], None),
        ];
        a.preset = Preset::Lane;
        a.collapsed.insert("open".into());
        a.jump_to("x".into());
        assert_eq!(a.preset, Preset::All);
        assert!(!a.collapsed.contains("open"));
        assert_eq!(a.selected.as_deref(), Some("x"));
        assert!(a.flat_order().contains(&"x".to_string()));
    }

    /// Every badge gets a rect, one space apart as drawn.
    #[test]
    fn badge_rects_follow_the_drawn_text() {
        use vega_herdr::theme::{DECISIONS, NOTES, WAITING};
        let parts = vec![
            (&NOTES, "N1".to_string()),
            (&WAITING, "W2".to_string()),
            (&DECISIONS, "D10".to_string()),
        ];
        assert_eq!(
            crate::ui::widgets::badge_hits(&parts, 20, 0),
            vec![
                (Bar::Badge(Kind::Note), Rect::new(20, 0, 2, 1)),
                (Bar::Badge(Kind::Waiting), Rect::new(23, 0, 2, 1)),
                (Bar::Badge(Kind::Decision), Rect::new(26, 0, 3, 1)),
            ]
        );
    }

    #[test]
    fn half_page_and_page_never_stall_on_a_tiny_pane() {
        let mut a = app();
        a.viewport_rows = 1;
        assert!(a.half_page() >= 1);
        assert!(a.page() >= 1);
    }

    /// The board drawn into a test terminal `w` columns wide: every row,
    /// one symbol per cell.
    fn draw(a: &mut App, w: u16) -> Vec<Vec<String>> {
        use ratatui::{backend::TestBackend, Terminal};
        let h = 16;
        let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
        t.draw(|f| crate::ui::render(f, a)).unwrap();
        let buf = t.backend().buffer();
        (0..h)
            .map(|y| (0..w).map(|x| buf[(x, y)].symbol().to_string()).collect())
            .collect()
    }

    /// Where `text` is drawn: (column, row).
    fn find(rows: &[Vec<String>], text: &str) -> (u16, u16) {
        let n = text.chars().count();
        for (y, row) in rows.iter().enumerate() {
            for x in 0..row.len() {
                let s: String = row[x..].iter().take(n).map(String::as_str).collect();
                if s == text {
                    return (x as u16, y as u16);
                }
            }
        }
        let screen: Vec<String> = rows.iter().map(|r| r.concat()).collect();
        panic!("{text:?} not drawn in\n{}", screen.join("\n"))
    }

    /// Draws `a` at width `w`, then clicks the first and the last cell of
    /// `text` where it is drawn; returns what the click did.
    fn click_drawn(a: &mut App, w: u16, text: &str) -> Option<Action> {
        let rows = draw(a, w);
        let (x, y) = find(&rows, text);
        let last = x + text.chars().count() as u16 - 1;
        assert_eq!(
            action(&a.hits, &click(last, y)),
            action(&a.hits, &click(x, y)),
            "{text:?} acts the same on its whole width"
        );
        let did = action(&a.hits, &click(x, y));
        handle_mouse(a, click(x, y));
        did
    }

    /// A board with a UAC tracker, two decisions, a waiting pane, a note
    /// and every badge non-zero.
    fn top_bar_app() -> App {
        let mut a = app();
        let mut d1 = bead("t.1", "decision", &["slayer-decision"], Some("t"));
        d1.title = "Merge order?".into();
        d1.created_at = Some("2026-10-12T10:00:00Z".into());
        let mut d2 = bead("t.2", "decision", &["slayer-decision"], Some("t"));
        d2.title = "Keep the alias?".into();
        d2.created_at = Some("2026-10-11T10:00:00Z".into());
        a.beads = vec![
            bead("t", "epic", &["uac-tracker"], None),
            d1,
            d2,
            bead("x", "task", &[], None),
        ];
        a.update_tracker();
        a.status_msg.clear();
        a.badges.now = Some(vega_herdr::badges::Badges {
            notes: 2,
            decisions: 5,
            waiting: 7,
        });
        a.badges.waiting = vec![vega_herdr::badges::Waiting {
            pane: "w3V:p2".into(),
            name: "flist".into(),
            attention: "asking".into(),
            title: "asks: tabs?".into(),
        }];
        a.notes.loaded = true;
        a.notes.rows = vec![crate::notes::NoteRow {
            id: 1_791_000_000_000,
            host: "deimos".into(),
            kind: "quarantine".into(),
            pane: Some("w1:p1".into()),
            bead: None,
            title: "reply from deimos".into(),
            body: String::new(),
            unread: true,
            local: false,
        }];
        a
    }

    /// Whether the badge `text` is drawn selected: a background colour.
    fn badge_selected(a: &mut App, w: u16, text: &str) -> bool {
        use ratatui::{backend::TestBackend, Terminal};
        let mut t = Terminal::new(TestBackend::new(w, 16)).unwrap();
        t.draw(|f| crate::ui::render(f, a)).unwrap();
        let buf = t.backend().buffer();
        let rows: Vec<Vec<String>> = (0..16)
            .map(|y| (0..w).map(|x| buf[(x, y)].symbol().to_string()).collect())
            .collect();
        let (x, y) = find(&rows, text);
        let bg = buf[(x, y)].bg;
        bg != ratatui::style::Color::Reset && bg == buf[(x + 1, y)].bg
    }

    /// The badge `t` with its count, as the top bar draws it.
    fn badge(t: &vega_herdr::theme::Topic, n: usize) -> String {
        format!("{}{n}", vega_herdr::theme::symbols().icon(t))
    }

    /// The chips of a bead tab react where they are drawn, narrow and wide.
    #[test]
    fn the_drawn_chips_are_clickable() {
        // At 60 columns the other tabs shrink to their letter and the chips
        // show their active half.
        let mut a = top_bar_app();
        a.view = View::List;
        let row = draw(&mut a, 60).remove(0).concat();
        assert!(row.contains(" I4 ") && row.contains(" List "), "{row}");
        assert!(row.trim_end().ends_with("all  repo  4/4"), "{row}");
        for w in [60, 80, 140] {
            let mut a = top_bar_app();
            a.view = View::List;
            // (wide, narrow) text of each chip state.
            let t = |wide: &'static str, narrow: &'static str| if w < 80 { narrow } else { wide };
            assert_eq!(
                click_drawn(&mut a, w, t("lane|all", "all")),
                Some(Action::Bar(Bar::Preset))
            );
            assert_eq!(a.preset, Preset::Lane, "{w}");
            click_drawn(&mut a, w, t("lane|all", "lane"));
            assert_eq!(a.preset, Preset::All);

            assert_eq!(
                click_drawn(&mut a, w, t("repo|global", "repo")),
                Some(Action::Bar(Bar::Scope))
            );
            assert_eq!(a.scope, Scope::Global);
            click_drawn(&mut a, w, t("repo|global", "global"));
            assert_eq!(a.scope, Scope::Repo);

            // toggle_scope cleared the beads: 0 of 0.
            assert_eq!(click_drawn(&mut a, w, "0/0"), Some(Action::Bar(Bar::Count)));
            assert!(matches!(
                a.input.as_ref().map(|i| &i.kind),
                Some(InputKind::Filter)
            ));
        }
    }

    /// The Inbox tab counts its rows; badges open it on their kind, a second
    /// click (or the kind chip) shows all kinds again.
    #[test]
    fn the_drawn_badges_open_the_inbox_on_their_kind() {
        use vega_herdr::theme::{DECISIONS, NOTES, WAITING};
        for w in [80, 140] {
            let mut a = top_bar_app();
            a.view = View::List;
            // Two decisions, one pane, one note.
            assert_eq!(
                click_drawn(&mut a, w, " Inbox 4 "),
                Some(Action::Tab(View::Inbox))
            );
            assert_eq!(a.view, View::Inbox);
            a.set_view(View::List);

            let d = badge(&DECISIONS, 5);
            assert_eq!(
                click_drawn(&mut a, w, &d),
                Some(Action::Bar(Bar::Badge(Kind::Decision)))
            );
            assert_eq!((a.view, a.inbox_kind), (View::Inbox, Some(Kind::Decision)));
            find(&draw(&mut a, w), "2/4");
            assert!(badge_selected(&mut a, w, &d), "the active kind's badge");
            assert!(!badge_selected(&mut a, w, &badge(&NOTES, 2)));
            click_drawn(&mut a, w, &d);
            assert_eq!(a.inbox_kind, None, "second click: all kinds");
            assert!(!badge_selected(&mut a, w, &d), "cleared");

            click_drawn(&mut a, w, &badge(&WAITING, 7));
            assert_eq!(a.inbox_kind, Some(Kind::Waiting));
            assert_eq!(
                click_drawn(&mut a, w, "waiting"),
                Some(Action::Bar(Bar::Kind))
            );
            assert_eq!(a.inbox_kind, None);

            click_drawn(&mut a, w, &badge(&NOTES, 2));
            assert_eq!(a.inbox_kind, Some(Kind::Note));
            handle_key(&mut a, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
            assert_eq!(a.inbox_kind, None, "Esc: all kinds");
            assert_eq!(a.view, View::Inbox);
        }
    }

    /// Inbox rows: waiting pane first, then newest first; a click selects
    /// the drawn row, Enter on a decision opens it in the bead tab, `m`
    /// answers it, `c` on it only hints.
    #[test]
    fn the_inbox_lists_jumps_answers_and_clears() {
        for w in [80, 140] {
            let mut a = top_bar_app();
            let rows = draw(&mut a, w);
            let order: Vec<u16> = ["flist", "t.1", "t.2", "reply from deimos"]
                .iter()
                .map(|t| find(&rows, t).1)
                .collect();
            let mut sorted = order.clone();
            sorted.sort();
            assert_eq!(order, sorted, "waiting, then newest first");
            find(&rows, "for you only");

            assert_eq!(
                click_drawn(&mut a, w, "Keep the alias?"),
                Some(Action::Row("d:t.2".into()))
            );
            press(&mut a, 'c');
            assert!(
                a.status_msg.contains("the UAC closes it"),
                "{}",
                a.status_msg
            );
            press(&mut a, 'm');
            assert!(matches!(
                a.input.as_ref().map(|i| &i.kind),
                Some(InputKind::Comment(id)) if id == "t.2"
            ));
            a.input = None;
            // x: dismiss asks for a reason; an empty one asks again.
            assert!(crate::ui::widgets::footer_keys(&a).contains(&"x dismiss".to_string()));
            press(&mut a, 'x');
            assert!(matches!(
                a.input.as_ref().map(|i| &i.kind),
                Some(InputKind::Dismiss(id)) if id == "t.2"
            ));
            handle_key(&mut a, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
            assert_eq!(a.status_msg, "dismiss needs a reason");
            assert!(matches!(
                a.input.as_ref().map(|i| &i.kind),
                Some(InputKind::Dismiss(_))
            ));
            a.input = None;
            handle_key(&mut a, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
            assert_eq!(a.view, View::List);
            assert_eq!(a.selected.as_deref(), Some("t.2"));
            assert!(a.detail_modal);

            // A note on another host: Enter says where it is.
            let mut a = top_bar_app();
            a.inbox_sel = Some("n:deimos:1791000000000".into());
            handle_key(&mut a, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
            assert!(a.status_msg.contains("on deimos"), "{}", a.status_msg);
            assert!(a.pane_jump.is_none());
            // A waiting pane: m points to the pane.
            a.inbox_sel = Some("w:w3V:p2".into());
            press(&mut a, 'm');
            assert!(a.input.is_none());
            assert!(a.status_msg.contains("answer it there"));
            press(&mut a, 'x');
            assert!(a.input.is_none(), "only a decision is dismissed");
        }
    }

    /// The footer as drawn: the rows below the body, joined.
    fn footer_text(a: &mut App, w: u16) -> Vec<String> {
        let rows = draw(a, w);
        let n = crate::ui::widgets::footer_lines(a, w).len();
        rows[rows.len() - n..]
            .iter()
            .map(|r| r.concat().trim_end().to_string())
            .collect()
    }

    /// The footer shows the keys of the tab and selection, one line when
    /// wide, wrapped at a separator when narrow; no key is cut.
    #[test]
    fn the_footer_is_contextual_and_wraps() {
        let mut a = top_bar_app();
        a.view = View::List;
        a.selected = Some("x".into());
        a.status_msg = "claimed".into();
        let keys = crate::ui::widgets::footer_keys(&a);
        assert!(keys.contains(&"c claim".to_string()));
        assert!(keys.contains(&"h/l fold".to_string()));
        assert!(!keys.iter().any(|k| k.starts_with("c clear")));

        let wide = footer_text(&mut a, 140);
        assert_eq!(wide.len(), 1, "{wide:?}");
        assert!(wide[0].starts_with(" claimed  "), "{wide:?}");

        let narrow = footer_text(&mut a, 60);
        assert!(narrow.len() >= 2, "{narrow:?}");
        let joined = narrow.join(" · ");
        for k in &keys {
            assert!(joined.contains(k.as_str()), "{k:?} cut in {narrow:?}");
        }
        for line in &narrow {
            assert!(line.chars().count() <= 60, "{line:?}");
            assert!(!line.trim_end().ends_with('·'), "{line:?}");
        }

        // The Inbox on a note: its keys, none of the bead tab's.
        let mut a = top_bar_app();
        a.inbox_sel = Some("n:deimos:1791000000000".into());
        let keys = crate::ui::widgets::footer_keys(&a);
        assert!(keys.iter().any(|k| k.starts_with("c clear")), "{keys:?}");
        assert!(!keys.contains(&"c claim".to_string()));
        assert!(
            !keys.contains(&"m answer".to_string()),
            "the note names no bead"
        );
        let narrow = footer_text(&mut a, 40);
        assert!(narrow.len() >= 2, "{narrow:?}");
    }

    /// Every key the footer can show is in the `?` help.
    #[test]
    fn footer_keys_are_in_the_help() {
        let help: String = help_lines()
            .iter()
            .map(|(k, d)| format!("{k} {d}\n"))
            .collect();
        for k in [
            "D",
            "N",
            "K",
            "m answer",
            "x dismiss",
            "c clear",
            "Esc all kinds",
            "o",
            "/",
        ] {
            let key = k.split(' ').next().unwrap();
            assert!(help.contains(key), "{k:?} not in the help");
        }
        assert!(help.contains("Inbox"));
        assert!(help.contains("lane|all"));
        let skill = crate::SKILL;
        for k in [
            "Inbox",
            "`D`",
            "`N`",
            "`m`",
            "`x`",
            "`c`",
            "lane|all",
            "repo|global",
        ] {
            assert!(skill.contains(k), "{k:?} not in --skill");
        }
    }
}
