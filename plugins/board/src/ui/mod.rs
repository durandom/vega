//! Top-level compositor: tab row, body (view + optional detail pane), the
//! footer (wraps when narrow), and the modal/overlay layers
//! (docs/design/board-ui.md).

pub mod theme;
pub mod widgets;

use crate::app::App;
use crate::model::View;
use crate::views;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::Frame;

pub fn render(f: &mut Frame, app: &mut App) {
    app.hits.clear();
    let area = f.area();
    let footer = widgets::footer_lines(app, area.width);
    // The footer may wrap, but never takes more than a third of the pane.
    let footer_h = (footer.len() as u16).clamp(1, (area.height / 3).max(1));
    let rows = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(footer_h),
    ])
    .split(area);

    widgets::render_activity_bar(f, rows[0], app);

    let body = rows[1];
    // Page motions move by what is on screen, so remember the body height.
    app.viewport_rows = body.height;
    let want_detail = app.show_detail && matches!(app.view, View::List | View::Table);
    if want_detail && body.width > 70 {
        // wide: detail beside the list
        let detail_w = (body.width / 3).clamp(30, 52);
        let cols =
            Layout::horizontal([Constraint::Min(0), Constraint::Length(detail_w)]).split(body);
        render_view(f, cols[0], app);
        views::detail::render_side(f, cols[1], app);
    } else if want_detail {
        // narrow sidebar: float the detail on top of the list (Clear-backed
        // modal) instead of splitting it below, so the full list stays put.
        render_view(f, body, app);
        views::detail::render_modal(f, area, app);
    } else {
        render_view(f, body, app);
    }

    widgets::render_footer(f, rows[2], footer);

    if app.detail_modal {
        views::detail::render_modal(f, area, app);
    }
    if let Some(fm) = app.create_form.as_ref() {
        widgets::render_create_form(f, area, fm);
    }
    if app.input.is_some() {
        widgets::render_input(f, area, app);
    }
    if app.show_help {
        widgets::render_help(f, area);
    }
    if app.status_pick {
        widgets::render_status_pick(f, area, app);
    }
}

fn render_view(f: &mut Frame, area: Rect, app: &mut App) {
    if matches!(app.view, View::List | View::Table) {
        app.hits.areas.push((crate::app::Area::List, area));
    }
    match app.view {
        View::List => views::list::render(f, area, app),
        View::Table => views::table::render(f, area, app),
        View::Kanban => views::kanban::render(f, area, app),
        View::Inbox => views::inbox::render(f, area, app),
    }
}
