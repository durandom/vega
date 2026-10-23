//! herdr's `~/.config/herdr/config.toml`, owned by vega (vega-lhc.103,
//! docs/design/theme-symbols.md "herdr's config", docs/design/config.md).
//!
//! The source is the repo's `config/herdr/config.toml`: herdr's config with
//! double-brace placeholders for what `~/.config/vega/theme.toml` already holds
//! (badge colours, host labels and colours, the sentinel icon) and for vega's
//! fixed sidebar colours. [`render`] fills them in and keeps the keys herdr
//! writes back itself ([`HERDR_OWNED`]) at their live value. The `herdr` unit
//! writes the result only after [`check`] accepts it, then [`reload`]s the
//! running server.

use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use toml_edit::{DocumentMut, Value};
use vega_herdr::theme::{
    Rgb, Symbols, CELL_BLANK, CLAUDE, CODEX, COPILOT, DECISIONS, NOTES, OTHER_AGENT, PI, SENTINEL,
    UAC, WAITING,
};

use crate::Env;

/// The source, with its placeholders.
pub const TEMPLATE: &str = crate::sources::HERDR;

/// `$host` on a host without a `[symbols.host_colors]` entry.
pub const HOST_COLOR: Rgb = Rgb(0x8a, 0x8a, 0x8a);
/// A sentinel's `$label` and `$badge`.
pub const SENTINEL_COLOR: Rgb = Rgb(0x45, 0x92, 0x2a);
/// `$git` when clean and level with upstream (`✓`).
pub const GIT_CLEAN_COLOR: Rgb = Rgb(0x45, 0x92, 0x2a);
/// `$git` otherwise (`↑N ↓N ●`).
pub const GIT_DIRTY_COLOR: Rgb = Rgb(0xf0, 0x78, 0x30);

/// Keys herdr rewrites itself (`[ui] agent_panel_sort` when the sort is
/// toggled in the UI): (table, key). The live value wins over the template's.
pub const HERDR_OWNED: [(&str, &str); 1] = [("ui", "agent_panel_sort")];

/// `$XDG_CONFIG_HOME/herdr/config.toml`, where herdr reads it.
pub fn path(env: &Env) -> PathBuf {
    env.config.join("herdr/config.toml")
}

/// The symbols of `$XDG_CONFIG_HOME/vega/theme.toml` (the nerd set without one).
pub fn symbols(env: &Env) -> Symbols {
    fs::read_to_string(env.config.join("vega/theme.toml"))
        .map_or_else(|_| Symbols::default(), |t| vega_herdr::theme::parse(&t))
}

/// A rendered config.
#[derive(Debug, Clone, PartialEq)]
pub struct Rendered {
    pub text: String,
    /// The herdr-owned keys taken from the live file: (`table.key`, value).
    pub kept: Vec<(String, String)>,
}

