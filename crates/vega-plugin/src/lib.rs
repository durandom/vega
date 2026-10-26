//! Shared by the `vega` umbrella, `vega-skills` and the plugins' tests:
//! finding `vega-<name>` plugins on PATH, and the SKILL.md frontmatter
//! contract every plugin's `--skill` output must meet.

use std::collections::BTreeSet;
use std::env;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

pub const PREFIX: &str = "vega-";

/// `bd --json` output with the `{"data": ..., "schema_version": n}` envelope
/// (`BD_JSON_ENVELOPE=1`, default in bd 1.2) removed; bare JSON passes unchanged.
pub fn unwrap_bd_envelope(out: &str) -> String {
    match serde_json::from_str::<serde_json::Value>(out) {
        Ok(serde_json::Value::Object(mut o))
            if o.contains_key("schema_version") && o.contains_key("data") =>
        {
            o.remove("data")
                .map_or_else(|| out.to_string(), |d| d.to_string())
        }
        _ => out.to_string(),
    }
}

/// Runs plugin `vega-<name>` with `args` for a moved command's thin alias:
/// the binary next to the running one, else from PATH. Returns its exit code
/// (127 with a stderr line when it is missing).
pub fn run_plugin(name: &str, args: &[&str]) -> std::process::ExitCode {
    let bin = format!("{PREFIX}{name}");
    let path = env::current_exe()
        .ok()
        .and_then(|e| Some(e.parent()?.join(&bin)))
        .filter(|p| is_executable(p))
        .or_else(|| find_on_path(&bin));
    let Some(path) = path else {
        eprintln!("error: {bin} not found; install vega (mise run install)");
        return std::process::ExitCode::from(127);
    };
    match std::process::Command::new(&path).args(args).status() {
        Ok(s) => std::process::ExitCode::from(s.code().unwrap_or(1).clamp(0, 255) as u8),
        Err(e) => {
            eprintln!("error: {}: {e}", path.display());
            std::process::ExitCode::from(1)
        }
    }
}

/// Names (without `vega-`) of the executable plugins on PATH, sorted,
/// deduplicated.
pub fn plugins() -> BTreeSet<String> {
    path_dirs()
        .filter_map(|dir| std::fs::read_dir(dir).ok())
        .flatten()
        .flatten()
        .filter(|e| is_executable(&e.path()))
        .filter_map(|e| {
            e.file_name()
                .to_str()?
                .strip_prefix(PREFIX)
                .map(str::to_owned)
        })
        .collect()
}

/// The real `bd` on PATH for vegad, whose own PATH may lack it: shims
/// (the bd claim shim, vega's agent shims) find it through a PATH vegad
/// does not have. Plain `bd` if none is found.
pub fn bd_path() -> String {
    path_dirs()
        .filter(|d| !d.to_string_lossy().contains("shim"))
        .map(|d| d.join("bd"))
        .find(|f| f.is_file())
        .map_or_else(|| "bd".into(), |f| f.display().to_string())
}

/// First executable named `bin` on PATH.
pub fn find_on_path(bin: &str) -> Option<PathBuf> {
    path_dirs().map(|d| d.join(bin)).find(|p| is_executable(p))
}

fn path_dirs() -> impl Iterator<Item = PathBuf> {
    env::var_os("PATH")
        .map(|p| env::split_paths(&p).collect::<Vec<_>>())
        .unwrap_or_default()
        .into_iter()
}

