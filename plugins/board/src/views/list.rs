//! Grouped-by-status list (phin-board style): collapsible status groups, one
//! bead per line, with a side/modal detail pane driven by the App.

use crate::app::App;
use crate::bd::types::Bead;
use crate::decisions::{self, Preset};
use crate::model::{status_glyph, status_label};
use crate::ui::theme;
use crate::ui::widgets::truncate_soft;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::Frame;

/// `tail` (the lane view's creator and age) ends the line after the title.
pub fn bead_line(b: &Bead, width: u16, tail: Option<String>) -> Line<'static> {
    // Prefix before the title: "  " (2) + "P0 " (3) + "{id:<13} " (14) = 19
    // cols plus the type tag. Reserve the deps suffix (space, symbol, count)
    // only when the bead carries deps. Theme symbols may be any width.
    let kind = theme::type_glyph(&b.issue_type);
    let deps = theme::icon(&vega_herdr::theme::DEPS);
    let dep_w = if b.dependency_count > 0 {
        1 + theme::cells(&deps) + b.dependency_count.to_string().len()
    } else {
        0
    };
    let tail_w = tail.as_ref().map_or(0, |t| t.chars().count());
    let title_w = (width as usize)
        .saturating_sub(19 + theme::cells(&kind) + dep_w + tail_w + theme::blocked_width(b))
        .max(6);
    let mut spans = vec![
        Span::raw("  "),
        Span::styled(
            format!("{} ", theme::priority_glyph(b.priority)),
            Style::default().fg(theme::priority_color(b.priority)),
        ),
        Span::styled(kind, Style::default().fg(theme::type_color(&b.issue_type))),
        Span::styled(
            format!("{:<13} ", truncate_soft(&b.id, 13)),
            Style::default().fg(theme::p().overlay1),
        ),
    ];
    spans.extend(theme::blocked_span(b));
    spans.push(Span::styled(
        truncate_soft(&b.title, title_w),
        Style::default().fg(theme::p().text),
    ));
    if b.dependency_count > 0 {
        spans.push(Span::styled(
            format!(" {deps}{}", b.dependency_count),
            Style::default().fg(theme::p().maroon),
        ));
    }
    if let Some(t) = tail {
        spans.push(Span::styled(t, Style::default().fg(theme::p().overlay1)));
    }
    Line::from(spans)
}

/// In the lane view: who created the bead (the sentinel's `<host>/<worktree>`)
/// and how long ago.
fn lane_tail(app: &App, b: &Bead, now: i64) -> Option<String> {
    (app.preset == Preset::Lane).then(|| {
        format!(
            "  {} {}",
            b.created_by.as_deref().unwrap_or("?"),
            decisions::bead_age(b, now)
        )
    })
}

pub fn render(f: &mut Frame, area: Rect, app: &mut App) {
    let now = decisions::unix_now();
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut ids: Vec<Option<String>> = Vec::new();

    for status in app.board_statuses() {
        let collapsed = app.collapsed.contains(&status);
        let group_ids = app.ids_in(&status);
        let arrow = if collapsed { "▸" } else { "▾" };
        let color = theme::status_color(&status);
        lines.push(Line::from(vec![
            Span::styled(format!("{arrow} "), Style::default().fg(color)),
            Span::styled(
                format!("{}{} ", status_glyph(&status), status_label(&status)),
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("({})", group_ids.len()),
                Style::default().fg(theme::p().overlay0),
            ),
        ]));
        ids.push(None);

        if collapsed {
            continue;
        }
        for id in group_ids {
            if let Some(b) = app.beads.iter().find(|x| x.id == id) {
                lines.push(bead_line(b, area.width, lane_tail(app, b, now)));
                ids.push(Some(id));
            }
        }
    }

    if lines.is_empty() {
        lines.push(Line::from(Span::styled(
            "  no beads - press a to create, g for global scope",
            Style::default().fg(theme::p().overlay0),
        )));
        ids.push(None);
    }

    super::render_lines(f, area, app, lines, ids);
}
