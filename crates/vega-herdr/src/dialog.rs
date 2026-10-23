//! Conservative classification of the visible Claude/Codex dialog, shared by mux and UAC.
use regex::Regex;
use serde::Serialize;
use std::sync::OnceLock;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Kind {
    Trust,
    HooksTrust,
    Update,
    Permission,
    Question,
    Unknown,
}
impl Kind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Trust => "trust",
            Self::HooksTrust => "hooks-trust",
            Self::Update => "update",
            Self::Permission => "permission",
            Self::Question => "question",
            Self::Unknown => "unknown",
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OptionLine {
    pub index: usize,
    pub text: String,
    pub selected: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Dialog {
    pub kind: Kind,
    pub agent: String,
    pub title: String,
    pub options: Vec<OptionLine>,
    pub raw: String,
}

/// Recognized dialogs require blocked/unknown state; unknown blocked screens remain unanswerable.
pub fn classify(screen: &str, agent: &str, status: &str) -> Option<Dialog> {
    if !matches!(status, "blocked" | "unknown") {
        return None;
    }
    let lines: Vec<&str> = screen.lines().map(str::trim).collect();
    let find = |patterns: &[&str]| {
        lines
            .iter()
            .position(|l| patterns.iter().any(|p| l.contains(p)))
    };
    let trust = find(&["Yes, I trust this folder", "Trust this folder?"]);
    let hooks = find(&["Hooks need review"]);
    // Codex's startup "Update available · 0.162.0 → 0.162.1" screen; its
    // in-chat banner says "Update available!" without these options.
    let update = find(&["Skip until next version"]);
    let permission = find(&[
        "Do you want to proceed?",
        "Would you like to run the following command?",
        "Do you want to allow this connection?",
        "requests your input",
    ]);
    static QUESTION: OnceLock<Regex> = OnceLock::new();
    let question = QUESTION.get_or_init(|| Regex::new(r"^(?:←\s+[☐☒]|Question \d+/\d+)").unwrap());
    let q = lines.iter().position(|l| {
        question.is_match(l)
            || (l.contains("Enter to select · Tab/Arrow keys to navigate")
                || l.contains("Enter to select · ↑/↓ to navigate"))
    });
    let (kind, mut title_at) = if let Some(i) = trust {
        (Kind::Trust, i)
    } else if let Some(i) = hooks {
        (Kind::HooksTrust, i)
    } else if let Some(i) = update {
        (Kind::Update, find(&["Update available"]).unwrap_or(i))
    } else if let Some(i) = permission {
        (Kind::Permission, i)
    } else if let Some(i) = q {
        (Kind::Question, i)
    } else if status == "blocked" {
        (
            Kind::Unknown,
            lines.iter().position(|l| !l.is_empty()).unwrap_or(0),
        )
    } else {
        return None;
    };
    static OPTIONS: OnceLock<Regex> = OnceLock::new();
    let re = OPTIONS.get_or_init(|| Regex::new(r"^\s*([❯›]?)\s*(\d+)\.\s+(.*)").unwrap());
    let options: Vec<OptionLine> = lines
        .iter()
        .filter_map(|l| {
            let c = re.captures(l)?;
            Some(OptionLine {
                index: c[2].parse().ok()?,
                text: c[3].to_string(),
                selected: !c[1].is_empty(),
            })
        })
        .collect();
    if kind == Kind::Question {
        // The actual question follows the tab bar / Question N/M, before the first option.
        if let Some(first) = lines.iter().position(|l| re.is_match(l)) {
            if let Some(i) = (0..first)
                .rev()
                .find(|&i| !lines[i].is_empty() && !question.is_match(lines[i]))
            {
                title_at = i;
            }
        }
    } else if kind == Kind::Trust {
        if let Some(i) = find(&["Quick safety check:", "Trust this folder?"]) {
            title_at = i;
        }
    }
    // Claude trust has unnumbered choices.
    let options = if kind == Kind::Trust && options.is_empty() {
        lines
            .iter()
            .filter(|l| l.contains("No, exit") || l.contains("Yes, I trust this folder"))
            .enumerate()
            .map(|(i, l)| OptionLine {
                index: i + 1,
                text: l.trim_start_matches('❯').trim().to_string(),
                selected: l.starts_with('❯'),
            })
            .collect()
    } else {
        options
    };
    Some(Dialog {
        kind,
        agent: agent.into(),
        title: lines.get(title_at).unwrap_or(&"").to_string(),
        options,
        raw: screen.into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn captured_screens() {
        let cases = [
            ("claude", "Quick safety check: Is this a project you\ncreated or one you trust?\n❯ No, exit\n Yes, I trust this folder\nEnter to confirm · Esc to cancel", Kind::Trust, 2),
            ("codex", "Trust this folder? Codex can read, edit, and run\n› 1. Trust and continue\n 2. Quit\nenter continue · esc quit", Kind::Trust, 2),
            ("codex", "Hooks need review\n› 1. Review hooks\n2. Trust all and continue", Kind::HooksTrust, 2),
            ("codex", "  Update available · 0.162.0 → 0.162.1\n  Release notes: https://github.com/openai/codex/releases/latest\n› 1. Update now (runs `npm install -g @openai/codex`)\n  2. Skip\n  3. Skip until next version\n  enter continue · esc skip", Kind::Update, 3),
            ("claude", "Bash command\ncurl -sI https://example.com\nDo you want to proceed?\n❯ 1. Yes\n2. Yes, and don't ask again for: curl *\n3. Yes, and switch to auto mode\n4. No\nEsc to cancel · Tab to amend", Kind::Permission, 4),
            ("codex", "Would you like to run the following command?\n$ touch probe.txt\n› 1. Yes, proceed (y)\n2. Yes, and don't ask again (p)\n3. No (esc)\nPress enter to confirm or esc to cancel", Kind::Permission, 3),
            ("claude", "←  ☐ Colour  ☐ Pets  ✔ Submit  →\nWhich colour do you prefer?\n❯ 1. Red\n     Red\n2. Blue\n3. Type something.\n4. Chat about this\nEnter to select · Tab/Arrow keys to navigate · Esc to cancel", Kind::Question, 4),
            ("codex", "Question 1/1 (1 unanswered)\nWhich colour do you prefer?\n› 1. Red                Choose Red.\n2. Blue               Choose Blue.\n3. None of the above  Optionally, add details in notes (tab)\ntab to add notes | enter to submit answer | esc to interrupt", Kind::Question, 3),
        ];
        for (agent, screen, kind, n) in cases {
            let d = classify(screen, agent, "blocked").unwrap();
            assert_eq!(d.kind, kind);
            assert_eq!(d.options.len(), n);
            assert!(d.options[0].selected);
            if kind == Kind::Question {
                assert_eq!(d.title, "Which colour do you prefer?");
            }
            if kind == Kind::Update {
                assert_eq!(d.title, "Update available · 0.162.0 → 0.162.1");
            }
        }
    }
    #[test]
    fn live_single_question() {
        let d = classify(" ☐ Colour\nWhich colour do you prefer?\n❯ 1. Red\n2. Blue\n3. Type something.\n4. Chat about this\nEnter to select · ↑/↓ to navigate · Esc to cancel", "claude", "blocked").unwrap();
        assert_eq!(d.kind, Kind::Question);
        assert_eq!(d.title, "Which colour do you prefer?");
    }
    #[test]
    fn safe_precedence_and_empty_states() {
        let s = "Question 1/1\nDo you want to proceed?\n❯ 1. Yes";
        assert_eq!(
            classify(s, "claude", "blocked").unwrap().kind,
            Kind::Permission
        );
        assert!(classify(s, "claude", "idle").is_none());
        assert!(classify("normal screen", "codex", "unknown").is_none());
        assert_eq!(
            classify("unrecognized prompt", "pi", "blocked")
                .unwrap()
                .kind,
            Kind::Unknown
        );
        assert_eq!(
            classify("Hooks need review", "codex", "unknown")
                .unwrap()
                .kind,
            Kind::HooksTrust
        );
    }
}