fn is_executable(path: &Path) -> bool {
    path.metadata()
        .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

/// The SKILL.md frontmatter of a plugin guide.
#[derive(Debug, PartialEq)]
pub struct Frontmatter {
    pub name: String,
    pub description: String,
}

/// Parses and validates the frontmatter that opens a guide.
///
/// The contract is the SKILL.md one (name: lowercase letters, digits and
/// hyphens, at most 64 chars; description: non-empty, at most 1024 chars, no
/// XML tags), narrowed so a line parser suffices: both fields on one line
/// each, plain YAML scalars (no `: ` or ` #`, no leading quote or indicator).
pub fn parse_frontmatter(guide: &str) -> Result<Frontmatter, String> {
    let mut lines = guide.lines();
    if lines.next() != Some("---") {
        return Err("guide does not start with a `---` frontmatter line".into());
    }
    let (mut name, mut description) = (None, None);
    let mut closed = false;
    for line in lines.by_ref() {
        if line == "---" {
            closed = true;
            break;
        }
        let Some((key, value)) = line.split_once(':') else {
            return Err(format!("frontmatter line is not `key: value`: {line}"));
        };
        let value = value.trim();
        match key {
            "name" => name = Some(value.to_string()),
            "description" => description = Some(value.to_string()),
            _ => return Err(format!("unknown frontmatter key `{key}`")),
        }
    }
    if !closed {
        return Err("frontmatter has no closing `---` line".into());
    }
    let name = name.ok_or("frontmatter has no name")?;
    let description = description.ok_or("frontmatter has no description")?;

    if name.is_empty()
        || name.len() > 64
        || !name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        return Err(format!(
            "name `{name}` must be 1-64 lowercase letters, digits or hyphens"
        ));
    }
    if name.contains("anthropic") || name.contains("claude") {
        return Err(format!("name `{name}` contains a reserved word"));
    }
    if description.is_empty() {
        return Err("description is empty".into());
    }
    if description.chars().count() > 1024 {
        return Err("description is longer than 1024 chars".into());
    }
    if has_xml_tag(&description) {
        return Err("description contains an XML tag".into());
    }
    if description.contains(": ") || description.contains(" #") {
        return Err("description is not a plain YAML scalar (`: ` or ` #`)".into());
    }
    if description.starts_with(|c| "[]{}\"'>|*&!%@`#,?-".contains(c)) {
        return Err("description starts with a YAML indicator".into());
    }
    if lines.all(|l| l.trim().is_empty()) {
        return Err("guide has no body after the frontmatter".into());
    }
    Ok(Frontmatter { name, description })
}

fn has_xml_tag(s: &str) -> bool {
    s.match_indices('<').any(|(i, _)| {
        s[i + 1..]
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || c == '/')
    })
}

/// Vega's local guide budget: bodies load into agents mid-task, so keep them
/// concise. Count words rather than lines. For upstream authoring guidance,
/// docs/skill_best_practices.md links to Anthropic's best-practices page.
pub const MAX_BODY_WORDS: usize = 400;

/// Checks Vega's local guide policy: valid frontmatter, third-person triggers,
/// word budget, forward-slash paths and wording without stale dates.
/// Returns one finding per broken rule; terminology and degrees of freedom
/// require separate evaluation.
pub fn lint_guide(guide: &str) -> Vec<String> {
    let fm = match parse_frontmatter(guide) {
        Ok(fm) => fm,
        Err(e) => return vec![e],
    };
    let mut findings = Vec::new();
    let desc = fm.description.to_lowercase();
    let first_or_second_person = desc
        .split(|c: char| !c.is_ascii_alphanumeric() && c != '\'')
        .any(|w| matches!(w, "i" | "i'm" | "my" | "you" | "your" | "you're"));
    if first_or_second_person {
        findings.push("description must be in the third person (no I/you/your)".to_string());
    }
    if !(fm.description.contains("Use when") || fm.description.contains("Use only when")) {
        findings.push("description must say when to use it (`Use when ...`)".to_string());
    }
    let body = body_of(guide);
    let words = body.split_whitespace().count();
    if words > MAX_BODY_WORDS {
        findings.push(format!(
            "body has {words} words; the budget is {MAX_BODY_WORDS}"
        ));
    }
    if let Some(p) = backslash_path(body) {
        findings.push(format!("Windows-style path `{p}`; use forward slashes"));
    }
    if let Some(t) = time_sensitive(guide) {
        findings.push(format!(
            "time-sensitive wording `{t}`; describe the current behavior instead"
        ));
    }
    findings
}

/// The text after the closing `---` of the frontmatter.
fn body_of(guide: &str) -> &str {
    guide
        .strip_prefix("---\n")
        .and_then(|rest| rest.split_once("\n---\n"))
        .map_or("", |(_, body)| body)
}

/// A token like `scripts\helper.py`: a backslash between path characters.
fn backslash_path(text: &str) -> Option<&str> {
    text.split_whitespace().find(|w| {
        let w = w.trim_matches(|c: char| "`'\"()[],.".contains(c));
        w.char_indices().any(|(i, c)| {
            c == '\\'
                && i > 0
                && w[..i].chars().last().is_some_and(char::is_alphanumeric)
                && w[i + 1..].chars().next().is_some_and(char::is_alphanumeric)
        })
    })
}

