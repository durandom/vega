//! Shared UI helpers: tab row, footer, input prompt, help overlay.

use crate::app::App;
use crate::decisions::Preset;
use crate::form::CreateForm;
use crate::inbox::Kind;
use crate::keys::help_lines;
use crate::model::{status_label, Mode, Scope, View};
use crate::ui::theme;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph, Wrap};
use ratatui::Frame;

pub fn truncate(s: &str, w: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= w {
        let mut out: String = chars.into_iter().collect();
        while out.chars().count() < w {
            out.push(' ');
        }
        out
    } else if w == 0 {
        String::new()
    } else {
        let mut out: String = chars.into_iter().take(w.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

pub fn truncate_soft(s: &str, w: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= w {
        s.to_string()
    } else if w == 0 {
        String::new()
    } else {
        let mut out: String = chars.into_iter().take(w.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

/// Click rects of the badges drawn from `x` on, one space apart, as
/// `Badges::text` joins them. Each opens the Inbox filtered to its kind.
pub fn badge_hits(
    parts: &[(&vega_herdr::theme::Topic, String)],
    mut x: u16,
    y: u16,
) -> Vec<(crate::app::Bar, Rect)> {
    use crate::app::Bar;
    let mut out = Vec::new();
    for (t, text) in parts {
        let w = theme::cells(text) as u16;
        if let Some(k) = crate::inbox::Kind::of_topic(t) {
            out.push((Bar::Badge(k), Rect::new(x, y, w, 1)));
        }
        x += w + 1;
    }
    out
}

/// The column right after `spans`, drawn from `area`'s left edge.
fn end_of(area: Rect, spans: &[Span]) -> u16 {
    area.x + spans.iter().map(|s| s.width() as u16).sum::<u16>()
}

/// A badge's style: bold in the topic's `[symbols.colors]` colour.
pub(crate) fn badge_style(s: &vega_herdr::theme::Symbols, t: &vega_herdr::theme::Topic) -> Style {
    let fg = s
        .color(t)
        .map_or(theme::p().yellow, |c| Color::Rgb(c.0, c.1, c.2));
    Style::default().fg(fg).add_modifier(Modifier::BOLD)
}

/// A selected badge: its colour as the background, like the active tab.
fn selected(badge: Style) -> Style {
    Style::default()
        .fg(theme::p().base)
        .bg(badge.fg.unwrap_or(theme::p().teal))
        .add_modifier(Modifier::BOLD)
}

/// A two-way chip like `lane|all`: the active half highlighted; only
/// that half when `compact`.
fn two_way(a: &str, b: &str, first: bool, compact: bool) -> Vec<Span<'static>> {
    let on = Style::default()
        .fg(theme::p().base)
        .bg(theme::p().teal)
        .add_modifier(Modifier::BOLD);
    let off = Style::default().fg(theme::p().overlay1);
    if compact {
        return vec![Span::styled(if first { a } else { b }.to_string(), on)];
    }
    vec![
        Span::styled(a.to_string(), if first { on } else { off }),
        Span::styled("|", Style::default().fg(theme::p().overlay0)),
        Span::styled(b.to_string(), if first { off } else { on }),
    ]
}

/// The right side of the tab row: each chip with what a click on it does;
/// `compact` on a narrow pane.
fn tab_chips(app: &App, compact: bool) -> Vec<(crate::app::Bar, Vec<Span<'static>>)> {
    use crate::app::Bar;
    let count = |s: String| vec![Span::styled(s, Style::default().fg(theme::p().overlay1))];
    let scope = two_way("repo", "global", app.scope == Scope::Repo, compact);
    if app.view == View::Inbox {
        let kind = match app.inbox_kind {
            Some(k) => Span::styled(
                k.label(),
                Style::default()
                    .fg(theme::p().base)
                    .bg(theme::p().teal)
                    .add_modifier(Modifier::BOLD),
            ),
            None => Span::styled(
                if compact { "all" } else { "all kinds" },
                Style::default().fg(theme::p().overlay1),
            ),
        };
        let n = format!("{}/{}", app.inbox_items().len(), app.inbox_all().len());
        return vec![
            (Bar::Kind, vec![kind]),
            (Bar::Scope, scope),
            (Bar::Kind, count(n)),
        ];
    }
    let (shown, total) = app.shown_total();
    vec![
        (
            Bar::Preset,
            two_way("lane", "all", app.preset == Preset::Lane, compact),
        ),
        (Bar::Scope, scope),
        (Bar::Count, count(format!("{shown}/{total}"))),
    ]
}

/// The tab row: the tabs (Inbox with its count), vegad's badges, the state
/// marks, then the chips right-aligned (docs/design/board-ui.md). Records
/// tab rects into `app.hits.tabs` and badge and chip rects into
/// `app.hits.bar` for mouse clicks.
pub fn render_activity_bar(f: &mut Frame, area: Rect, app: &mut App) {
    let mut spans: Vec<Span> = Vec::new();
    let mut x = area.x + 1;
    spans.push(Span::styled(" ", Style::default()));

    let width = |s: &[Span]| s.iter().map(|s| s.width() as u16).sum::<u16>();
    let chips_width =
        |c: &[(crate::app::Bar, Vec<Span>)]| c.iter().map(|(_, s)| width(s) + 2).sum::<u16>();
    let mut chips = tab_chips(app, false);
    let sym = vega_herdr::theme::symbols();
    let parts = app
        .badges
        .now
        .filter(|b| !b.is_zero())
        .map(|b| b.parts(&sym))
        .unwrap_or_default();
    let badges_w = parts
        .iter()
        .map(|(_, t)| theme::cells(t) as u16 + 1)
        .sum::<u16>()
        + if parts.is_empty() { 0 } else { 1 };

    let inbox_n = app.inbox_count();
    let label = |v: View, short: bool| match (v, short) {
        (View::Inbox, false) => format!(" {} {inbox_n} ", v.title()),
        (View::Inbox, true) => format!(" I{inbox_n} "),
        (_, false) => format!(" {} ", v.title()),
        (_, true) => format!(" {} ", &v.title()[..1]),
    };
    let full_w = View::ALL
        .iter()
        .map(|v| label(*v, false).chars().count() as u16 + 1)
        .sum::<u16>();
    // A narrow pane: the other tabs shrink to their letter and the chips
    // show only their active half, so all still fit.
    let short = 1 + full_w + badges_w + 2 + chips_width(&chips) > area.width;
    if short {
        chips = tab_chips(app, true);
    }
    let chips_w = chips_width(&chips);
    for v in View::ALL {
        let label = label(v, short && v != app.view);
        let w = label.chars().count() as u16;
        let style = if v == app.view {
            Style::default()
                .fg(theme::p().base)
                .bg(theme::p().mauve)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme::p().subtext).bg(Color::Reset)
        };
        app.hits.tabs.push((v, Rect::new(x, area.y, w, 1)));
        spans.push(Span::styled(label, style));
        spans.push(Span::raw(" "));
        x += w + 1;
    }

    // The UAC pane's badges, from vegad (notes, decisions, waiting), each
    // in its theme.toml [symbols.colors] colour, as herdr's sidebar tokens
    // draw the UAC label. A click opens the Inbox on that kind; the badge
    // of the Inbox's kind filter is drawn selected, a click on it clears it.
    if !parts.is_empty() {
        let x = end_of(area, &spans) + 2;
        app.hits.bar.extend(badge_hits(&parts, x, area.y));
        let active = app.inbox_kind.filter(|_| app.view == View::Inbox);
        spans.push(Span::raw(" "));
        for (t, text) in parts {
            spans.push(Span::raw(" "));
            let style = badge_style(&sym, t);
            let style = if active.is_some() && crate::inbox::Kind::of_topic(t) == active {
                selected(style)
            } else {
                style
            };
            spans.push(Span::styled(text, style));
        }
    }

    let mut marks = String::new();
    if app.show_closed {
        marks.push_str("  +closed");
    }
    if app.auto_dock_enabled() {
        marks.push_str("  +auto");
    }
    if app.live && app.scope == Scope::Repo {
        marks.push_str("  +live");
    }
    if !app.filter.is_empty() {
        marks.push_str(&format!("  /{}", app.filter));
    }
    spans.push(Span::styled(
        marks,
        Style::default().fg(theme::p().overlay1),
    ));

    // A theme.toml [symbols] mistake: the first one, and how many.
    let errors = vega_herdr::theme::symbols().errors.clone();
    if let Some(first) = errors.first() {
        let more = match errors.len() {
            1 => String::new(),
            n => format!(" (+{})", n - 1),
        };
        spans.push(Span::styled(
            format!("  {first}{more}"),
            Style::default().fg(theme::p().red),
        ));
    }

    if app.vegad.stopped() {
        spans.push(Span::styled(
            "  vegad stopped · V ",
            Style::default()
                .fg(theme::p().red)
                .add_modifier(Modifier::BOLD),
        ));
    }

    if app.move_mode {
        spans.push(Span::styled(
            "  MOVE ",
            Style::default()
                .fg(theme::p().base)
                .bg(theme::p().peach)
                .add_modifier(Modifier::BOLD),
        ));
    }

    // The chips, right-aligned; on a narrow pane they follow the rest two
    // cells apart and run off the edge last.
    let left_end = end_of(area, &spans);
    let start = (left_end + 2).max((area.x + area.width).saturating_sub(chips_w));
    spans.push(Span::raw(" ".repeat((start - left_end) as usize)));
    let mut cx = start;
    for (bar, chip) in chips {
        let w = width(&chip);
        app.hits.bar.push((bar, Rect::new(cx, area.y, w, 1)));
        spans.extend(chip);
        spans.push(Span::raw("  "));
        cx += w + 2;
    }

    let bar = Paragraph::new(Line::from(spans)).style(Style::default().bg(Color::Reset));
    f.render_widget(bar, area);
}

/// The keys that apply to the current tab and selection, in footer order.
pub fn footer_keys(app: &App) -> Vec<String> {
    if app.move_mode {
        return vec!["h/l retag".into(), "v/Esc exit".into()];
    }
    let popup = app.mode == Mode::Popup;
    let mut k: Vec<&str> = Vec::new();
    // The popup grabs every key and Esc only backs out a layer: `q` is the
    // only way out, so it leads there.
    if popup {
        k.push("q close board");
    }
    if app.view == View::Inbox {
        if let Some(item) = app.inbox_selected() {
            match item.kind() {
                Kind::Decision => k.extend(["Enter open", "m answer", "x dismiss"]),
                Kind::Waiting => k.push("Enter jump"),
                Kind::Note => {
                    if item.pane().is_some() {
                        k.push("Enter jump");
                    }
                    if item.bead().is_some() {
                        k.push("m answer");
                    }
                    k.push("c clear (and that host's older notes)");
                }
            }
        }
        k.push("j/k move");
        if app.inbox_kind.is_some() {
            k.push("Esc all kinds");
        }
        if app.vegad.stopped() {
            k.push("V start vegad");
        }
        k.extend(["r refresh", "K view"]);
    } else {
        if app.selected.is_some() {
            k.extend(["Enter detail", "m answer", "c claim", "x close"]);
        }
        k.push("j/k move");
        match app.view {
            View::List => k.push("h/l fold"),
            View::Kanban => k.push("h/l column"),
            View::Table => k.push("o sort"),
            View::Inbox => {}
        }
        k.push(if app.filter.is_empty() {
            "/ filter"
        } else {
            "Esc clear filter"
        });
        k.extend(["D lane|all", "N inbox", "K view"]);
    }
    k.push("? help");
    if !popup {
        k.push("q quit");
    }
    k.into_iter().map(String::from).collect()
}

/// The footer: the status message, then the keys joined by ` · `, wrapped
/// onto further lines at a separator instead of cut.
pub fn footer_lines(app: &App, width: u16) -> Vec<Line<'static>> {
    wrap_footer(&app.status_msg, &footer_keys(app), width)
}

pub fn wrap_footer(status: &str, keys: &[String], width: u16) -> Vec<Line<'static>> {
    let green = Style::default().fg(theme::p().green);
    let dim = Style::default().fg(theme::p().overlay0);
    let mut segs: Vec<(String, Style, &str)> = Vec::new();
    if !status.is_empty() {
        segs.push((status.to_string(), green, ""));
    }
    for (i, key) in keys.iter().enumerate() {
        let sep = match (i, status.is_empty()) {
            (0, true) => "",
            (0, false) => "  ",
            _ => " · ",
        };
        segs.push((key.clone(), dim, sep));
    }
    let width = width as usize;
    let mut lines: Vec<Vec<Span<'static>>> = Vec::new();
    let mut cur: Vec<Span<'static>> = Vec::new();
    let mut cur_w = 0;
    for (text, style, sep) in segs {
        let w = text.chars().count();
        if cur.is_empty() {
            cur.push(Span::raw(" "));
            cur_w = 1;
        } else if cur_w + sep.chars().count() + w > width {
            lines.push(std::mem::take(&mut cur));
            cur.push(Span::raw(" "));
            cur_w = 1;
        } else {
            cur.push(Span::styled(sep.to_string(), dim));
            cur_w += sep.chars().count();
        }
        cur.push(Span::styled(text, style));
        cur_w += w;
    }
    if !cur.is_empty() {
        lines.push(cur);
    }
    lines.into_iter().map(Line::from).collect()
}

pub fn render_footer(f: &mut Frame, area: Rect, lines: Vec<Line<'static>>) {
    f.render_widget(
        Paragraph::new(lines).style(Style::default().bg(Color::Reset)),
        area,
    );
}

/// Centre a help popup tall enough for `content_lines`, never taller than the
/// pane. Width stays generous so long descriptions do not wrap and push later
/// entries out of view.
pub fn help_rect(area: Rect, content_lines: u16) -> Rect {
    let height = content_lines.saturating_add(2).min(area.height);
    let width = area.width.saturating_mul(4) / 5;
    let width = width.max(40).min(area.width);

    Rect {
        x: area.x + (area.width.saturating_sub(width)) / 2,
        y: area.y + (area.height.saturating_sub(height)) / 2,
        width,
        height,
    }
}

pub fn centered_rect(px: u16, py: u16, area: Rect) -> Rect {
    let v = Layout::vertical([
        Constraint::Percentage((100 - py) / 2),
        Constraint::Percentage(py),
        Constraint::Percentage((100 - py) / 2),
    ])
    .split(area);
    Layout::horizontal([
        Constraint::Percentage((100 - px) / 2),
        Constraint::Percentage(px),
        Constraint::Percentage((100 - px) / 2),
    ])
    .split(v[1])[1]
}

pub fn render_input(f: &mut Frame, area: Rect, app: &App) {
    let Some(inp) = app.input.as_ref() else {
        return;
    };
    let w = area.width.min(70);
    // Grow with the text (answers run long), wrapped, up to half the pane.
    let inner = w.saturating_sub(2).max(1) as usize;
    let rows = (inp.buffer.chars().count() + 3).div_ceil(inner) as u16;
    let h = (rows + 2).min((area.height / 2).max(3));
    let rect = Rect::new(
        area.x + (area.width.saturating_sub(w)) / 2,
        area.y + area.height.saturating_sub(h) / 2,
        w,
        h,
    );
    f.render_widget(Clear, rect);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme::p().mauve))
        .title(format!(" {} ", inp.title))
        .style(Style::default().bg(Color::Reset));
    let text = Line::from(vec![
        Span::styled("› ", Style::default().fg(theme::p().mauve)),
        Span::styled(inp.buffer.clone(), Style::default().fg(theme::p().text)),
        Span::styled("█", Style::default().fg(theme::p().mauve)),
    ]);
    // Keep the cursor in view once the text outgrows the box.
    let scroll = rows.saturating_sub(h.saturating_sub(2));
    f.render_widget(
        Paragraph::new(text)
            .block(block)
            .wrap(Wrap { trim: false })
            .scroll((scroll, 0)),
        rect,
    );
}

