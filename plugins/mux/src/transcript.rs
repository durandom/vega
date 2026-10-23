//! `vega mux transcript`: an agent's session log instead of its screen.
//! herdr names the session (`agent_session`); `vega-transcript` reads it.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use vega_herdr::{cell, collapse_home, herdr, machine, print_failure, str_of, Failure};
use vega_transcript::{
    civil, locate, one_line, truncate, utc_offset, Entry, Kind, LocateError, Role,
};

/// Characters per entry in the table; `--full` lifts it.
const TEXT_MAX: usize = 300;

/// Which entries to print.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum View {
    /// The newest `n` entries.
    Tail(usize),
    /// The last assistant reply, untruncated.
    Last,
    /// Entries after reply `n`.
    Since(usize),
    /// Only the counts.
    Count,
}

pub fn run(target: &str, view: View, full: bool, session: Option<&str>) -> ExitCode {
    match load(target, session) {
        Ok((pane, kind, path, entries, by_cwd)) => {
            println!("pane: {pane}");
            println!("kind: {}", kind.as_str());
            println!(
                "session: {}",
                cell(&collapse_home(&path.display().to_string()))
            );
            if by_cwd {
                println!("note: herdr names no session yet; this is the cwd's only log and may be an older session");
            }
            for line in render(&pane, &entries, view, full, utc_offset()) {
                println!("{line}");
            }
            ExitCode::SUCCESS
        }
        Err((f, candidates)) => {
            print_failure(&f);
            if !candidates.is_empty() {
                println!("candidates[{}]{{path}}:", candidates.len());
                for c in &candidates {
                    println!("  {}", cell(&collapse_home(&c.display().to_string())));
                }
            }
            ExitCode::from(if f.code == "usage" { 2 } else { 1 })
        }
    }
}

/// Pane, kind, log path, entries, and whether the log was picked by cwd.
type Loaded = (String, Kind, PathBuf, Vec<Entry>, bool);