/// `template` with its placeholders filled from `sym`, and the herdr-owned
/// keys set to their value in `live` (the current file, if any).
pub fn render(template: &str, sym: &Symbols, live: Option<&str>) -> Result<Rendered, String> {
    let color = |c: Rgb| quote(&c.hex());
    let topic = |t| sym.color(t).map_or_else(String::new, color);
    let sentinel = match sym.get(&SENTINEL) {
        "" => SENTINEL.nerd,
        s => s,
    };
    let host_rules: Vec<String> = sym
        .host_colors()
        .into_iter()
        .map(|(_, label, c)| format!("     {{ equals = {}, fg = {} }},", quote(&label), color(c)))
        .collect();
    // One rule per known kind by its exact `$kind` value; other_agent is the
    // token's own fg. An empty override reports no token, so no rule.
    let kind_rules: Vec<String> = [&CLAUDE, &CODEX, &COPILOT, &PI]
        .into_iter()
        .filter_map(|t| Some((sym.kind(t), sym.color(t)?)))
        .filter(|(k, _)| !k.is_empty())
        .map(|(k, c)| format!("      {{ equals = {}, fg = {} }},", quote(&k), color(c)))
        .collect();
    // Workspace glyphs use compact text symbols; match those exact values.
    let space_kind_rules: Vec<String> = [&CLAUDE, &CODEX, &COPILOT, &PI]
        .into_iter()
        .filter_map(|t| Some((sym.space_kind(t), sym.color(t)?)))
        .filter(|(k, _)| !k.is_empty())
        .map(|(k, c)| format!("      {{ equals = {}, fg = {} }},", quote(k), color(c)))
        .collect();
    let vars = [
        ("color.host", color(HOST_COLOR)),
        ("color.sentinel", color(SENTINEL_COLOR)),
        ("color.git_clean", color(GIT_CLEAN_COLOR)),
        ("color.git_dirty", color(GIT_DIRTY_COLOR)),
        ("color.uac", topic(&UAC)),
        ("color.notes", topic(&NOTES)),
        ("color.decisions", topic(&DECISIONS)),
        ("color.waiting", topic(&WAITING)),
        ("color.other_agent", topic(&OTHER_AGENT)),
        ("icon.sentinel", quote(sentinel)),
        ("host_rules", host_rules.join("\n")),
        ("kind_rules", kind_rules.join("\n")),
        ("space_kind_rules", space_kind_rules.join("\n")),
    ];
    let mut text = template.to_string();
    for (k, v) in &vars {
        text = text.replace(&format!("{{{{{k}}}}}"), v);
    }
    if let Some(i) = text.find("{{") {
        let name: String = text[i..].chars().take_while(|c| *c != '\n').collect();
        return Err(format!(
            "config/herdr/config.toml: unknown placeholder {name}"
        ));
    }
    // A line that held only {{host_rules}} (or {{kind_rules}}) and no
    // entries: drop the blank line.
    let text = text.replace("rules = [\n\n", "rules = [\n");
    let mut doc: DocumentMut = text
        .parse()
        .map_err(|e| format!("config/herdr/config.toml: rendered TOML is invalid: {e}"))?;
    let live: Option<DocumentMut> = live.and_then(|l| l.parse().ok());
    let mut kept = Vec::new();
    for (table, key) in HERDR_OWNED {
        let Some(v) = live
            .as_ref()
            .and_then(|d| d.get(table)?.get(key)?.as_str().map(str::to_string))
            .filter(|v| !v.is_empty() && v.chars().all(|c| c.is_ascii_lowercase() || c == '_'))
        else {
            continue;
        };
        let Some(slot) = doc
            .get_mut(table)
            .and_then(|t| t.get_mut(key))
            .and_then(|i| i.as_value_mut())
        else {
            continue;
        };
        let decor = slot.decor().clone();
        *slot = Value::from(v.as_str());
        *slot.decor_mut() = decor;
        kept.push((format!("{table}.{key}"), v));
    }
    Ok(Rendered {
        text: doc.to_string(),
        kept,
    })
}

