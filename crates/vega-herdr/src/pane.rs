//! A herdr pane id in a selection (`w1:p2`, `io:w1:p2`, `w1:p2 on io`), the
//! machine it lives on, and jumping to it. Shared by vega-open and the
//! board.
//!
//! herdr focuses a pane by id only on its own server (`pane.focus`). The
//! client switches machines only on user input (sidebar, notification), so a
//! pane on another machine is focused there and the user switches to that
//! machine themselves (upstream: vega-8m7).

use std::env;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::Duration;

use serde_json::{json, Value};

/// Characters that wrap a path in prose, markdown or compiler output.
const WRAPPERS: &[(char, char)] = &[
    ('`', '`'),
    ('\'', '\''),
    ('"', '"'),
    ('(', ')'),
    ('[', ']'),
    ('<', '>'),
];

/// Peel wrappers and trailing punctuation repeatedly: "(`src/a.rs:3`)." ->
/// "src/a.rs:3".
pub fn peel(s: &str) -> &str {
    let mut s = s.trim();
    loop {
        let before = s;
        s = s.trim_end_matches(['.', ',', ';', '!', '?']);
        for (open, close) in WRAPPERS {
            if let Some(inner) = s.strip_prefix(*open).and_then(|r| r.strip_suffix(*close)) {
                s = inner.trim();
            }
        }
        if s == before {
            return s;
        }
    }
}

/// Bound on one herdr call; another machine answers over ssh.
const HERDR_TIMEOUT: Duration = Duration::from_secs(5);

/// A pane id as written on screen, not yet looked up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaneRef {
    pub pane: String,
    pub machine: Option<String>,
    /// The machine came from prose (`w1:p2 on io`); an unknown one is
    /// ignored, so "on this machine" does not fail the lookup.
    pub loose: bool,
}

/// Where a pane was found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Hit {
    Local(String),
    Remote { machine: String, pane: String },
}

impl Hit {
    /// `w1:p2` here, `io:w1:p2` elsewhere: the form vega prints.
    pub fn display(&self) -> String {
        match self {
            Hit::Local(p) => p.clone(),
            Hit::Remote { machine, pane } => crate::qualify(Some(machine), pane),
        }
    }
}

/// A failed lookup: `(code, message)`.
pub type Miss = (&'static str, String);

/// `w<id>:p<id>`, e.g. `w3V:p1`, `w1C:pB`.
pub fn is_pane_id(s: &str) -> bool {
    let Some((w, p)) = s.split_once(':') else {
        return false;
    };
    let w = w.strip_prefix('w').unwrap_or("");
    let p = p.strip_prefix('p').unwrap_or("");
    let id = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric());
    id(w) && id(p)
}

/// A herdr machine label as `herdr machine list` saves it.
fn is_label(s: &str) -> bool {
    !s.is_empty()
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
}

/// The first pane id in `text`, bare or machine-qualified; `w1:p2 on io`
/// takes the machine from the next words.
pub fn parse(text: &str) -> Option<PaneRef> {
    let words: Vec<&str> = text.split_whitespace().map(peel).collect();
    words.iter().enumerate().find_map(|(i, w)| {
        if is_pane_id(w) {
            let machine = (words.get(i + 1) == Some(&"on"))
                .then(|| words.get(i + 2).copied())
                .flatten()
                .filter(|m| is_label(m));
            return Some(PaneRef {
                pane: w.to_string(),
                loose: machine.is_some(),
                machine: machine.map(String::from),
            });
        }
        let (m, pane) = w.split_once(':')?;
        (is_label(m) && is_pane_id(pane)).then(|| PaneRef {
            pane: pane.to_string(),
            machine: Some(m.to_string()),
            loose: false,
        })
    })
}

