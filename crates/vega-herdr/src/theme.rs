//! The user's theme, `~/.config/vega/theme.toml` (`$XDG_CONFIG_HOME/vega`).
//! This module reads its `[symbols]` section: the symbols of the UAC pane's
//! label (vegad) and of `vega board`. Other sections belong to `vega theme`.
//!
//! ```toml
//! [symbols]
//! set = "nerd"        # or "emoji": the base table
//!
//! [symbols.icons]     # optional overrides per topic, any string
//! notes = "✉"
//!
//! [symbols.colors]    # optional, badge and agent kind topics, "#RRGGBB"
//! notes = "#2f7fd8"
//!
//! [symbols.hosts]     # optional, host name -> label of its `$host` token
//! example = "E"
//!
//! [symbols.host_colors] # optional, host name -> "#RRGGBB"
//! example = "#2f7fd8"
//! ```
//!
//! TOPICS names and documents every topic. The renderer adds one space after
//! each symbol; the config holds the bare symbol. A missing file means the
//! nerd set; a bad value, unknown set or unknown topic is reported in
//! `Symbols::errors` and the rest still applies. Readers re-read the file at
//! most every READ_EVERY, so a change shows without a restart.

use std::collections::BTreeMap;
use std::env;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use toml_edit::{DocumentMut, Item};

/// How long a read of the theme file stays good.
const READ_EVERY: Duration = Duration::from_secs(2);

/// One symbol: its name in `[symbols.icons]`, what it marks, and its
/// symbol in each set.
#[derive(Debug, PartialEq, Eq)]
pub struct Topic {
    pub name: &'static str,
    pub about: &'static str,
    pub nerd: &'static str,
    pub emoji: &'static str,
}

