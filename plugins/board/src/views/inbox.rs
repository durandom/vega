//! The Inbox tab (`crate::inbox`): vegad's state on this host, then one row
//! per item, each with its kind's themed symbol and colour.

use crate::app::{App, Area};
use crate::inbox::{Item, Source};
use crate::ui::theme;
use crate::ui::widgets::{badge_style, truncate, truncate_soft};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

/// One row: symbol, when, who, what, where.
fn row(item: &Item, width: u16, now: i64, sym: &vega_herdr::theme::Symbols) -> Line<'static> {
    let dim = Style::default().fg(theme::p().overlay1);
    let icon = sym.icon(item.kind().topic());
    let when = match (&item.source, item.at) {
        (Source::Waiting(w), _) => w.attention.clone(),
        (_, Some(at)) => crate::decisions::age(now - at),
        (_, None) => "?".into(),
    };
    let (label, title, tail, warn) = match &item.source {
        Source::Decision { id, title } => (id.clone(), title.clone(), "  decision".into(), false),
        Source::Waiting(w) => (
            w.name.clone(),
            w.title.clone(),
            format!("  [{}]", w.pane),
            false,
        ),
        Source::Note(n) => {
            let quarantine = n.kind == "quarantine";
            let mut tail = n
                .pane
                .as_ref()
                .map(|p| format!("  [{p}]"))
                .unwrap_or_default();
            if quarantine {
                tail = format!("  for you only{tail}");
            }
            let body = n.body.replace('\n', " ");
            let title = if body.is_empty() || width < 110 {
                n.title.clone()
            } else {
                format!("{} - {body}", n.title)
            };
            (format!("{} · {}", n.host, n.kind), title, tail, quarantine)
        }
    };
    let label_w = if width >= 110 { 22 } else { 14 };
    let used = theme::cells(&icon) + 1 + 6 + 1 + label_w + 1 + tail.chars().count() + 1;
    let title_w = (width as usize).saturating_sub(used).max(6);
    let label_style = if warn {
        Style::default().fg(theme::p().peach)
    } else {
        dim
    };
    Line::from(vec![
        Span::raw(" "),
        Span::styled(icon, badge_style(sym, item.kind().topic())),
        Span::styled(format!("{when:>6} "), dim),
        Span::styled(format!("{} ", truncate(&label, label_w)), label_style),
        Span::styled(
            truncate_soft(&title, title_w),
            Style::default().fg(theme::p().text),
        ),
        Span::styled(tail, if warn { label_style } else { dim }),
    ])
}

pub fn render(f: &mut Frame, area: Rect, app: &mut App) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    app.hits.areas.push((Area::Inbox, area));
    let dim = Style::default().fg(theme::p().overlay0);
    let mut lines: Vec<Line<'static>> = Vec::new();
    let vegad = if app.vegad.stopped() {
        Style::default()
            .fg(theme::p().red)
            .add_modifier(Modifier::BOLD)
    } else {
        dim
    };
    lines.push(Line::styled(format!(" {}", app.vegad.line()), vegad));
    if let Some(p) = app.tracker.problem().filter(|_| !app.beads.is_empty()) {
        lines.push(Line::styled(
            format!(" {p}"),
            Style::default().fg(theme::p().peach),
        ));
    }
    if let Some(e) = &app.notes.error {
        lines.push(Line::styled(
            format!(" notes: {e}"),
            Style::default().fg(theme::p().red),
        ));
    } else if app.notes.loading.is_some() && !app.notes.loaded {
        lines.push(Line::styled(" loading notes (vega post notes)...", dim));
    }
    let head = lines.len();

    let items = app.inbox_items();
    let sel = app.inbox_selected().map(|i| i.key);
    let sym = vega_herdr::theme::symbols();
    let now = crate::decisions::unix_now();
    for item in &items {
        lines.push(row(item, area.width, now, &sym));
    }
    if items.is_empty() {
        let what = match app.inbox_kind {
            Some(k) => format!(" no {} · Esc shows all kinds", k.label()),
            None => " nothing needs you".into(),
        };
        lines.push(Line::styled(what, dim));
    }

    let h = area.height as usize;
    let sel_line = sel
        .as_ref()
        .and_then(|k| items.iter().position(|i| &i.key == k))
        .map(|i| head + i);
    let offset = match sel_line {
        Some(s) if lines.len() > h => s.saturating_sub(h / 2).min(lines.len() - h),
        _ => 0,
    };
    f.render_widget(
        Paragraph::new(lines)
            .scroll((offset as u16, 0))
            .style(Style::default().bg(Color::Reset)),
        area,
    );
    for (i, item) in items.iter().enumerate() {
        let line = head + i;
        if line < offset || line >= offset + h {
            continue;
        }
        let rect = Rect::new(area.x, area.y + (line - offset) as u16, area.width, 1);
        app.hits.rows.push((item.key.clone(), rect));
        if sel.as_deref() == Some(item.key.as_str()) {
            f.buffer_mut()
                .set_style(rect, Style::default().bg(theme::p().surface1));
        }
    }
}
