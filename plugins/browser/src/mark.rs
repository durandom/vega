//! Session colours and the window mark (design, phase 2): every named
//! session gets a colour (its owner project's `[symbols.project_colors]`
//! colour, else a stable hash of the session name), and headed calls carry an
//! init script that colours the browser window's pages — a fixed frame, a
//! session tag and a `[session]` title prefix, so the window on screen can
//! be matched to its panel row. Fresh headless sessions have no mark; they show
//! their colour in the panels only, and `launch` (bot blockers) never marks.
//!
//! Verified on 0.43.0 (vega-nic.22): init scripts persist across calls and
//! cover tab new's first navigation. Repeating a script path is deduplicated.
//! Scripts run before document.documentElement exists; defer DOM work and
//! retain the page guard. Only headed calls register the mark, but omitting
//! the flag later does not remove a registration. Opt out before the first
//! marked call; use a fresh session for unmarked work. Never eval a shared tab.
//!
//! Review (Slayer, 2026-11-17): the mark changes what callers see — title
//! asserts, `snapshot` text, screenshot pixels, injected DOM on bot-wary
//! sites — so it rides only explicitly headed calls (`--headed` flag or a
//! truthy `AGENT_BROWSER_HEADED`), is invisible to the accessibility tree
//! (`aria-hidden`), and `VEGA_BROWSER_MARK=0` prevents new registrations.

use std::env;
use std::fs;
use std::path::PathBuf;

use sha2::{Digest, Sha256};
use vega_herdr::theme;

/// The fallback palette: ten hues that read on light and dark panels, in
/// the families of vega's default symbol colours.
pub const PALETTE: [&str; 10] = [
    "#d9465f", "#0e8f6f", "#3d73cc", "#e0a020", "#9d6bf0", "#1f8fb0", "#c96442", "#b558a6",
    "#7a8ce8", "#6f8f2f",
];

/// The colour of `session`: its owner project's `[symbols.project_colors]`
/// colour when the theme has one, else a stable FNV-1a hash of the session
/// name into the palette. Lowercase `#rrggbb`.
pub fn session_colour(session: &str, project: Option<&str>) -> String {
    if let Some(c) = project.and_then(|p| theme::symbols().project_color(p)) {
        return c.hex();
    }
    let mut hash: u32 = 2166136261;
    for byte in session.bytes() {
        hash ^= u32::from(byte);
        hash = hash.wrapping_mul(16777619);
    }
    PALETTE[(hash as usize) % PALETTE.len()].to_string()
}

/// The marks directory `$XDG_STATE_HOME/vega/browser/marks`.
fn marks_dir() -> Option<PathBuf> {
    Some(
        env::var_os("XDG_STATE_HOME")
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
            .or_else(|| env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/state")))?
            .join("vega")
            .join("browser")
            .join("marks"),
    )
}

/// A session name that is safe as one file name (no traversal). Session
/// names are free-form; one that would not be a safe file name keeps its
/// colour in `ls` but gets no window mark.
fn safe_name(session: &str) -> Option<&str> {
    (!session.is_empty()
        && session.len() <= 100
        && !session.starts_with('.')
        && session
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-')))
    .then_some(session)
}

/// The init script of `session` in `colour`, as verified: DOMContentLoaded
/// only, a dataset guard against duplicate registrations, a title observer
/// for SPA renames.
fn script(session: &str, colour: &str) -> String {
    let session = serde_json::Value::String(session.to_owned());
    // inline-md: allow (generated page JS with format! interpolation, not a guide)
    format!(
        r#"(() => {{
  const name = {session}, colour = "{colour}";
  const mark = () => {{
    const root = document.documentElement;
    if (!root || root.dataset.vegaMark === name) return;
    root.dataset.vegaMark = name;
    const frame = document.createElement("div");
    frame.setAttribute("data-vega-session", name);
    frame.setAttribute("aria-hidden", "true");
    frame.style.cssText = "position:fixed;inset:0;border:6px solid " + colour + ";box-sizing:border-box;pointer-events:none;z-index:2147483647;";
    const tag = document.createElement("div");
    tag.textContent = name;
    tag.setAttribute("aria-hidden", "true");
    tag.style.cssText = "position:fixed;top:2px;left:50%;transform:translateX(-50%);background:" + colour + ";color:#fff;font:600 12px ui-sans-serif;padding:1px 8px;border-radius:0 0 8px 8px;z-index:2147483647;pointer-events:none;";
    (document.body || root).append(frame, tag);
    const prefix = () => {{
      const base = document.title.replace(/^\[[^\]]*\]\s*/, "");
      const wanted = "[" + name + "] " + base;
      if (document.title !== wanted) document.title = wanted;
    }};
    prefix();
    const title = document.querySelector("head > title");
    if (title) new MutationObserver(prefix).observe(title, {{childList: true, characterData: true, subtree: true}});
  }};
  if (document.readyState === "loading") document.addEventListener("DOMContentLoaded", mark);
  else mark();
}})();
"#
    )
}

