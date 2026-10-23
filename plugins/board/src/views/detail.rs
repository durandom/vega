//! The detail pane/modal: full bead info, enriched by `bd show` when cached.
//!
//! The text is wrapped here, not by ratatui, so every clickable id has an
//! exact rect: bead ids in the relations (parent, children, dependencies,
//! blocked by) and herdr pane ids anywhere (`app.hits.links`).

use crate::app::{App, Area, Link};
use crate::bd::types::Bead;
use crate::model::{status_glyph, status_label};
use crate::ui::theme::{self, cells};
use crate::ui::widgets::centered_rect;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph};
use ratatui::Frame;

/// One line before wrapping. `links` are bead ids at (cell, width); a line
/// with links is clipped, never wrapped, so they stay put. A `wrap` line is
/// one span.
pub struct DLine {
    pub line: Line<'static>,
    pub links: Vec<(usize, usize, String)>,
    pub wrap: bool,
}

/// A line on screen and the ids on it at (cell, width).
pub type Row = (Line<'static>, Vec<(usize, usize, Link)>);

fn fixed(line: Line<'static>) -> DLine {
    DLine {
        line,
        links: Vec::new(),
        wrap: false,
    }
}

fn text(s: String, style: Style) -> DLine {
    DLine {
        line: Line::from(Span::styled(s, style)),
        links: Vec::new(),
        wrap: true,
    }
}

/// `prefix`, then the ids as links joined by `", "`, then `suffix`.
fn linked(prefix: String, ids: &[&str], suffix: String, style: Style) -> DLine {
    let mut s = prefix;
    let mut links = Vec::new();
    for (i, id) in ids.iter().enumerate() {
        if i > 0 {
            s.push_str(", ");
        }
        links.push((cells(&s), cells(id), id.to_string()));
        s.push_str(id);
    }
    s.push_str(&suffix);
    DLine {
        line: Line::from(Span::styled(s, style)),
        links,
        wrap: false,
    }
}

pub fn build_lines(b: &Bead, beads: &[Bead]) -> Vec<DLine> {
    let title_of = |id: &str| {
        beads
            .iter()
            .find(|x| x.id == id)
            .map(|x| format!(" - {}", x.title))
            .unwrap_or_default()
    };
    let head = Style::default().fg(theme::p().yellow);
    let sub = Style::default().fg(theme::p().subtext);
    let mut lines: Vec<DLine> = Vec::new();
    lines.push(fixed(Line::from(Span::styled(
        b.id.clone(),
        Style::default().fg(theme::p().overlay1),
    ))));
    lines.push(text(
        b.title.clone(),
        Style::default()
            .fg(theme::p().text)
            .add_modifier(Modifier::BOLD),
    ));
    lines.push(fixed(Line::raw("")));
    lines.push(fixed(Line::from(vec![
        Span::styled(
            format!("{}{}", status_glyph(&b.status), status_label(&b.status)),
            Style::default().fg(theme::status_color(&b.status)),
        ),
        Span::raw("   "),
        Span::styled(
            theme::priority_glyph(b.priority).to_string(),
            Style::default().fg(theme::priority_color(b.priority)),
        ),
        Span::raw("   "),
        Span::styled(b.issue_type.clone(), sub),
        Span::raw("   "),
        Span::styled(
            format!("@{}", b.assignee()),
            Style::default().fg(theme::p().overlay0),
        ),
    ])));
    if let (Some(c), Some(u)) = (&b.created_at, &b.updated_at) {
        lines.push(text(
            format!("created {c} · updated {u}"),
            Style::default().fg(theme::p().overlay0),
        ));
    }
    lines.push(fixed(Line::raw("")));
    if !b.description.is_empty() {
        lines.push(fixed(Line::from(Span::styled("Description", head))));
        // One Line per text line: a Span cannot hold a line break.
        for l in b.description.lines() {
            lines.push(text(l.to_string(), sub));
        }
        lines.push(fixed(Line::raw("")));
    }
    if b.is_blocked {
        let style = Style::default().fg(theme::p().red);
        let prefix = format!(
            "{}Blocked by: ",
            theme::icon(&vega_herdr::theme::BLOCKED_BY)
        );
        if b.blocked_by.is_empty() {
            lines.push(text(format!("{prefix}another bead"), style));
        } else {
            let ids: Vec<&str> = b.blocked_by.iter().map(String::as_str).collect();
            lines.push(linked(prefix, &ids, String::new(), style));
        }
        lines.push(fixed(Line::raw("")));
    }
    // The parent is a dependency in `bd show`; the board's copy may lack it.
    if let Some(p) = b
        .parent
        .as_deref()
        .filter(|p| !b.dependencies.iter().any(|d| d.other_id() == Some(*p)))
    {
        lines.push(linked("Parent: ".into(), &[p], title_of(p), head));
        lines.push(fixed(Line::raw("")));
    }
    if !b.dependencies.is_empty() {
        lines.push(fixed(Line::from(Span::styled(
            format!("Dependencies ({})", b.dependencies.len()),
            head,
        ))));
        for d in &b.dependencies {
            let kind = d
                .dep_type
                .as_deref()
                .map(|k| format!("{k}: "))
                .unwrap_or_default();
            let title = d
                .title
                .as_deref()
                .map(|t| format!(" - {t}"))
                .unwrap_or_default();
            match d.other_id() {
                Some(id) => lines.push(linked(format!("  • {kind}"), &[id], title, sub)),
                None => lines.push(text(format!("  • {}", d.label()), sub)),
            }
        }
        lines.push(fixed(Line::raw("")));
    }
    let children: Vec<&Bead> = beads
        .iter()
        .filter(|x| x.parent.as_deref() == Some(b.id.as_str()))
        .collect();
    if !children.is_empty() {
        lines.push(fixed(Line::from(Span::styled(
            format!("Children ({})", children.len()),
            head,
        ))));
        for c in children {
            lines.push(linked(
                "  • ".into(),
                &[&c.id],
                format!(" - {}", c.title),
                sub,
            ));
        }
        lines.push(fixed(Line::raw("")));
    }
    if !b.comments.is_empty() {
        lines.push(fixed(Line::from(Span::styled(
            format!("Comments ({})", b.comments.len()),
            head,
        ))));
        for c in &b.comments {
            // "2026-10-09T07:00:34Z" -> "2026-10-09 07:00"
            let when: String = c
                .created_at
                .replacen('T', " ", 1)
                .chars()
                .take(16)
                .collect();
            lines.push(text(
                format!("{} · {when}", c.author),
                Style::default().fg(theme::p().overlay1),
            ));
            for l in c.text.lines() {
                lines.push(text(format!("  {l}"), Style::default().fg(theme::p().text)));
            }
        }
        lines.push(fixed(Line::raw("")));
    }
    lines.push(text(
        format!(
            "deps {} · dependents {} · comments {}",
            b.dependency_count, b.dependent_count, b.comment_count
        ),
        Style::default().fg(theme::p().overlay0),
    ));
    lines
}

/// Word-wrap `s` to `width` cells; a word longer than a line is split.
pub fn wrap(s: &str, width: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = s;
    while width > 0 && cells(rest) > width {
        let (mut used, mut cut, mut space) = (0, rest.len(), None);
        for (i, c) in rest.char_indices() {
            let w = cells(c.encode_utf8(&mut [0; 4]));
            if used + w > width {
                if c == ' ' {
                    space = Some(i);
                }
                // Never an empty line: a cell too narrow still takes a char.
                cut = if i == 0 { c.len_utf8() } else { i };
                break;
            }
            if c == ' ' {
                space = Some(i);
            }
            used += w;
        }
        let (line, next) = match space.filter(|&i| i > 0) {
            Some(i) => (&rest[..i], &rest[i + 1..]),
            None => (&rest[..cut], &rest[cut..]),
        };
        out.push(line.trim_end().to_string());
        rest = next.trim_start();
    }
    out.push(rest.to_string());
    out
}

/// The herdr pane ids in `s` at (cell, width), as vega open recognizes them.
pub fn pane_ids(s: &str) -> Vec<(usize, usize, Link)> {
    let mut out = Vec::new();
    let mut at = 0;
    for word in s.split(' ') {
        let id = vega_herdr::pane::peel(word);
        if !id.is_empty() && vega_herdr::pane::parse(id).is_some() {
            let off = word.find(id).unwrap_or(0);
            let col = cells(&s[..at + off]);
            out.push((col, cells(id), Link::Pane(id.to_string())));
        }
        at += word.len() + 1;
    }
    out
}

/// The lines as screen rows `width` cells wide, with their links.
pub fn layout(lines: Vec<DLine>, width: usize) -> Vec<Row> {
    let mut rows = Vec::new();
    for l in lines {
        let span = l.line.spans.first().cloned();
        match span {
            Some(sp) if l.wrap && l.line.spans.len() == 1 => {
                for part in wrap(&sp.content, width) {
                    let links = pane_ids(&part);
                    rows.push((Line::from(Span::styled(part, sp.style)), links));
                }
            }
            _ => {
                let plain: String = l.line.spans.iter().map(|s| s.content.as_ref()).collect();
                let mut links: Vec<_> = l
                    .links
                    .into_iter()
                    .map(|(c, w, id)| (c, w, Link::Bead(id)))
                    .collect();
                links.extend(pane_ids(&plain));
                rows.push((l.line, links));
            }
        }
    }
    rows
}

fn lines_for(app: &App) -> Vec<DLine> {
    match app.detail_bead() {
        Some(b) => build_lines(&b, &app.beads),
        None => vec![fixed(Line::from(Span::styled(
            "no selection",
            Style::default().fg(theme::p().overlay0),
        )))],
    }
}

/// Draw the detail text in `block` at `rect`, scrolled, and record its
/// area and links.
fn render_in(f: &mut Frame, rect: Rect, block: Block<'static>, app: &mut App) {
    let inner = block.inner(rect);
    f.render_widget(block, rect);
    app.hits.areas.push((Area::Detail, rect));
    if app.detail_scroll_id != app.selected {
        app.detail_scroll_id = app.selected.clone();
        app.detail_scroll = 0;
    }
    let rows = layout(lines_for(app), inner.width as usize);
    let h = inner.height as usize;
    app.detail_scroll = app.detail_scroll.min(rows.len().saturating_sub(h));
    let shown: Vec<Row> = rows.into_iter().skip(app.detail_scroll).take(h).collect();
    for (y, (_, links)) in shown.iter().enumerate() {
        for (col, w, link) in links {
            let x = inner.x + *col as u16;
            if x >= inner.right() {
                continue;
            }
            let w = (*w as u16).min(inner.right() - x);
            app.hits
                .links
                .push((link.clone(), Rect::new(x, inner.y + y as u16, w, 1)));
        }
    }
    let lines: Vec<Line> = shown.into_iter().map(|(l, _)| l).collect();
    f.render_widget(Paragraph::new(lines), inner);
}

pub fn render_side(f: &mut Frame, area: Rect, app: &mut App) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme::p().surface2))
        .title(" Detail ")
        .style(Style::default().bg(Color::Reset));
    render_in(f, area, block, app);
}