// Nerd: monochrome Nerd Font glyphs chosen by the Slayer, codepoints checked
// against nerd-fonts' glyphnames.json (md = Material Design, fa = Font Awesome).
pub const UAC: Topic = Topic {
    name: "uac",
    about: "the UAC pane's marker, before UAC",
    nerd: "\u{F0768}", // md atom
    emoji: "\u{2622}\u{FE0F}",
};
pub const NOTES: Topic = Topic {
    name: "notes",
    about: "badge: unread notifications",
    nerd: "\u{F01EE}", // md email
    emoji: "\u{1F514}",
};
pub const DECISIONS: Topic = Topic {
    name: "decisions",
    about: "badge: open slayer-decision beads",
    nerd: "\u{F05D1}", // md scale_balance
    emoji: "\u{1F5F3}\u{FE0F}",
};
pub const WAITING: Topic = Topic {
    name: "waiting",
    about: "badge: headed panes waiting on the user",
    nerd: "\u{F256}", // fa hand_paper_o
    emoji: "\u{270B}",
};
pub const SENTINEL: Topic = Topic {
    name: "sentinel",
    about: "sidebar: prefix of a sentinel's name",
    nerd: "\u{F0493}", // md cog
    emoji: "\u{2699}\u{FE0F}",
};
pub const CLAUDE: Topic = Topic {
    name: "claude",
    about: "sidebar: agent kind claude",
    nerd: "\u{EC82}", // cod claude (Nerd Fonts >= 3.4)
    emoji: "\u{2733}\u{FE0F}",
};
pub const CODEX: Topic = Topic {
    name: "codex",
    about: "sidebar: agent kind codex",
    nerd: "\u{EC81}", // cod openai (Nerd Fonts >= 3.4)
    emoji: "\u{1FAA2}",
};
pub const COPILOT: Topic = Topic {
    name: "copilot",
    about: "sidebar: agent kind copilot",
    nerd: "\u{EC1E}", // cod copilot
    emoji: "\u{2708}\u{FE0F}",
};
pub const PI: Topic = Topic {
    name: "pi",
    about: "sidebar: agent kind pi",
    nerd: "\u{F03FF}", // md pi
    emoji: "\u{1F967}",
};
pub const OTHER_AGENT: Topic = Topic {
    name: "other_agent",
    about: "sidebar: any other agent kind",
    nerd: "\u{F06A9}", // md robot
    emoji: "\u{1F916}",
};
pub const OPEN: Topic = Topic {
    name: "open",
    about: "bead status open",
    nerd: "\u{F0766}", // md circle_outline
    emoji: "\u{1F535}",
};
pub const IN_PROGRESS: Topic = Topic {
    name: "in_progress",
    about: "bead status in_progress",
    nerd: "\u{F1396}", // md circle_half_full
    emoji: "\u{1F7E1}",
};
pub const BLOCKED: Topic = Topic {
    name: "blocked",
    about: "bead status blocked",
    nerd: "\u{F0765}", // md circle
    emoji: "\u{1F534}",
};
pub const DEFERRED: Topic = Topic {
    name: "deferred",
    about: "bead status deferred",
    nerd: "\u{F0717}", // md snowflake
    emoji: "\u{2744}\u{FE0F}",
};
pub const CLOSED: Topic = Topic {
    name: "closed",
    about: "bead status closed",
    nerd: "\u{F012C}", // md check
    emoji: "\u{2705}",
};
pub const PINNED: Topic = Topic {
    name: "pinned",
    about: "bead status pinned",
    nerd: "\u{F0403}", // md pin
    emoji: "\u{1F4CC}",
};
pub const HOOKED: Topic = Topic {
    name: "hooked",
    about: "bead status hooked",
    nerd: "\u{F06E2}", // md hook
    emoji: "\u{1FA9D}",
};
pub const OTHER_STATUS: Topic = Topic {
    name: "other_status",
    about: "any other bead status",
    nerd: "\u{F09DF}", // md circle_small
    emoji: "\u{26AA}",
};
pub const EPIC: Topic = Topic {
    name: "epic",
    about: "bead type epic",
    nerd: "\u{F070B}", // md rhombus
    emoji: "\u{1F537}",
};
pub const DECISION_BEAD: Topic = Topic {
    name: "decision_bead",
    about: "bead type decision",
    nerd: "\u{F05D1}", // md scale_balance
    emoji: "\u{1F5F3}\u{FE0F}",
};
pub const BLOCKED_BY: Topic = Topic {
    name: "blocked_by",
    about: "a bead blocked by another bead",
    nerd: "\u{F073A}", // md cancel
    emoji: "\u{26D4}",
};
pub const DEPS: Topic = Topic {
    name: "deps",
    about: "a bead's dependency count",
    nerd: "\u{F0339}", // md link_variant
    emoji: "\u{1F517}",
};

/// Every topic, in the order `vega theme` lists them.
pub const TOPICS: [&Topic; 22] = [
    &UAC,
    &NOTES,
    &DECISIONS,
    &WAITING,
    &SENTINEL,
    &CLAUDE,
    &CODEX,
    &COPILOT,
    &PI,
    &OTHER_AGENT,
    &OPEN,
    &IN_PROGRESS,
    &BLOCKED,
    &DEFERRED,
    &CLOSED,
    &PINNED,
    &HOOKED,
    &OTHER_STATUS,
    &EPIC,
    &DECISION_BEAD,
    &BLOCKED_BY,
    &DEPS,
];

/// The topic of herdr's agent kind (its pane's `agent` field): one per
/// known kind, OTHER_AGENT for the rest.
pub fn agent_topic(kind: &str) -> &'static Topic {
    match kind {
        "claude" => &CLAUDE,
        "codex" => &CODEX,
        "copilot" => &COPILOT,
        "pi" => &PI,
        _ => &OTHER_AGENT,
    }
}