/// Writes the mark script unless it already has this content, and returns
/// its path. Best effort like the owner records: None (no flag) when the
/// name is unsafe or the file cannot be written.
pub fn ensure_mark(session: &str, colour: &str) -> Option<PathBuf> {
    let name = safe_name(session)?;
    write_mark(name, session, colour)
}

pub(crate) fn dev_mark(session: &str, project: &str, label: &str) -> Option<PathBuf> {
    let digest = Sha256::digest(format!("{project}\0{session}").as_bytes());
    let colour = session_colour(session, Some(label));
    write_mark(
        &format!("dev-{digest:x}"),
        &format!("{label} / {session}"),
        &colour,
    )
}

fn write_mark(name: &str, label: &str, colour: &str) -> Option<PathBuf> {
    let dir = marks_dir()?;
    let path = dir.join(format!("{name}.js"));
    let text = script(label, colour);
    if fs::read_to_string(&path).ok().as_deref() != Some(text.as_str()) {
        use std::io::Write;
        fs::create_dir_all(&dir).ok()?;
        let mut tmp = tempfile::NamedTempFile::new_in(&dir).ok()?;
        tmp.write_all(text.as_bytes()).ok()?;
        tmp.persist(&path).ok()?;
    }
    Some(path)
}

/// The mark of `session` from its owner record's project, if any. Reads the
/// theme at most every 2 s (`theme::symbols`), so this stays cheap per call.
pub fn mark_for(session: &str, project: Option<&str>) -> Option<PathBuf> {
    let colour = session_colour(session, project);
    ensure_mark(session, &colour)
}

/// Whether `name`'s call args ask for a headed browser: an explicit
/// `--headed` flag, or agent-browser's `AGENT_BROWSER_HEADED` env set to
/// anything but `0`/`false`/empty.
pub fn headed(args: &[String]) -> bool {
    if let Some(index) = args.iter().rposition(|a| a == "--headed") {
        return args.get(index + 1).is_none_or(|value| value != "false");
    }
    std::env::var("AGENT_BROWSER_HEADED").is_ok_and(|value| {
        !matches!(
            value.to_ascii_lowercase().as_str(),
            "" | "0" | "false" | "no"
        )
    })
}

/// Whether this call registers a mark: headed and not opted out.
/// An existing upstream registration persists even when this returns false.
pub fn wanted(args: &[String]) -> bool {
    if std::env::var("VEGA_BROWSER_MARK").ok().as_deref() == Some("0") {
        return false;
    }
    headed(args)
}