fn load(target: &str, session: Option<&str>) -> Result<Loaded, (Failure, Vec<PathBuf>)> {
    let plain = |f| (f, Vec::new());
    if let Some(m) = machine() {
        return Err(plain(Failure::new(
            "unsupported",
            format!("transcript reads session files on this machine only, not on {m}"),
            "run `vega mux transcript` on that machine, e.g. over ssh",
        )));
    }
    let a = herdr(&["agent", "get", target]).map_err(plain)?["result"]["agent"].clone();
    let pane = str_of(&a, "pane_id").to_string();
    let kind_name = str_of(&a, "agent");
    let kind = Kind::from_herdr(kind_name).ok_or_else(|| {
        plain(Failure::new(
            "unsupported_kind",
            format!("no session log reader for agent kind {kind_name}"),
            "transcript reads claude, codex, pi and agy; use `vega mux show` for others",
        ))
    })?;
    let session = session.or(a["agent_session"]["value"].as_str());
    let home = std::env::var("HOME").unwrap_or_default();
    let path = locate(kind, session, str_of(&a, "cwd"), Path::new(&home)).map_err(|e| match e {
        LocateError::NotFound { looked } => plain(Failure::new(
            "session_not_found",
            format!(
                "no {} session log for {pane}; looked in {}",
                kind.as_str(),
                looked
                    .iter()
                    .map(|p| collapse_home(&p.display().to_string()))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            "a fresh agent writes its log on the first prompt; or pass --session <id|path>",
        )),
        LocateError::Ambiguous { candidates } => (
            Failure::new(
                "session_ambiguous",
                format!(
                    "herdr names no session for {pane} and its cwd has {} {} logs",
                    candidates.len(),
                    kind.as_str()
                ),
                "pass one of the candidates (newest first) with --session <path>",
            ),
            candidates,
        ),
    })?;
    let entries = vega_transcript::read(kind, &path).map_err(|e| {
        plain(Failure::new(
            "session_unreadable",
            format!(
                "cannot read {}: {e}",
                collapse_home(&path.display().to_string())
            ),
            "check the file's permissions",
        ))
    })?;
    Ok((pane, kind, path, entries, session.is_none()))
}

/// `MM-DD HH:MM` local time, `-` without a timestamp.
fn when(t: Option<i64>, offset: i64) -> String {
    t.map_or("-".into(), |t| {
        let (_, mo, d, h, mi) = civil(t, offset);
        format!("{mo:02}-{d:02} {h:02}:{mi:02}")
    })
}

/// The output after the `pane`/`kind`/`session` lines. Assistant entries are
/// numbered (`reply`) so `--since <reply>` picks up later ones.
pub fn render(pane: &str, entries: &[Entry], view: View, full: bool, offset: i64) -> Vec<String> {
    let mut numbered = Vec::with_capacity(entries.len());
    let mut replies = 0;
    for e in entries {
        let n = (e.role == Role::Assistant).then(|| {
            replies += 1;
            replies
        });
        numbered.push((n, e));
    }
    let mut out = vec![format!("replies: {replies}")];
    let rows: &[(Option<usize>, &Entry)] = match view {
        View::Count => {
            out.push(format!("entries: {}", entries.len()));
            return out;
        }
        View::Last => {
            match numbered.iter().rev().find(|(n, _)| n.is_some()) {
                Some((n, e)) => {
                    out.push(format!("reply: {}", n.unwrap_or(0)));
                    out.push(format!("time: {}", when(e.time, offset)));
                    out.push("text:".into());
                    out.extend(e.text.lines().map(|l| format!("  {l}")));
                }
                None => out.push("reply: none yet".into()),
            }
            return out;
        }
        View::Since(n) => {
            // Everything after reply n: the tool calls and prompts that
            // follow it too, not only later replies.
            let after = match n {
                0 => 0,
                n => numbered
                    .iter()
                    .position(|(r, _)| *r == Some(n))
                    .map_or(numbered.len(), |i| i + 1),
            };
            &numbered[after..]
        }
        View::Tail(k) => &numbered[numbered.len().saturating_sub(k)..],
    };
    let mut cut = false;
    if rows.is_empty() {
        out.push("entries: 0 new".into());
    } else {
        let header = if rows.len() == entries.len() {
            format!("entries[{}]", rows.len())
        } else {
            format!("entries[{} of {}]", rows.len(), entries.len())
        };
        out.push(format!("{header}{{reply,role,time,text}}:"));
        for (n, e) in rows {
            let flat = one_line(&e.text);
            let text = if full || e.role == Role::Tool {
                flat
            } else {
                let t = truncate(&flat, TEXT_MAX);
                cut |= t != flat;
                t
            };
            out.push(format!(
                "  {},{},{},{}",
                n.map_or("-".into(), |n| n.to_string()),
                e.role.as_str(),
                when(e.time, offset),
                cell(&text)
            ));
        }
    }
    let mut help = Vec::new();
    if cut {
        help.push(format!(
            "vega mux transcript {pane} --full   # untruncated text"
        ));
    }
    if replies > 0 {
        help.push(format!(
            "vega mux transcript {pane} --since {replies}   # only what comes after reply {replies}"
        ));
        help.push(format!(
            "vega mux transcript {pane} --last   # last reply, full text"
        ));
    }
    if !help.is_empty() {
        out.push(format!("help[{}]:", help.len()));
        out.extend(help.into_iter().map(|h| format!("  {h}")));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(role: Role, text: &str) -> Entry {
        Entry {
            role,
            time: Some(0),
            text: text.into(),
        }
    }

    fn sample() -> Vec<Entry> {
        vec![
            e(Role::User, "go"),
            e(Role::Assistant, "first"),
            e(Role::Tool, "Bash: ls"),
            e(Role::Assistant, "second\nline two"),
            e(Role::Error, "429"),
        ]
    }

    #[test]
    fn count_and_last() {
        assert_eq!(
            render("p", &sample(), View::Count, false, 0),
            ["replies: 2", "entries: 5"]
        );
        assert_eq!(
            render("p", &sample(), View::Last, false, 7200),
            [
                "replies: 2",
                "reply: 2",
                "time: 01-01 02:00",
                "text:",
                "  second",
                "  line two"
            ]
        );
        assert_eq!(
            render("p", &[e(Role::User, "hi")], View::Last, false, 0),
            ["replies: 0", "reply: none yet"]
        );
    }

    #[test]
    fn since_starts_right_after_the_reply() {
        let out = render("p", &sample(), View::Since(1), false, 0);
        assert_eq!(out[1], "entries[3 of 5]{reply,role,time,text}:");
        assert_eq!(out[2], "  -,tool,01-01 00:00,Bash: ls");
        assert_eq!(out[3], "  2,assistant,01-01 00:00,second line two");
        assert_eq!(out[4], "  -,error,01-01 00:00,429");
        assert_eq!(
            render("p", &sample(), View::Since(2), false, 0)[1],
            "entries[1 of 5]{reply,role,time,text}:"
        );
        assert_eq!(
            render("p", &sample(), View::Since(0), false, 0)[1],
            "entries[5]{reply,role,time,text}:"
        );
        assert_eq!(
            render("p", &sample(), View::Since(9), false, 0)[1],
            "entries: 0 new"
        );
    }

    #[test]
    fn tail_truncates_unless_full() {
        let long = vec![e(Role::Assistant, &"x".repeat(TEXT_MAX + 50))];
        let out = render("p", &long, View::Tail(20), false, 0);
        assert_eq!(out[1], "entries[1]{reply,role,time,text}:");
        assert!(out[2].ends_with('…'));
        assert!(out.iter().any(|l| l.contains("--full")));
        let out = render("p", &long, View::Tail(20), true, 0);
        assert!(!out[2].ends_with('…'));
        assert!(!out.iter().any(|l| l.contains("--full")));
        assert_eq!(
            render("p", &sample(), View::Tail(2), false, 0)[1],
            "entries[2 of 5]{reply,role,time,text}:"
        );
    }

    #[test]
    fn no_timestamp_is_a_dash() {
        assert_eq!(when(None, 0), "-");
    }
}