/// The topics `[symbols.colors]` may colour, with their default colours:
/// the UAC label's symbols, which herdr's sidebar tokens and the board's
/// badges draw, and the agent kinds of the sidebar's `$kind` token. Each
/// default keeps a contrast of at least 3:1 on the Catppuccin Latte and
/// Macchiato base and mantle backgrounds.
pub const COLORS: [(&Topic, Rgb); 9] = [
    (&UAC, Rgb(0xb5, 0x6d, 0x0c)),
    (&NOTES, Rgb(0x2f, 0x7f, 0xd8)),
    (&DECISIONS, Rgb(0x9a, 0x5c, 0xf0)),
    (&WAITING, Rgb(0xd9, 0x46, 0x5f)),
    (&CLAUDE, Rgb(0xc9, 0x64, 0x42)),      // terracotta
    (&CODEX, Rgb(0x0e, 0x8f, 0x6f)),       // green
    (&COPILOT, Rgb(0x89, 0x57, 0xe5)),     // purple
    (&PI, Rgb(0x1f, 0x8f, 0xb0)),          // blue
    (&OTHER_AGENT, Rgb(0x80, 0x80, 0x80)), // grey
];

/// The blank after a private-use glyph in a herdr token: Ghostty draws a
/// Nerd Font icon two cells wide when the cell after it is whitespace
/// (herdr's ` · ` separator), and herdr trims whitespace from token values,
/// NBSP and thin spaces included. U+2800 (braille blank) is no whitespace,
/// so the icon keeps one cell (vega-lhc.132).
pub const CELL_BLANK: char = '\u{2800}';

/// A colour from `[symbols.colors]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgb(pub u8, pub u8, pub u8);

impl Rgb {
    /// Strict `#RRGGBB`, either case.
    pub fn parse(s: &str) -> Option<Rgb> {
        let h = s.strip_prefix('#')?;
        if h.len() != 6 || !h.chars().all(|c| c.is_ascii_hexdigit()) {
            return None;
        }
        let b = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).ok();
        Some(Rgb(b(0)?, b(2)?, b(4)?))
    }

    /// `#rrggbb`.
    pub fn hex(&self) -> String {
        format!("#{:02x}{:02x}{:02x}", self.0, self.1, self.2)
    }
}

/// The base tables `set` may name.
pub const SETS: [&str; 2] = ["nerd", "emoji"];

/// The symbols in effect: a base set plus the file's overrides.
#[derive(Debug, Clone, PartialEq)]
pub struct Symbols {
    pub set: &'static str,
    icons: BTreeMap<&'static str, String>,
    colors: BTreeMap<&'static str, Rgb>,
    /// The word after the UAC symbol (`[symbols] uac_text`), default `UAC`.
    pub uac_text: String,
    /// `[symbols.hosts]`: host name (lowercase) -> label.
    hosts: BTreeMap<String, String>,
    /// `[symbols.host_colors]`: host name (lowercase) -> colour.
    host_colors: BTreeMap<String, Rgb>,
    /// What in the file was not applied, one line each.
    pub errors: Vec<String>,
}

impl Symbols {
    /// The base set `name` (nerd for an unknown one).
    pub fn of(name: &str) -> Symbols {
        let set = SETS.into_iter().find(|s| *s == name).unwrap_or("nerd");
        Symbols {
            set,
            icons: TOPICS
                .iter()
                .map(|t| {
                    (
                        t.name,
                        if set == "emoji" { t.emoji } else { t.nerd }.to_string(),
                    )
                })
                .collect(),
            colors: COLORS.iter().map(|(t, c)| (t.name, *c)).collect(),
            uac_text: "UAC".into(),
            hosts: BTreeMap::new(),
            host_colors: BTreeMap::new(),
            errors: Vec::new(),
        }
    }

    /// The bare symbol of `t`.
    pub fn get(&self, t: &Topic) -> &str {
        self.icons.get(t.name).map_or(t.nerd, String::as_str)
    }

