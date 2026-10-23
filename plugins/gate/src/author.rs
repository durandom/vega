//! Who wrote the change: kinds and models a reviewer must not have
//! (gate.md section 4, step 1).

use std::collections::BTreeSet;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::time::Duration;

use serde_json::Value;

use crate::git::git;
use crate::segments::{Kind, Segment};
use crate::{run_ok, tool};

#[derive(Debug, Default, Clone, PartialEq)]
pub struct Author {
    pub kinds: BTreeSet<String>,
    pub models: BTreeSet<String>,
    /// A missing model in any evidence excludes that entire harness.
    pub uncertain_kinds: BTreeSet<String>,
}

impl Author {
    pub fn add(&mut self, kind: Option<String>, model: Option<String>) {
        let kind = kind.filter(|k| !k.is_empty());
        let model = model.filter(|m| !m.is_empty());
        if model.is_none() {
            self.uncertain_kinds.extend(kind.clone());
        }
        self.kinds.extend(kind);
        self.models.extend(model);
    }

    pub fn excludes(&self, kind: &str, model: &str) -> bool {
        self.uncertain_kinds.contains(kind) || self.models.contains(model)
    }

    pub fn text(&self) -> String {
        let j = |s: &BTreeSet<String>| s.iter().cloned().collect::<Vec<_>>().join(",");
        format!("kinds {} models {}", j(&self.kinds), j(&self.models))
    }
}

/// The pushing pane's kind and model: herdr's view of the pane, its model
/// from the agent's session log. Ok(None) outside herdr.
pub fn from_pane() -> Option<(String, Option<String>)> {
    std::env::var("HERDR_PANE_ID")
        .ok()
        .filter(|p| !p.is_empty())?;
    let out = run_ok(
        &tool("herdr"),
        &["pane", "current", "--current"],
        None,
        Some(Duration::from_secs(5)),
    )
    .ok()?;
    let v: Value = serde_json::from_str(&out).ok()?;
    let pane = v.pointer("/result/pane")?;
    let kind = pane.get("agent").and_then(Value::as_str)?.to_string();
    let session = pane.pointer("/agent_session/value").and_then(Value::as_str);
    let cwd = pane.get("cwd").and_then(Value::as_str).unwrap_or("");
    let model = vega_transcript::Kind::from_herdr(&kind).and_then(|k| {
        let home = std::env::var_os("HOME")?;
        let log = vega_transcript::locate(k, session, cwd, Path::new(&home)).ok()?;
        last_model(&log)
    });
    Some((kind, model))
}

/// The last `"model":"<id>"` in the tail of a session log (Claude replies and
/// codex turn contexts carry one); synthetic `<...>` values are skipped.
pub fn last_model(log: &Path) -> Option<String> {
    const TAIL: u64 = 512 * 1024;
    let mut f = File::open(log).ok()?;
    let len = f.metadata().ok()?.len();
    f.seek(SeekFrom::Start(len.saturating_sub(TAIL))).ok()?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf).ok()?;
    let text = String::from_utf8_lossy(&buf);
    let mut last = None;
    for (i, _) in text.match_indices("\"model\":") {
        let rest = text[i + 8..].trim_start();
        let Some(rest) = rest.strip_prefix('"') else {
            continue;
        };
        if let Some(end) = rest.find('"') {
            let m = &rest[..end];
            if !m.is_empty() && !m.starts_with('<') {
                last = Some(m.to_string());
            }
        }
    }
    last
}