/// `args` with the mark flag in front: agent-browser takes its options
/// before the command. In 0.43 registrations persist across calls.
/// Calls that are not headed (or opted out) pass through
/// unchanged; sessions without a mark file pass through unchanged too.
pub fn with_mark(args: &[String], session: &str, project: Option<&str>) -> Vec<String> {
    if !wanted(args) {
        return args.to_vec();
    }
    let Some(mark) = mark_for(session, project) else {
        return args.to_vec();
    };
    let mut marked = Vec::with_capacity(args.len() + 2);
    marked.push("--init-script".to_owned());
    marked.push(mark.display().to_string());
    marked.extend_from_slice(args);
    marked
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colours_are_stable_and_hashed_into_the_palette() {
        assert_eq!(
            session_colour("classic-live", None),
            session_colour("classic-live", None)
        );
        assert!(PALETTE.contains(&session_colour("classic-live", None).as_str()));
        assert_ne!(
            session_colour("classic-live", None),
            session_colour("classic-p2", None)
        );
        // Distinct names spread over the palette, not one bucket.
        let colours: std::collections::BTreeSet<_> = (0..40)
            .map(|i| session_colour(&format!("session-{i}"), None))
            .collect();
        assert!(colours.len() >= 6, "{colours:?}");
    }

    #[test]
    fn marks_are_written_once_and_only_for_safe_names() {
        let tmp = tempfile::tempdir().unwrap();
        env::set_var("XDG_STATE_HOME", tmp.path().join("state"));
        let path = mark_for("vega-shim-test", Some("vega")).unwrap();
        assert!(path.ends_with("vega-shim-test.js"));
        let first = fs::read_to_string(&path).unwrap();
        mark_for("vega-shim-test", Some("vega"));
        assert_eq!(fs::read_to_string(&path).unwrap(), first);
        // A different colour rewrites; the script carries both facts.
        let rewritten = ensure_mark("vega-shim-test", "#0e8f6f").unwrap();
        assert_eq!(rewritten, path);
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains(r##"name = "vega-shim-test""##));
        assert!(text.contains(r##"colour = "#0e8f6f""##));
        assert!(text.contains("DOMContentLoaded"));
        assert!(text.contains("vegaMark"));
        assert!(text.contains("MutationObserver"));
        assert_eq!(
            text.matches("aria-hidden").count(),
            2,
            "frame and tag hide from the a11y tree"
        );
        // Unsafe names get no mark file.
        assert!(mark_for("../escape", None).is_none());
        assert!(mark_for("", None).is_none());
        assert!(mark_for(".hidden", None).is_none());
        env::remove_var("XDG_STATE_HOME");
    }

    #[test]
    fn with_mark_prepends_the_flag_or_passes_through() {
        let tmp = tempfile::tempdir().unwrap();
        env::set_var("XDG_STATE_HOME", tmp.path().join("state"));
        env::remove_var("AGENT_BROWSER_HEADED");
        env::remove_var("VEGA_BROWSER_MARK");
        let args = vec!["open".to_owned(), "https://example.com".to_owned()];
        // Headless calls (no --headed, no env) stay unchanged.
        assert_eq!(with_mark(&args, "vega-shim-test", None), args);
        // An explicit --headed flag marks.
        let headed = ["--headed".to_owned()]
            .iter()
            .chain(args.iter())
            .cloned()
            .collect::<Vec<_>>();
        let marked = with_mark(&headed, "vega-shim-test", None);
        assert_eq!(marked[0], "--init-script");
        assert!(marked[1].ends_with("vega-shim-test.js"));
        assert_eq!(&marked[3..], &args[..]);
        // So does a truthy AGENT_BROWSER_HEADED on a flagless call.
        env::set_var("AGENT_BROWSER_HEADED", "1");
        let marked = with_mark(&args, "vega-shim-test", None);
        assert_eq!(marked[0], "--init-script");
        assert_eq!(&marked[2..], &args[..]);
        // Falsy env values do not.
        for v in ["", "0", "false"] {
            env::set_var("AGENT_BROWSER_HEADED", v);
            assert_eq!(with_mark(&args, "vega-shim-test", None), args, "{v:?}");
        }
        env::remove_var("AGENT_BROWSER_HEADED");
        // VEGA_BROWSER_MARK=0 opts out even of a headed call.
        env::set_var("VEGA_BROWSER_MARK", "0");
        assert_eq!(with_mark(&headed, "vega-shim-test", None), headed);
        env::remove_var("VEGA_BROWSER_MARK");
        // An unsafe session name never gets a mark, headed or not.
        assert_eq!(with_mark(&headed, "../escape", None), headed);
        env::remove_var("XDG_STATE_HOME");
    }
}