pub fn render_modal(f: &mut Frame, area: Rect, app: &mut App) {
    let rect = centered_rect(70, 80, area);
    f.render_widget(Clear, rect);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(theme::p().mauve))
        .title(" Detail - Esc to close ")
        .style(Style::default().bg(Color::Reset));
    render_in(f, rect, block, app);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bd::types::Dependency;

    fn bead(id: &str, parent: Option<&str>) -> Bead {
        Bead {
            id: id.into(),
            title: format!("{id} title"),
            parent: parent.map(String::from),
            ..Default::default()
        }
    }

    /// Every link's rect covers exactly its id on the row.
    fn links_of(rows: &[Row]) -> Vec<(String, Link)> {
        rows.iter()
            .flat_map(|(line, links)| {
                let plain: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
                links.iter().map(move |(c, w, l)| {
                    // The chars in cells c..c+w: a theme icon may be wide.
                    let mut at = 0;
                    let mut shown = String::new();
                    for ch in plain.chars() {
                        if at >= *c && at < c + w {
                            shown.push(ch);
                        }
                        at += cells(ch.encode_utf8(&mut [0; 4]));
                    }
                    (shown, l.clone())
                })
            })
            .collect()
    }

    #[test]
    fn relations_are_bead_links_and_pane_ids_are_pane_links() {
        let mut b = bead("v-1.2", Some("v-1"));
        b.description = "ask w3V:p1 or (io:w1:p2) about it".into();
        b.is_blocked = true;
        b.blocked_by = vec!["v-3".into(), "v-4".into()];
        b.dependencies = vec![Dependency {
            depends_on_id: Some("v-9".into()),
            dep_type: Some("discovered-from".into()),
            ..Default::default()
        }];
        let beads = vec![bead("v-1", None), b.clone(), bead("v-1.2.1", Some("v-1.2"))];
        let rows = layout(build_lines(&b, &beads), 80);
        let mut got = links_of(&rows);
        got.sort_by(|a, b| a.0.cmp(&b.0));
        let bead = |s: &str| (s.to_string(), Link::Bead(s.into()));
        let pane = |s: &str| (s.to_string(), Link::Pane(s.into()));
        assert_eq!(
            got,
            vec![
                pane("io:w1:p2"),
                bead("v-1"),
                bead("v-1.2.1"),
                bead("v-3"),
                bead("v-4"),
                bead("v-9"),
                pane("w3V:p1"),
            ]
        );
    }

    #[test]
    fn wraps_at_words_and_splits_long_ones() {
        assert_eq!(wrap("aa bb cc", 5), vec!["aa bb", "cc"]);
        assert_eq!(wrap("abcdefg", 3), vec!["abc", "def", "g"]);
        assert_eq!(wrap("short", 10), vec!["short"]);
        assert_eq!(wrap("", 10), vec![""]);
    }

    /// A pane id found on a wrapped row points at its place on that row.
    #[test]
    fn pane_ids_on_wrapped_rows() {
        let mut b = bead("v-5", None);
        b.description = "first words then w3V:p1.".into();
        let rows = layout(build_lines(&b, &[]), 12);
        let got: Vec<_> = links_of(&rows);
        assert_eq!(got, vec![("w3V:p1".into(), Link::Pane("w3V:p1".into()))]);
    }
}