/// Kind and model of an `Assisted-by` value: `claude:claude-opus-5-5`,
/// `Claude Code (claude-opus-5-5)`, `codex`, `pi:xai/grok-4.3`.
pub fn parse_trailer(v: &str) -> (Option<String>, Option<String>) {
    let v = v.trim();
    let (harness, model) = if let Some((h, m)) = v.split_once(':') {
        (h, Some(m.trim()))
    } else if let Some((h, m)) = v.split_once('(') {
        (h, Some(m.trim_end_matches(')').trim()))
    } else {
        (v, None)
    };
    let h = harness.trim().to_ascii_lowercase();
    let kind = if h.contains("claude") {
        Some("claude")
    } else if h.contains("codex") {
        Some("codex")
    } else if h == "pi" || h.starts_with("pi ") {
        Some("pi")
    } else if h.contains("agy") || h.contains("antigravity") {
        Some("agy")
    } else {
        None
    };
    // pi models carry their provider: `xai/grok-4.3`.
    let model = model
        .filter(|m| !m.is_empty() && !m.contains(' ') && !m.contains('<'))
        .map(|m| m.rsplit('/').next().unwrap_or(m).to_string());
    (kind.map(String::from), model)
}

/// Kinds and models from the `Assisted-by` trailers of the segments' commits.
pub fn from_trailers(dir: &Path, segments: &[Segment]) -> Result<Author, String> {
    let mut a = Author::default();
    for s in segments {
        let range = match &s.kind {
            Kind::Merge { merge } => format!("{merge}^!"),
            _ => format!("{}..{}", s.from, s.to),
        };
        let out = git(
            dir,
            &[
                "log",
                "--format=%(trailers:key=Assisted-by,valueonly,separator=%x0a)",
                &range,
            ],
        )?;
        for line in out.lines().filter(|l| !l.trim().is_empty()) {
            let (k, m) = parse_trailer(line);
            a.add(k, m);
        }
    }
    Ok(a)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trailers_in_the_shapes_vega_history_has() {
        let p = |s| parse_trailer(s);
        let o = |s: &str| Some(s.to_string());
        assert_eq!(
            p("claude:claude-opus-5-5"),
            (o("claude"), o("claude-opus-5-5"))
        );
        assert_eq!(
            p("Claude Code (claude-opus-5-5)"),
            (o("claude"), o("claude-opus-5-5"))
        );
        assert_eq!(
            p("Claude Code:claude-opus-5-5"),
            (o("claude"), o("claude-opus-5-5"))
        );
        assert_eq!(p("codex"), (o("codex"), None));
        assert_eq!(p("codex:gpt-6.1-sol"), (o("codex"), o("gpt-6.1-sol")));
        assert_eq!(p("pi:xai/grok-4.3"), (o("pi"), o("grok-4.3")));
        assert_eq!(p("Claude Opus 5.5"), (o("claude"), None));
        assert_eq!(
            p("Claude Code <noreply@anthropic.com>"),
            (o("claude"), None)
        );
    }

    #[test]
    fn last_model_skips_synthetic_values() {
        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join("s.jsonl");
        std::fs::write(
            &f,
            "{\"message\":{\"model\":\"claude-opus-5-5\"}}\n{\"message\":{\"model\":\"<synthetic>\"}}\n",
        )
        .unwrap();
        assert_eq!(last_model(&f).as_deref(), Some("claude-opus-5-5"));
        std::fs::write(&f, "{\"payload\":{\"model\": \"gpt-6.1-sol\"}}\n").unwrap();
        assert_eq!(last_model(&f).as_deref(), Some("gpt-6.1-sol"));
    }

    #[test]
    fn excludes_exact_models_and_uncertain_harnesses() {
        let mut a = Author::default();
        a.add(Some("claude".into()), Some("claude-opus-5-5".into()));
        a.add(None, Some("gpt-6.1-sol".into()));
        assert!(!a.excludes("claude", "claude-sonnet-5-5"));
        assert!(a.excludes("pi", "claude-opus-5-5"));
        assert!(a.excludes("codex", "gpt-6.1-sol"));
        assert!(!a.excludes("codex", "gpt-5.6-luna"));
        a.add(Some("codex".into()), None);
        a.add(Some("codex".into()), Some("gpt-6.1-sol".into()));
        assert!(a.excludes("codex", "gpt-5.6-luna"));
    }
}