pub fn render_create_form(f: &mut Frame, area: Rect, form: &CreateForm) {
    use crate::form::{
        F_ASSIGNEE, F_BACKLOG, F_DESC, F_EPIC, F_LABELS, F_PRIORITY, F_TITLE, F_TYPE,
    };
    let rect = centered_rect(68, 84, area);
    f.render_widget(Clear, rect);
    let title = if form.edit_id.is_some() {
        " Edit bead - Tab/↑↓ field · ←→ choose · Space toggle · Enter save · Esc cancel "
    } else {
        " New bead - Tab/↑↓ field · ←→ choose · Space toggle · Enter create · Esc cancel "
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme::p().mauve))
        .title(title)
        .style(Style::default().bg(Color::Reset));

    let cur = form.field;
    let label = |i: u8, name: &str| {
        let style = if i == cur {
            Style::default()
                .fg(theme::p().mauve)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme::p().overlay1)
        };
        Span::styled(format!("  {name:<13}"), style)
    };
    let cursor = |i: u8| if i == cur { "█" } else { "" };
    let text = |s: &str, ph: &str| {
        if s.is_empty() {
            Span::styled(ph.to_string(), Style::default().fg(theme::p().overlay0))
        } else {
            Span::styled(s.to_string(), Style::default().fg(theme::p().text))
        }
    };

    let it = form.issue_type();
    let pname = match form.priority {
        0 => "critical",
        1 => "high",
        2 => "medium",
        3 => "low",
        _ => "backlog",
    };
    let check = if form.deferred { "[x]" } else { "[ ]" };
    let epic_fg = if form.parent_id().is_empty() {
        theme::p().overlay1
    } else {
        theme::p().mauve
    };

    let lines = vec![
        Line::from(vec![
            label(F_TYPE, "Type"),
            Span::styled(
                format!("‹ {it} ›"),
                Style::default()
                    .fg(theme::type_color(it))
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            label(F_PRIORITY, "Priority"),
            Span::styled(
                format!("‹ {} · {pname} ›", theme::priority_glyph(form.priority)),
                Style::default()
                    .fg(theme::priority_color(form.priority))
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::raw(""),
        Line::from(vec![
            label(F_TITLE, "Title"),
            text(&form.title, "what needs doing?"),
            Span::styled(cursor(F_TITLE), Style::default().fg(theme::p().mauve)),
        ]),
        Line::from(vec![
            label(F_DESC, "Description"),
            text(&form.description, "optional details / acceptance"),
            Span::styled(cursor(F_DESC), Style::default().fg(theme::p().mauve)),
        ]),
        Line::raw(""),
        Line::from(vec![
            label(F_ASSIGNEE, "Assignee"),
            text(&form.assignee, "unassigned"),
            Span::styled(cursor(F_ASSIGNEE), Style::default().fg(theme::p().mauve)),
        ]),
        Line::from(vec![
            label(F_EPIC, "Parent epic"),
            Span::styled(
                format!("‹ {} ›", form.epic_label()),
                Style::default().fg(epic_fg),
            ),
        ]),
        Line::from(vec![
            label(F_LABELS, "Labels"),
            text(&form.labels, "comma,separated"),
            Span::styled(cursor(F_LABELS), Style::default().fg(theme::p().mauve)),
        ]),
        Line::raw(""),
        Line::from(vec![
            label(F_BACKLOG, "Backlog"),
            Span::styled(
                format!("{check} start deferred  (Space)"),
                Style::default().fg(if form.deferred {
                    theme::p().yellow
                } else {
                    theme::p().overlay0
                }),
            ),
        ]),
    ];
    f.render_widget(
        Paragraph::new(lines)
            .block(block)
            .wrap(Wrap { trim: false }),
        rect,
    );
}

/// Status picker overlay: board statuses numbered 1-9 for a one-key retag.
pub fn render_status_pick(f: &mut Frame, area: Rect, app: &App) {
    let statuses = app.board_statuses();
    let mut spans: Vec<Span> = Vec::new();
    for (i, s) in statuses.iter().enumerate().take(9) {
        spans.push(Span::styled(
            format!(" {} ", i + 1),
            Style::default().fg(theme::p().overlay0),
        ));
        spans.push(Span::styled(
            format!("{}  ", status_label(s)),
            Style::default()
                .fg(theme::status_color(s))
                .add_modifier(Modifier::BOLD),
        ));
    }
    let w = area.width.min(74);
    let rect = Rect::new(
        area.x + (area.width.saturating_sub(w)) / 2,
        area.y + area.height / 2,
        w,
        3,
    );
    f.render_widget(Clear, rect);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme::p().mauve))
        .title(" Set status - press 1-9 (Esc cancels) ")
        .style(Style::default().bg(Color::Reset));
    f.render_widget(Paragraph::new(Line::from(spans)).block(block), rect);
}

pub fn render_help(f: &mut Frame, area: Rect) {
    let mut lines: Vec<Line> = vec![
        Line::from(Span::styled(
            "vega board - keys",
            Style::default()
                .fg(theme::p().mauve)
                .add_modifier(Modifier::BOLD),
        )),
        Line::raw(""),
    ];
    for (k, d) in help_lines() {
        lines.push(Line::from(vec![
            Span::styled(
                format!("  {:<10}", k),
                Style::default().fg(theme::p().yellow),
            ),
            Span::styled(d.to_string(), Style::default().fg(theme::p().subtext)),
        ]));
    }
    lines.push(Line::raw(""));
    lines.push(Line::from(Span::styled(
        "  any key to close",
        Style::default().fg(theme::p().overlay0),
    )));

    // Size to the content instead of a fixed 80% of the pane. At a fixed
    // fraction the list is silently truncated by Paragraph, and the entries
    // near the bottom - including "C  toggle showing closed" - are simply not
    // there to be found.
    let rect = help_rect(area, lines.len() as u16);
    f.render_widget(Clear, rect);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme::p().mauve))
        .style(Style::default().bg(Color::Reset));
    f.render_widget(
        Paragraph::new(lines)
            .block(block)
            .alignment(Alignment::Left)
            .wrap(Wrap { trim: false }),
        rect,
    );
}

#[cfg(test)]
mod help_rect_tests {
    use super::*;

    fn area(w: u16, h: u16) -> Rect {
        Rect {
            x: 0,
            y: 0,
            width: w,
            height: h,
        }
    }

    /// The whole point: every help line must have somewhere to render. At a
    /// fixed fraction of the pane the tail of the list was cut off, which is
    /// how "C  toggle showing closed" went missing.
    #[test]
    fn tall_enough_for_every_line_when_the_pane_allows() {
        let r = help_rect(area(120, 50), 29);
        assert_eq!(r.height, 31, "29 lines + 2 border rows");
        assert!(r.y + r.height <= 50);
    }

    #[test]
    fn never_taller_than_the_pane() {
        for h in [1u16, 2, 5, 20, 31] {
            let r = help_rect(area(120, h), 29);
            assert!(r.height <= h, "escaped a {h}-row pane");
            assert!(r.y + r.height <= h);
        }
    }

    #[test]
    fn stays_inside_a_narrow_pane() {
        for w in [1u16, 10, 39, 40, 120] {
            let r = help_rect(area(w, 50), 29);
            assert!(r.width <= w, "escaped a {w}-column pane");
            assert!(r.x + r.width <= w);
        }
    }
}