/// Dates that will go stale: `August 2025`, `before 2026`, `as of 2026`.
fn time_sensitive(text: &str) -> Option<String> {
    const MONTHS: [&str; 12] = [
        "january",
        "february",
        "march",
        "april",
        "may",
        "june",
        "july",
        "august",
        "september",
        "october",
        "november",
        "december",
    ];
    const MARKERS: [&str; 4] = ["before", "after", "until", "of"];
    let words: Vec<String> = text
        .split_whitespace()
        .map(|w| {
            w.trim_matches(|c: char| !c.is_alphanumeric())
                .to_lowercase()
        })
        .collect();
    words.windows(2).find_map(|pair| {
        let year = pair[1].len() == 4
            && pair[1].starts_with("20")
            && pair[1].chars().all(|c| c.is_ascii_digit());
        let lead = MONTHS.contains(&pair[0].as_str()) || MARKERS.contains(&pair[0].as_str());
        (year && lead).then(|| format!("{} {}", pair[0], pair[1]))
    })
}

/// Test helper for plugins: their `--skill` text meets the contract, passes
/// `lint_guide` and is named `vega-<plugin>`.
pub fn assert_guide(guide: &str, plugin: &str) {
    let fm = parse_frontmatter(guide).unwrap_or_else(|e| panic!("vega-{plugin} guide: {e}"));
    assert_eq!(fm.name, format!("{PREFIX}{plugin}"));
    let findings = lint_guide(guide);
    assert!(
        findings.is_empty(),
        "vega-{plugin} guide breaks the skill rules:\n- {}",
        findings.join("\n- ")
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bd_envelope_is_unwrapped_and_bare_json_passes() {
        assert_eq!(
            unwrap_bd_envelope(r#"{"data":[{"id":"a"}],"schema_version":1}"#),
            r#"[{"id":"a"}]"#
        );
        assert_eq!(unwrap_bd_envelope(r#"[{"id":"a"}]"#), r#"[{"id":"a"}]"#);
        assert_eq!(unwrap_bd_envelope("no json"), "no json");
    }

    const OK: &str = "---\nname: vega-x\ndescription: Does x. Use when y.\n---\n\n# vega x\n";

    #[test]
    fn parses_valid_guide() {
        let fm = parse_frontmatter(OK).unwrap();
        assert_eq!(fm.name, "vega-x");
        assert_eq!(fm.description, "Does x. Use when y.");
    }

    #[test]
    fn rejects_bad_guides() {
        let bad = [
            "# no frontmatter\n",
            "---\nname: vega-x\ndescription: d\n",
            "---\nname: Vega_X\ndescription: d\n---\nbody\n",
            "---\nname: vega-claude\ndescription: d\n---\nbody\n",
            "---\nname: vega-x\ndescription:\n---\nbody\n",
            "---\nname: vega-x\ndescription: send to <pane> now\n---\nbody\n",
            "---\nname: vega-x\ndescription: a: b\n---\nbody\n",
            "---\nname: vega-x\ndescription: \"quoted\"\n---\nbody\n",
            "---\nname: vega-x\ndescription: d\nextra: e\n---\nbody\n",
            "---\nname: vega-x\ndescription: d\n---\n\n",
        ];
        for g in bad {
            assert!(parse_frontmatter(g).is_err(), "accepted: {g:?}");
        }
        let long = format!(
            "---\nname: vega-x\ndescription: {}\n---\nb\n",
            "a".repeat(1025)
        );
        assert!(parse_frontmatter(&long).is_err());
    }

    #[test]
    fn lint_accepts_a_short_third_person_guide() {
        assert_eq!(lint_guide(OK), Vec::<String>::new());
    }

    #[test]
    fn lint_flags_each_broken_rule() {
        let lint = |desc: &str, body: &str| {
            lint_guide(&format!(
                "---\nname: vega-x\ndescription: {desc}\n---\n{body}\n"
            ))
        };
        let ok = "Does x. Use when y.";
        assert!(lint("You can do x. Use when y.", "b")[0].contains("third person"));
        assert!(lint("Does x for your files. Use when y.", "b")[0].contains("third person"));
        assert!(lint("Does x.", "b")[0].contains("when to use"));
        assert!(lint(ok, &"word ".repeat(MAX_BODY_WORDS + 1))[0].contains("budget"));
        assert!(lint(ok, "Run scripts\\helper.py now")[0].contains("Windows"));
        assert!(lint(ok, "Before 2026 use v1.")[0].contains("time-sensitive"));
        assert!(lint(ok, "Changed in August 2025.")[0].contains("time-sensitive"));
        assert!(lint(ok, "Escape with `\\n` or a 2026-10-08 stamp.").is_empty());
    }

    #[test]
    fn allows_comparison_signs() {
        let g = "---\nname: vega-x\ndescription: Keeps n < 5 and a <= b.\n---\nb\n";
        assert!(parse_frontmatter(g).is_ok());
    }
}