/// A TOML basic string; private-use glyphs (Nerd Font icons, invisible in
/// most editors) and CELL_BLANK as `\U` escapes.
fn quote(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{E000}'..='\u{F8FF}' | '\u{F0000}'..='\u{10FFFF}' | CELL_BLANK => {
                out.push_str(&format!("\\U{:08X}", c as u32))
            }
            c if c.is_control() => out.push_str(&format!("\\u{:04X}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// What [`check`] found.
#[derive(Debug, Clone, PartialEq)]
pub enum Check {
    Ok,
    /// No herdr on this host.
    NoHerdr,
    /// `herdr config check` rejected the candidate: its issues.
    Rejected(Vec<String>),
}

/// Runs `<herdr> config check` with `HERDR_CONFIG_PATH` on a temp copy of
/// `text` next to `path` (removed afterwards).
pub fn check(path: &Path, text: &str, herdr: &OsStr) -> Result<Check, String> {
    let dir = path.parent().ok_or("no parent directory")?;
    fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let candidate = dir.join(format!(".config.toml.vega-check.{}", std::process::id()));
    fs::write(&candidate, text).map_err(|e| format!("{}: {e}", candidate.display()))?;
    let out = Command::new(herdr)
        .args(["config", "check"])
        .env("HERDR_CONFIG_PATH", &candidate)
        .output();
    let _ = fs::remove_file(&candidate);
    let out = match out {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Check::NoHerdr),
        Err(e) => return Err(format!("herdr config check: {e}")),
        Ok(o) => o,
    };
    if out.status.success() {
        return Ok(Check::Ok);
    }
    let mut lines: Vec<String> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .chain(String::from_utf8_lossy(&out.stderr).lines())
        .map(str::trim)
        .filter(|l| !l.is_empty() && *l != "config: issues found")
        .map(str::to_string)
        .collect();
    if lines.is_empty() {
        lines.push(format!("herdr config check exited {}", out.status));
    }
    Ok(Check::Rejected(lines))
}

/// `<herdr> server reload-config`; an error (e.g. no server) keeps the
/// written file: herdr reads it on its next start.
pub fn reload(herdr: &OsStr) -> Result<(), String> {
    match Command::new(herdr)
        .args(["server", "reload-config"])
        .output()
    {
        Err(e) => Err(e.to_string()),
        Ok(o) if o.status.success() => Ok(()),
        Ok(o) => Err(last_line(&o.stderr)
            .or_else(|| last_line(&o.stdout))
            .unwrap_or_else(|| format!("exited {}", o.status))),
    }
}

fn last_line(b: &[u8]) -> Option<String> {
    String::from_utf8_lossy(b)
        .lines()
        .rev()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .map(str::to_string)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    const THEME: &str = "[symbols]\nset = \"nerd\"\nuac_text = \"u▲c\"\n\n[symbols.icons]\nuac = \"\"\n\n[symbols.colors]\nuac = \"#e5484d\"\nnotes = \"#e0a020\"\ndecisions = \"#9d6bf0\"\nwaiting = \"#f07830\"\n\n[symbols.hosts]\nproteus = \"♆₈\"\nganymede = \"♃₃\"\nio = \"♃₁\"\ndeimos = \"♂₂\"\n\n[symbols.host_colors]\nproteus = \"#3d73cc\"\nganymede = \"#2a9187\"\nio = \"#a87c00\"\ndeimos = \"#c8463c\"\n";

    /// A fake herdr: `config check` passes unless the candidate contains
    /// `BAD`; every call is logged to `<dir>/calls`.
    pub(crate) fn fake_herdr(d: &Path) -> PathBuf {
        let bin = d.join("herdr");
        let log = d.join("calls");
        fs::write(
            &bin,
            format!(
                "#!/bin/sh\necho \"$*\" >> '{}'\nif [ \"$1\" = config ]; then\n  if grep -q BAD \"$HERDR_CONFIG_PATH\"; then echo 'config: issues found'; echo 'bad key BAD'; exit 1; fi\n  echo 'config: ok'\nfi\nexit 0\n",
                log.display()
            ),
        )
        .unwrap();
        fs::set_permissions(&bin, fs::Permissions::from_mode(0o755)).unwrap();
        bin
    }

    #[test]
    fn check_passes_rejects_and_leaves_no_candidate() {
        let d = tempfile::tempdir().unwrap();
        let herdr = fake_herdr(d.path());
        let p = d.path().join("h/config.toml");
        assert_eq!(check(&p, "a = 1\n", herdr.as_os_str()).unwrap(), Check::Ok);
        assert_eq!(
            check(&p, "BAD = 1\n", herdr.as_os_str()).unwrap(),
            Check::Rejected(vec!["bad key BAD".into()])
        );
        assert_eq!(fs::read_dir(d.path().join("h")).unwrap().count(), 0);
        let missing = d.path().join("no-herdr");
        assert_eq!(
            check(&p, "a = 1\n", missing.as_os_str()).unwrap(),
            Check::NoHerdr
        );
        assert!(reload(herdr.as_os_str()).is_ok());
        assert_eq!(
            fs::read_to_string(d.path().join("calls")).unwrap(),
            "config check\nconfig check\nserver reload-config\n"
        );
    }

    fn get<'a>(doc: &'a toml_edit::DocumentMut, path: &[&str]) -> &'a toml_edit::Item {
        path.iter().fold(doc.as_item(), |i, k| &i[*k])
    }

    #[test]
    fn renders_theme_values_into_the_sidebar() {
        let r = render(TEMPLATE, &vega_herdr::theme::parse(THEME), None).unwrap();
        assert!(r.kept.is_empty());
        let doc: DocumentMut = r.text.parse().unwrap();
        assert_eq!(
            get(&doc, &["ui", "agent_panel_sort"]).as_str(),
            Some("priority")
        );
        let host = &get(&doc, &["ui", "sidebar", "agents", "rows"])
            .as_array()
            .unwrap()
            .get(0)
            .unwrap()
            .as_array()
            .unwrap()
            .get(1)
            .unwrap()
            .as_inline_table()
            .unwrap()
            .clone();
        assert_eq!(host.get("fg").unwrap().as_str(), Some("#8a8a8a"));
        let rules: Vec<(String, String)> = host
            .get("rules")
            .unwrap()
            .as_array()
            .unwrap()
            .iter()
            .map(|r| {
                let t = r.as_inline_table().unwrap();
                (
                    t.get("equals").unwrap().as_str().unwrap().to_string(),
                    t.get("fg").unwrap().as_str().unwrap().to_string(),
                )
            })
            .collect();
        assert_eq!(rules.len(), 4);
        assert!(rules.contains(&("♆₈".into(), "#3d73cc".into())));
        assert!(rules.contains(&("♂₂".into(), "#c8463c".into())));
        assert!(r
            .text
            .contains(r##"{ token = "$uac", fg = "#e5484d", bold = true }"##));
        assert!(r
            .text
            .contains(r##"starts_with = "\U000F0493", fg = "#45922a""##));
        assert!(!r.text.contains("{{"));
    }

    #[test]
    fn defaults_without_theme_and_unknown_placeholders_fail() {
        let r = render(TEMPLATE, &Symbols::default(), None).unwrap();
        assert!(r
            .text
            .contains(r##"{ token = "$uac", fg = "#b56d0c", bold = true }"##));
        assert!(
            r.text.contains("rules = [\n   ] },"),
            "no host rules, no blank line"
        );
        r.text.parse::<DocumentMut>().unwrap();
        let e = render("a = {{nope}}\n", &Symbols::default(), None).unwrap_err();
        assert!(e.contains("unknown placeholder {{nope}}"), "{e}");
    }

    #[test]
    fn kind_token_gets_one_rule_per_kind_with_its_colour() {
        let r = render(TEMPLATE, &Symbols::default(), None).unwrap();
        assert!(r.text.contains(
            r##"{ token = "$kind", fg = "#808080", rules = [
      { equals = "\U0000EC82\U00002800", fg = "#c96442" },"##
        ));
        assert!(r
            .text
            .contains(r##"{ equals = "\U000F03FF\U00002800", fg = "#1f8fb0" },"##));
        let doc: DocumentMut = r.text.parse().unwrap();
        let kind = get(&doc, &["ui", "sidebar", "agents", "rows"])[1][0]
            .as_inline_table()
            .unwrap()
            .clone();
        let rules = kind.get("rules").unwrap().as_array().unwrap();
        assert_eq!(rules.len(), 4);
        assert_eq!(
            rules.get(0).unwrap().as_inline_table().unwrap()["equals"].as_str(),
            Some("\u{EC82}\u{2800}")
        );
        // Workspace slots use text symbols with the same per-kind colours.
        for slot in 1..=5 {
            let entry = get(&doc, &["ui", "sidebar", "spaces", "rows"])[0][slot + 1]
                .as_inline_table()
                .unwrap();
            assert_eq!(entry["token"].as_str().unwrap(), format!("$kind{slot}"));
            assert_eq!(entry["fg"].as_str(), kind["fg"].as_str());
            let pairs = |value: &Value| {
                value
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|rule| {
                        let rule = rule.as_inline_table().unwrap();
                        (
                            rule["equals"].as_str().unwrap().to_string(),
                            rule["fg"].as_str().unwrap().to_string(),
                        )
                    })
                    .collect::<Vec<_>>()
            };
            let expected: Vec<_> = ["✳", "֍", "✈", "π"]
                .into_iter()
                .zip(pairs(&kind["rules"]))
                .map(|(glyph, (_, colour))| (glyph.to_string(), colour))
                .collect();
            assert_eq!(pairs(&entry["rules"]), expected);
        }
        let own = vega_herdr::theme::parse(
            "[symbols.icons]\nclaude = \"C\"\ncodex = \"\"\n[symbols.colors]\nclaude = \"#010203\"\nother_agent = \"#040506\"\n",
        );
        let r = render(TEMPLATE, &own, None).unwrap();
        assert!(r.text.contains(r##"fg = "#040506", rules = ["##));
        assert!(r.text.contains(r##"{ equals = "C", fg = "#010203" },"##));
        assert!(!r.text.contains("\\U0000EC81"), "empty codex: no rule");
    }

    #[test]
    fn keeps_herdr_owned_keys_from_the_live_file() {
        let sym = Symbols::default();
        let r = render(
            TEMPLATE,
            &sym,
            Some("[ui]\nagent_panel_sort = \"recent\"\n"),
        )
        .unwrap();
        assert_eq!(
            r.kept,
            [("ui.agent_panel_sort".to_string(), "recent".to_string())]
        );
        assert!(r.text.contains("\nagent_panel_sort = \"recent\"\n"));
        // Garbled or missing: the template's default.
        for live in [
            "[ui]\nagent_panel_sort = \"x\\\"; y\"\n",
            "[ui]\n",
            "not toml [",
            "",
        ] {
            let r = render(TEMPLATE, &sym, Some(live)).unwrap();
            assert!(r.kept.is_empty(), "{live}");
            assert!(r.text.contains("\nagent_panel_sort = \"priority\"\n"));
        }
    }
}