/// Look `r` up: a machine named as `here` (this host) is this machine; any
/// other must be a known herdr machine. A bare id is looked up here first,
/// then on every known machine. `exists(None, pane)` checks this machine.
pub fn resolve(
    r: &PaneRef,
    here: &str,
    machines: &[String],
    exists: impl Fn(Option<&str>, &str) -> bool + Sync,
) -> Result<Vec<Hit>, Miss> {
    let not_found = |place: String| ("pane_not_found", format!("no pane {} {place}", r.pane));
    match r.machine.as_deref() {
        Some(m) if m.eq_ignore_ascii_case(here) => {
            return if exists(None, &r.pane) {
                Ok(vec![Hit::Local(r.pane.clone())])
            } else {
                Err(not_found("on this machine".into()))
            };
        }
        Some(m) if machines.iter().any(|k| k == m) => {
            return if exists(Some(m), &r.pane) {
                Ok(vec![Hit::Remote {
                    machine: m.to_string(),
                    pane: r.pane.clone(),
                }])
            } else {
                Err(not_found(format!("on {m}")))
            };
        }
        Some(m) if !r.loose => {
            return Err((
                "unknown_machine",
                format!("{m} is neither this machine ({here}) nor a herdr machine"),
            ))
        }
        _ => {}
    }
    if exists(None, &r.pane) {
        return Ok(vec![Hit::Local(r.pane.clone())]);
    }
    // Other machines answer over ssh: ask them all at once.
    let hits: Vec<Hit> = std::thread::scope(|s| {
        let asks: Vec<_> = machines
            .iter()
            .map(|m| s.spawn(|| exists(Some(m), &r.pane).then(|| m.clone())))
            .collect();
        asks.into_iter()
            .filter_map(|h| h.join().ok().flatten())
            .map(|machine| Hit::Remote {
                machine,
                pane: r.pane.clone(),
            })
            .collect()
    });
    if hits.is_empty() {
        let place = if machines.is_empty() {
            "on this machine".to_string()
        } else {
            format!("here or on {}", machines.join(", "))
        };
        return Err(not_found(place));
    }
    Ok(hits)
}

/// Enabled herdr machines, by label.
pub fn machines() -> Vec<String> {
    crate::herdr_on(None, &["machine", "list", "--json"], HERDR_TIMEOUT)
        .ok()
        .and_then(|v| v.as_array().cloned())
        .unwrap_or_default()
        .iter()
        .filter(|m| m["enabled"] != false)
        .filter_map(|m| m["label"].as_str().map(String::from))
        .collect()
}

/// Whether herdr on `machine` (`None`: this one) knows the pane.
pub fn exists(machine: Option<&str>, pane: &str) -> bool {
    crate::herdr_on(machine, &["pane", "get", pane], HERDR_TIMEOUT).is_ok()
}

/// Focus the pane. On another machine herdr focuses it there; the user's
/// client stays on its machine.
pub fn focus(hit: &Hit) -> Result<(), String> {
    match hit {
        Hit::Local(pane) => focus_local(pane),
        Hit::Remote { machine, pane } => focus_remote(machine, pane),
    }
}

/// `pane.focus` over the socket: herdr's CLI only focuses agents
/// (`agent focus`) or neighbours (`pane focus --direction`).
fn focus_local(pane: &str) -> Result<(), String> {
    let path = socket_path().ok_or("no herdr socket (HOME unset)")?;
    let mut conn =
        UnixStream::connect(&path).map_err(|e| format!("connect {}: {e}", path.display()))?;
    conn.set_read_timeout(Some(HERDR_TIMEOUT))
        .map_err(|e| e.to_string())?;
    let req = json!({"id": "vega-open", "method": "pane.focus", "params": {"pane_id": pane}});
    conn.write_all(format!("{req}\n").as_bytes())
        .map_err(|e| format!("write: {e}"))?;
    let mut line = String::new();
    BufReader::new(conn)
        .read_line(&mut line)
        .map_err(|e| format!("read: {e}"))?;
    let v: Value = serde_json::from_str(&line).map_err(|_| "herdr sent no JSON reply")?;
    match v.get("error") {
        Some(err) => Err(err["message"].as_str().unwrap_or("refused").to_string()),
        None => Ok(()),
    }
}

/// `agent focus` reaches the pane itself; a pane without an agent gets its
/// tab focused instead (the remote CLI has no pane focus by id).
fn focus_remote(machine: &str, pane: &str) -> Result<(), String> {
    let on = Some(machine);
    if crate::herdr_on(on, &["agent", "focus", pane], HERDR_TIMEOUT).is_ok() {
        return Ok(());
    }
    let v = crate::herdr_on(on, &["pane", "get", pane], HERDR_TIMEOUT).map_err(|f| f.message)?;
    let tab = v["result"]["pane"]["tab_id"]
        .as_str()
        .ok_or_else(|| format!("{machine}: pane {pane} has no tab"))?;
    crate::herdr_on(on, &["tab", "focus", tab], HERDR_TIMEOUT)
        .map(|_| ())
        .map_err(|f| f.message)
}