    /// The symbol of `t` as rendered: one space after it; nothing for an
    /// empty override (`uac = ""` leaves only the uac_text).
    pub fn icon(&self, t: &Topic) -> String {
        match self.get(t) {
            "" => String::new(),
            g => format!("{g} "),
        }
    }

    /// The UAC pane's name: symbol and uac_text, e.g. `<atom> UAC` or `u▲c`.
    pub fn uac_name(&self) -> String {
        format!("{}{}", self.icon(&UAC), self.uac_text)
    }

    /// The colour of `t`; `None` for a topic outside COLORS.
    pub fn color(&self, t: &Topic) -> Option<Rgb> {
        self.colors.get(t.name).copied()
    }

    /// The `$kind` token of agent kind topic `t`: its symbol, followed by
    /// CELL_BLANK when it ends in a private-use glyph; empty for an empty
    /// override.
    pub fn kind(&self, t: &Topic) -> String {
        let g = self.get(t);
        match g.chars().last() {
            Some(c) if is_private_use(c) => format!("{g}{CELL_BLANK}"),
            _ => g.to_string(),
        }
    }

    /// Compact workspace kind glyphs: no Nerd Font scaling or cell blank.
    /// Custom icon overrides still apply; base-set icons use text symbols.
    pub fn space_kind(&self, t: &Topic) -> &str {
        let glyph = self.get(t);
        if glyph != t.nerd && glyph != t.emoji {
            return glyph;
        }
        match t.name {
            "claude" => "✳",
            "codex" => "֍",
            "copilot" => "✈",
            "pi" => "π",
            _ => "◇",
        }
    }

    /// The `$host` token of `host`: its `[symbols.hosts]` label as written
    /// (e.g. `♃₃`, or `<icon> ganymede` to keep the name); just the name
    /// without an entry or with an empty label.
    pub fn host_label(&self, host: &str) -> String {
        match self.hosts.get(&host.to_lowercase()).map(String::as_str) {
            None | Some("") => host.to_string(),
            Some(l) => l.to_string(),
        }
    }

    /// The `[symbols.host_colors]` colour of `host`, if any.
    pub fn host_color(&self, host: &str) -> Option<Rgb> {
        self.host_colors.get(&host.to_lowercase()).copied()
    }

    /// Every host with a `[symbols.host_colors]` colour, by name: (name,
    /// `$host` label, colour).
    pub fn host_colors(&self) -> Vec<(&str, String, Rgb)> {
        self.host_colors
            .iter()
            .map(|(h, c)| (h.as_str(), self.host_label(h), *c))
            .collect()
    }
}

impl Default for Symbols {
    fn default() -> Self {
        Symbols::of("nerd")
    }
}

/// A private-use code point: where Nerd Font icons live.
pub fn is_private_use(c: char) -> bool {
    matches!(c, '\u{E000}'..='\u{F8FF}' | '\u{F0000}'..='\u{FFFFD}' | '\u{100000}'..='\u{10FFFD}')
}

/// `$XDG_CONFIG_HOME/vega/theme.toml`, default `~/.config/vega/theme.toml`.
pub fn file() -> Option<PathBuf> {
    let base = env::var("XDG_CONFIG_HOME")
        .ok()
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            env::var("HOME")
                .ok()
                .map(|h| PathBuf::from(h).join(".config"))
        })?;
    Some(base.join("vega").join("theme.toml"))
}

