//! Minimal picker for the herdr popup: arrows/j/k + Enter, an item's hotkey
//! picks it directly, a mouse click picks a row, Esc/q cancels. The first
//! item is preselected.

use std::io::{self, Write};

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEventKind};
use crossterm::style::{Attribute, Print, SetAttribute};
use crossterm::{cursor, execute, queue, terminal};

/// Row of the first item; row 0 is the title, row 1 is blank.
const FIRST_ROW: u16 = 2;

/// Hotkeys 1-9 for lists without their own letters (files, apps).
pub fn numbered<'a>(labels: &[&'a str]) -> Vec<(char, &'a str)> {
    labels
        .iter()
        .enumerate()
        .map(|(i, l)| (char::from_digit(i as u32 + 1, 10).unwrap_or(' '), *l))
        .collect()
}

/// Returns the index of the chosen item, or `None` when cancelled. Each
/// item is `(hotkey, label)`; avoid `j`, `k` and `q`, which navigate.
pub fn pick(title: &str, items: &[(char, &str)]) -> io::Result<Option<usize>> {
    let mut out = io::stdout();
    terminal::enable_raw_mode()?;
    execute!(out, cursor::Hide, event::EnableMouseCapture)?;
    let result = run(&mut out, title, items);
    execute!(out, event::DisableMouseCapture, cursor::Show)?;
    terminal::disable_raw_mode()?;
    result
}

/// Show `lines` under `title` until any key or click.
pub fn message(title: &str, lines: &[&str]) -> io::Result<()> {
    let mut out = io::stdout();
    terminal::enable_raw_mode()?;
    execute!(out, cursor::Hide, event::EnableMouseCapture)?;
    queue!(
        out,
        terminal::Clear(terminal::ClearType::All),
        cursor::MoveTo(0, 0)
    )?;
    queue!(
        out,
        SetAttribute(Attribute::Bold),
        Print(title),
        SetAttribute(Attribute::Reset)
    )?;
    let width = terminal::size()
        .map(|(w, _)| usize::from(w).max(10))
        .unwrap_or(60);
    let wrapped: Vec<String> = lines.iter().flat_map(|l| wrap(l, width)).collect();
    for (i, line) in wrapped.iter().enumerate() {
        queue!(out, cursor::MoveTo(0, FIRST_ROW + i as u16), Print(line))?;
    }
    out.flush()?;
    let result = loop {
        match event::read() {
            Ok(Event::Key(_)) => break Ok(()),
            Ok(Event::Mouse(m)) if matches!(m.kind, MouseEventKind::Down(_)) => break Ok(()),
            Ok(_) => {}
            Err(e) => break Err(e),
        }
    };
    execute!(out, event::DisableMouseCapture, cursor::Show)?;
    terminal::disable_raw_mode()?;
    result
}

fn run(out: &mut impl Write, title: &str, items: &[(char, &str)]) -> io::Result<Option<usize>> {
    let mut sel = 0usize;
    loop {
        draw(out, title, items, sel)?;
        match event::read()? {
            Event::Key(KeyEvent {
                code, modifiers, ..
            }) => match code {
                KeyCode::Esc | KeyCode::Char('q') => return Ok(None),
                KeyCode::Char('c') if modifiers.contains(KeyModifiers::CONTROL) => return Ok(None),
                KeyCode::Enter => return Ok(Some(sel)),
                KeyCode::Up | KeyCode::Char('k') => sel = sel.saturating_sub(1),
                KeyCode::Down | KeyCode::Char('j') => sel = (sel + 1).min(items.len() - 1),
                KeyCode::Char(c) => {
                    if let Some(i) = items.iter().position(|(key, _)| *key == c) {
                        return Ok(Some(i));
                    }
                }
                _ => {}
            },
            Event::Mouse(m) if m.kind == MouseEventKind::Down(MouseButton::Left) => {
                if let Some(i) = m.row.checked_sub(FIRST_ROW).map(usize::from) {
                    if i < items.len() {
                        return Ok(Some(i));
                    }
                }
            }
            _ => {}
        }
    }
}

/// Hard-wrap `s` into chunks of at most `width` chars; "" stays one line.
fn wrap(s: &str, width: usize) -> Vec<String> {
    let chars: Vec<char> = s.chars().collect();
    if chars.is_empty() {
        return vec![String::new()];
    }
    chars.chunks(width).map(|c| c.iter().collect()).collect()
}

fn draw(out: &mut impl Write, title: &str, items: &[(char, &str)], sel: usize) -> io::Result<()> {
    queue!(
        out,
        terminal::Clear(terminal::ClearType::All),
        cursor::MoveTo(0, 0)
    )?;
    queue!(
        out,
        SetAttribute(Attribute::Dim),
        Print(title),
        SetAttribute(Attribute::Reset)
    )?;
    for (i, (key, label)) in items.iter().enumerate() {
        queue!(out, cursor::MoveTo(0, FIRST_ROW + i as u16))?;
        if i == sel {
            queue!(out, SetAttribute(Attribute::Reverse))?;
        }
        queue!(
            out,
            Print(format!(" {key}  {label} ")),
            SetAttribute(Attribute::Reset)
        )?;
    }
    out.flush()
}

#[cfg(test)]
mod tests {
    use super::{numbered, wrap};

    #[test]
    fn numbered_assigns_digits_from_one() {
        assert_eq!(numbered(&["a", "b"]), vec![('1', "a"), ('2', "b")]);
    }

    #[test]
    fn wraps_long_lines_and_keeps_empty_ones() {
        assert_eq!(wrap("abcdef", 4), vec!["abcd", "ef"]);
        assert_eq!(wrap("", 4), vec![""]);
    }
}