/// `$HERDR_SOCKET_PATH`, else herdr's default under the config dir.
fn socket_path() -> Option<PathBuf> {
    if let Some(p) = env::var_os("HERDR_SOCKET_PATH").filter(|p| !p.is_empty()) {
        return Some(PathBuf::from(p));
    }
    env::var_os("XDG_CONFIG_HOME")
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .map(|c| c.join("herdr/herdr.sock"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(pane: &str, machine: Option<&str>, loose: bool) -> Option<PaneRef> {
        Some(PaneRef {
            pane: pane.into(),
            machine: machine.map(Into::into),
            loose,
        })
    }

    #[test]
    fn recognizes_pane_ids() {
        for ok in ["w1:p2", "w3V:p1", "wA2:p10", "w1C:pB"] {
            assert!(is_pane_id(ok), "{ok}");
        }
        for bad in [
            "w:p1",
            "w1:p",
            "w1:t1",
            "x1:p2",
            "w1:p-2",
            "w-1:p2",
            "src/a.rs:12",
        ] {
            assert!(!is_pane_id(bad), "{bad}");
        }
    }

    #[test]
    fn parses_bare_qualified_and_prose_forms() {
        assert_eq!(parse("w3V:p1"), r("w3V:p1", None, false));
        assert_eq!(parse("`io:w1:p2`."), r("w1:p2", Some("io"), false));
        assert_eq!(
            parse("confirm it in pane io-wsl:w1:p2, then"),
            r("w1:p2", Some("io-wsl"), false)
        );
        assert_eq!(parse("w1:p2 on io"), r("w1:p2", Some("io"), true));
        assert_eq!(parse("(w1:p2) on io."), r("w1:p2", Some("io"), true));
        assert_eq!(parse("src/main.rs:12"), None);
        assert_eq!(parse("io:w1"), None);
        assert_eq!(parse(""), None);
    }

    fn machines() -> Vec<String> {
        vec!["io".into(), "deimos".into()]
    }

    /// Panes: w1:p1 here; w1:p2 on io and deimos; w2:p1 on io.
    fn fake(machine: Option<&str>, pane: &str) -> bool {
        matches!(
            (machine, pane),
            (None, "w1:p1") | (Some("io" | "deimos"), "w1:p2") | (Some("io"), "w2:p1")
        )
    }

    fn local(p: &str) -> Hit {
        Hit::Local(p.into())
    }

    fn remote(m: &str, p: &str) -> Hit {
        Hit::Remote {
            machine: m.into(),
            pane: p.into(),
        }
    }

    #[test]
    fn bare_ids_prefer_this_machine_then_ask_all() {
        let res = |t: &str| resolve(&parse(t).unwrap(), "proteus", &machines(), fake);
        assert_eq!(res("w1:p1"), Ok(vec![local("w1:p1")]));
        assert_eq!(res("w2:p1"), Ok(vec![remote("io", "w2:p1")]));
        assert_eq!(
            res("w1:p2"),
            Ok(vec![remote("io", "w1:p2"), remote("deimos", "w1:p2")])
        );
        let (code, msg) = res("w9:p9").unwrap_err();
        assert_eq!(code, "pane_not_found");
        assert!(msg.contains("here or on io, deimos"), "{msg}");
    }

    #[test]
    fn qualified_ids_go_to_their_machine_only() {
        let res = |t: &str| resolve(&parse(t).unwrap(), "proteus", &machines(), fake);
        assert_eq!(res("io:w1:p2"), Ok(vec![remote("io", "w1:p2")]));
        assert_eq!(res("proteus:w1:p1"), Ok(vec![local("w1:p1")]));
        assert_eq!(res("w2:p1 on io"), Ok(vec![remote("io", "w2:p1")]));
        assert_eq!(res("deimos:w2:p1").unwrap_err().0, "pane_not_found");
        assert_eq!(res("titan:w1:p1").unwrap_err().0, "unknown_machine");
        // Prose: "on this machine" is no machine label.
        assert_eq!(res("w1:p1 on this machine"), Ok(vec![local("w1:p1")]));
    }

    #[test]
    fn hits_display_machine_qualified_elsewhere() {
        assert_eq!(local("w1:p1").display(), "w1:p1");
        assert_eq!(remote("io", "w1:p2").display(), "io:w1:p2");
    }
}