/// The symbols the theme text sets.
pub fn parse(text: &str) -> Symbols {
    let doc = match text.parse::<DocumentMut>() {
        Ok(d) => d,
        Err(e) => {
            let mut s = Symbols::default();
            let first = e.to_string().lines().next().unwrap_or("").to_string();
            s.errors.push(format!("theme.toml: not TOML: {first}"));
            return s;
        }
    };
    let Some(section) = doc.get("symbols") else {
        return Symbols::default();
    };
    let Some(section) = section.as_table_like() else {
        let mut s = Symbols::default();
        s.errors.push("theme.toml: [symbols] is not a table".into());
        return s;
    };
    let mut errors = Vec::new();
    let set = match section.get("set") {
        None => "nerd",
        Some(v) => match v.as_str() {
            Some(n) if SETS.contains(&n) => n,
            _ => {
                errors.push(format!(
                    "theme.toml: symbols.set {}: not one of {}",
                    v.to_string().trim(),
                    SETS.join(", ")
                ));
                "nerd"
            }
        },
    };
    let mut s = Symbols::of(set);
    for (key, _) in section.iter() {
        if !KEYS.contains(&key) {
            errors.push(format!(
                "theme.toml: symbols.{key}: unknown key ({})",
                KEYS.join(", ")
            ));
        }
    }
    match section.get("uac_text").map(|v| v.as_str()) {
        None => {}
        Some(Some(t)) if !t.trim().is_empty() => s.uac_text = t.trim().to_string(),
        Some(_) => errors.push("theme.toml: symbols.uac_text: not a non-empty string".into()),
    }
    match section.get("icons") {
        None => {}
        Some(Item::Table(_)) | Some(Item::Value(toml_edit::Value::InlineTable(_))) => {
            let icons = section.get("icons").and_then(Item::as_table_like);
            for (key, v) in icons.into_iter().flat_map(|t| t.iter()) {
                let Some(t) = TOPICS.iter().find(|t| t.name == key) else {
                    errors.push(format!(
                        "theme.toml: symbols.icons.{key}: unknown topic (vega theme lists them)"
                    ));
                    continue;
                };
                match v.as_str() {
                    Some(icon) => {
                        s.icons.insert(t.name, icon.to_string());
                    }
                    None => errors.push(format!("theme.toml: symbols.icons.{key}: not a string")),
                }
            }
        }
        Some(_) => errors.push("theme.toml: symbols.icons is not a table".into()),
    }
    match section.get("colors") {
        None => {}
        Some(Item::Table(_)) | Some(Item::Value(toml_edit::Value::InlineTable(_))) => {
            let colors = section.get("colors").and_then(Item::as_table_like);
            for (key, v) in colors.into_iter().flat_map(|t| t.iter()) {
                let Some((t, _)) = COLORS.iter().find(|(t, _)| t.name == key) else {
                    let names: Vec<&str> = COLORS.iter().map(|(t, _)| t.name).collect();
                    errors.push(format!(
                        "theme.toml: symbols.colors.{key}: not a colourable topic ({})",
                        names.join(", ")
                    ));
                    continue;
                };
                match v.as_str().and_then(Rgb::parse) {
                    Some(c) => {
                        s.colors.insert(t.name, c);
                    }
                    None => errors.push(format!(
                        "theme.toml: symbols.colors.{key} {}: not \"#RRGGBB\"",
                        v.to_string().trim()
                    )),
                }
            }
        }
        Some(_) => errors.push("theme.toml: symbols.colors is not a table".into()),
    }
    for (key, v) in host_table(section, "hosts", &mut errors) {
        match v.as_str() {
            Some(icon) => {
                s.hosts.insert(key.to_lowercase(), icon.to_string());
            }
            None => errors.push(format!("theme.toml: symbols.hosts.{key}: not a string")),
        }
    }
    for (key, v) in host_table(section, "host_colors", &mut errors) {
        match v.as_str().and_then(Rgb::parse) {
            Some(c) => {
                s.host_colors.insert(key.to_lowercase(), c);
            }
            None => errors.push(format!(
                "theme.toml: symbols.host_colors.{key} {}: not \"#RRGGBB\"",
                v.to_string().trim()
            )),
        }
    }
    s.errors = errors;
    s
}

/// The keys `[symbols]` may hold.
const KEYS: [&str; 6] = ["set", "uac_text", "icons", "colors", "hosts", "host_colors"];

/// The entries of the per-host table `[symbols.<name>]`; an error if it is
/// no table.
fn host_table<'a>(
    section: &'a dyn toml_edit::TableLike,
    name: &str,
    errors: &mut Vec<String>,
) -> Vec<(&'a str, &'a Item)> {
    match section.get(name) {
        None => Vec::new(),
        Some(i) => match i.as_table_like() {
            Some(t) => t.iter().collect(),
            None => {
                errors.push(format!("theme.toml: symbols.{name} is not a table"));
                Vec::new()
            }
        },
    }
}

type Cache = Option<(Instant, Arc<Symbols>)>;
static CACHE: Mutex<Cache> = Mutex::new(None);

/// The symbols in effect, re-read at most every READ_EVERY.
pub fn symbols() -> Arc<Symbols> {
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((at, s)) = cache.as_ref() {
        if at.elapsed() < READ_EVERY {
            return s.clone();
        }
    }
    let s = Arc::new(
        file()
            .and_then(|f| std::fs::read_to_string(f).ok())
            .map_or_else(Symbols::default, |t| parse(&t)),
    );
    *cache = Some((Instant::now(), s.clone()));
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_file_or_section_is_the_nerd_set() {
        assert_eq!(parse(""), Symbols::default());
        assert_eq!(parse("[colors]\nmode = \"dark\"\n"), Symbols::default());
        assert_eq!(Symbols::default().get(&NOTES), "\u{F01EE}");
        assert_eq!(Symbols::default().icon(&NOTES), "\u{F01EE} ");
    }

    #[test]
    fn set_picks_the_base_and_icons_override_topics() {
        let s = parse("[symbols]\nset = \"emoji\"\n[symbols.icons]\nnotes = \"M\"\n");
        assert_eq!(s.set, "emoji");
        assert!(s.errors.is_empty(), "{:?}", s.errors);
        assert_eq!(s.get(&NOTES), "M");
        assert_eq!(s.get(&PINNED), "\u{1F4CC}");
        let inline = parse("[symbols]\nicons = { uac = \"V\" }\n");
        assert_eq!(inline.get(&UAC), "V");
        assert_eq!(inline.get(&NOTES), "\u{F01EE}");
    }

    #[test]
    fn unknown_and_bad_entries_are_errors_the_rest_applies() {
        let s = parse(
            "[symbols]\nset = \"wingdings\"\ncolour = 1\n[symbols.icons]\nbell = \"B\"\nwaiting = 3\nnotes = \"N\"\n",
        );
        assert_eq!(s.set, "nerd");
        assert_eq!(s.get(&NOTES), "N");
        assert_eq!(s.get(&WAITING), "\u{F256}");
        assert_eq!(s.errors.len(), 4, "{:?}", s.errors);
        assert!(s
            .errors
            .iter()
            .any(|e| e.contains("symbols.icons.bell: unknown topic")));
        assert!(parse("[symbols").errors[0].starts_with("theme.toml: not TOML"));
    }

    #[test]
    fn colors_parse_strict_hex_and_report_the_rest() {
        assert_eq!(Rgb::parse("#2F7fd8"), Some(Rgb(0x2f, 0x7f, 0xd8)));
        assert_eq!(Rgb(0x2f, 0x7f, 0xd8).hex(), "#2f7fd8");
        for bad in ["2f7fd8", "#2f7", "#2f7fd8a", "#2g7fd8", "#", "", "#ÿÿÿ"] {
            assert_eq!(Rgb::parse(bad), None, "{bad}");
        }
        let d = Symbols::default();
        assert_eq!(d.color(&NOTES), Some(Rgb(0x2f, 0x7f, 0xd8)));
        assert_eq!(d.color(&PINNED), None);
        let s = parse(
            "[symbols]\nset = \"emoji\"\n[symbols.colors]\nnotes = \"#ff0000\"\nwaiting = \"red\"\npinned = \"#000000\"\ndecisions = 3\n",
        );
        assert_eq!(s.color(&NOTES), Some(Rgb(255, 0, 0)));
        assert_eq!(s.color(&WAITING), d.color(&WAITING));
        assert_eq!(s.color(&DECISIONS), d.color(&DECISIONS));
        assert_eq!(s.errors.len(), 3, "{:?}", s.errors);
        assert!(s
            .errors
            .iter()
            .any(|e| e.contains("symbols.colors.waiting \"red\": not")));
        assert!(s
            .errors
            .iter()
            .any(|e| e.contains("symbols.colors.pinned: not a colourable")));
        let inline = parse("[symbols]\ncolors = { uac = \"#ABCDEF\" }\n");
        assert!(inline.errors.is_empty(), "{:?}", inline.errors);
        assert_eq!(inline.color(&UAC), Some(Rgb(0xab, 0xcd, 0xef)));
        assert_eq!(
            parse("[symbols]\ncolors = 1\n").errors,
            ["theme.toml: symbols.colors is not a table"]
        );
    }

    #[test]
    fn uac_text_replaces_the_word_and_an_empty_icon_drops_the_space() {
        let s = parse("[symbols]\nuac_text = \"u\u{25B2}c\"\n[symbols.icons]\nuac = \"\"\n");
        assert!(s.errors.is_empty(), "{:?}", s.errors);
        assert_eq!(s.uac_name(), "u\u{25B2}c");
        assert_eq!(Symbols::default().uac_name(), "\u{F0768} UAC");
        assert_eq!(
            parse("[symbols]\nuac_text = \" \"\n").errors,
            vec!["theme.toml: symbols.uac_text: not a non-empty string".to_string()]
        );
    }

    #[test]
    fn hosts_map_names_to_icons_and_colours_with_errors() {
        let s = parse(
            "[symbols.hosts]\nAlpha = \"A\"\nbeta = \"\"\ngamma = 1\n[symbols.host_colors]\nalpha = \"#00ff00\"\nbeta = \"green\"\n",
        );
        assert_eq!(s.host_label("alpha"), "A");
        assert_eq!(s.host_label("beta"), "beta");
        assert_eq!(s.host_label("gamma"), "gamma");
        assert_eq!(s.host_label("delta"), "delta");
        assert_eq!(s.host_color("alpha"), Some(Rgb(0, 0xff, 0)));
        assert_eq!(s.host_color("beta"), None);
        assert_eq!(s.errors.len(), 2, "{:?}", s.errors);
        assert!(s
            .errors
            .iter()
            .any(|e| e == "theme.toml: symbols.hosts.gamma: not a string"));
        assert!(s
            .errors
            .iter()
            .any(|e| e.contains("symbols.host_colors.beta \"green\": not")));
        let inline = parse("[symbols]\nhosts = { alpha = \"A\" }\nhost_colors = 2\n");
        assert_eq!(inline.host_label("alpha"), "A");
        assert_eq!(
            inline.errors,
            ["theme.toml: symbols.host_colors is not a table"]
        );
    }

    #[test]
    fn agent_kinds_map_to_their_topic_and_override() {
        assert_eq!(agent_topic("claude"), &CLAUDE);
        assert_eq!(agent_topic("codex"), &CODEX);
        assert_eq!(agent_topic("copilot"), &COPILOT);
        assert_eq!(agent_topic("pi"), &PI);
        assert_eq!(agent_topic("opencode"), &OTHER_AGENT);
        let d = Symbols::default();
        assert_eq!(d.get(&CLAUDE), "\u{EC82}");
        assert_eq!(Symbols::of("emoji").get(&PI), "\u{1F967}");
        let s = parse("[symbols.icons]\nclaude = \"C\"\nother_agent = \"?\"\n");
        assert!(s.errors.is_empty(), "{:?}", s.errors);
        assert_eq!(s.get(&CLAUDE), "C");
        assert_eq!(s.get(&OTHER_AGENT), "?");
        assert_eq!(s.get(&CODEX), "\u{EC81}");
    }

    #[test]
    fn kind_token_keeps_a_private_use_glyph_one_cell() {
        let d = Symbols::default();
        assert_eq!(d.kind(&CLAUDE), "\u{EC82}\u{2800}");
        assert_eq!(d.kind(&PI), "\u{F03FF}\u{2800}");
        assert_eq!(Symbols::of("emoji").kind(&PI), "\u{1F967}");
        let s = parse("[symbols.icons]\nclaude = \"C\"\ncodex = \"\"\n");
        assert_eq!(s.kind(&CLAUDE), "C");
        assert_eq!(s.kind(&CODEX), "");
    }

    #[test]
    fn workspace_kinds_use_compact_text_and_keep_custom_overrides() {
        for symbols in [Symbols::default(), Symbols::of("emoji")] {
            for (topic, glyph) in [
                (&CLAUDE, "✳"),
                (&CODEX, "֍"),
                (&COPILOT, "✈"),
                (&PI, "π"),
                (&OTHER_AGENT, "◇"),
            ] {
                assert_eq!(symbols.space_kind(topic), glyph);
            }
        }
        let custom = parse("[symbols.icons]\nclaude = \"C\"\ncodex = \"\"\n");
        assert_eq!(custom.space_kind(&CLAUDE), "C");
        assert_eq!(custom.space_kind(&CODEX), "");
        assert_eq!(custom.kind(&PI), "\u{F03FF}\u{2800}");
    }

    #[test]
    fn agent_kinds_have_colours_with_override() {
        let d = Symbols::default();
        assert_eq!(d.color(&CLAUDE), Some(Rgb(0xc9, 0x64, 0x42)));
        assert_eq!(d.color(&OTHER_AGENT), Some(Rgb(0x80, 0x80, 0x80)));
        let s = parse("[symbols.colors]\ncodex = \"#00ff00\"\n");
        assert!(s.errors.is_empty(), "{:?}", s.errors);
        assert_eq!(s.color(&CODEX), Some(Rgb(0, 0xff, 0)));
        assert_eq!(s.color(&CLAUDE), d.color(&CLAUDE));
    }

    /// WCAG contrast of every default colour on Catppuccin Latte and
    /// Macchiato base and mantle.
    #[test]
    fn default_colours_keep_3_to_1_contrast() {
        fn lum(Rgb(r, g, b): Rgb) -> f64 {
            let ch = |v: u8| {
                let v = f64::from(v) / 255.0;
                if v <= 0.03928 {
                    v / 12.92
                } else {
                    ((v + 0.055) / 1.055).powf(2.4)
                }
            };
            0.2126 * ch(r) + 0.7152 * ch(g) + 0.0722 * ch(b)
        }
        let backgrounds = [
            Rgb(0xef, 0xf1, 0xf5),
            Rgb(0xe6, 0xe9, 0xef),
            Rgb(0x24, 0x27, 0x3a),
            Rgb(0x1e, 0x20, 0x30),
        ];
        for (t, c) in COLORS {
            for bg in backgrounds {
                let (a, b) = (lum(c), lum(bg));
                let ratio = (a.max(b) + 0.05) / (a.min(b) + 0.05);
                assert!(
                    ratio >= 3.0,
                    "{} {} on {}: {ratio:.2}",
                    t.name,
                    c.hex(),
                    bg.hex()
                );
            }
        }
    }

    #[test]
    fn topics_are_unique_and_complete_in_both_sets() {
        let mut names: Vec<&str> = TOPICS.iter().map(|t| t.name).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), TOPICS.len());
        for t in TOPICS {
            assert!(!t.nerd.is_empty() && !t.emoji.is_empty() && !t.about.is_empty());
            assert_eq!(t.nerd.chars().count(), 1, "{}", t.name);
        }
    }
}
