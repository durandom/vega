use std::process::ExitCode;
use vega_herdr::dialog::{classify, Dialog};
use vega_herdr::{cell, herdr, herdr_text, print_failure, str_of, Failure};

pub fn read(target: &str) -> Result<(String, Option<Dialog>), Failure> {
    let info = herdr(&["agent", "get", target])?;
    let a = &info["result"]["agent"];
    let pane = str_of(a, "pane_id").to_string();
    let raw = herdr_text(&["pane", "read", &pane, "--source", "visible"])?;
    Ok((
        pane,
        classify(&raw, str_of(a, "agent"), str_of(a, "agent_status")),
    ))
}

pub fn show(target: &str, json: bool) -> ExitCode {
    match read(target) {
        Ok((_, d)) => {
            if json {
                println!(
                    "{}",
                    serde_json::to_string(&d).expect("serializable dialog")
                );
            } else if let Some(d) = &d {
                println!(
                    "dialog{{kind,agent,title}}:\n  {},{},{}",
                    d.kind.as_str(),
                    cell(&d.agent),
                    cell(&d.title)
                );
                println!("options[{}]{{index,selected,text}}:", d.options.len());
                for o in &d.options {
                    println!("  {},{},{}", o.index, o.selected, cell(&o.text));
                }
            } else {
                println!("dialog: none");
            }
            ExitCode::from(if d.is_some() { 0 } else { 1 })
        }
        Err(f) => {
            print_failure(&f);
            ExitCode::from(1)
        }
    }
}

use std::thread;
use std::time::{Duration, Instant};
use vega_herdr::dialog::Kind;

fn refusal(message: impl Into<String>) -> Failure {
    Failure::new(
        "usage",
        message,
        "only question dialogs may be answered; trust and permission stay with the Slayer",
    )
}

fn question(d: Option<Dialog>) -> Result<Dialog, Failure> {
    let d = d.ok_or_else(|| refusal("no question dialog is present"))?;
    if d.kind != Kind::Question {
        return Err(refusal(format!("refusing {} dialog", d.kind.as_str())));
    }
    Ok(d)
}

/// Identity excludes the cursor and incidental screen chrome. Any changed question is refused.
fn same_question(a: &Dialog, b: &Dialog) -> bool {
    a.kind == b.kind
        && a.title == b.title
        && a.agent == b.agent
        && a.options
            .iter()
            .map(|o| (o.index, &o.text))
            .eq(b.options.iter().map(|o| (o.index, &o.text)))
}

trait Pane {
    fn dialog(&mut self) -> Result<Option<Dialog>, Failure>;
    fn after_dialog(&mut self) -> Result<Option<Dialog>, Failure> {
        self.dialog()
    }
    fn key(&mut self, key: &str) -> Result<(), Failure>;
    fn text(&mut self, text: &str) -> Result<(), Failure>;
}
struct LivePane(String);
impl Pane for LivePane {
    fn dialog(&mut self) -> Result<Option<Dialog>, Failure> {
        read(&self.0).map(|(_, d)| d)
    }
    fn after_dialog(&mut self) -> Result<Option<Dialog>, Failure> {
        // Verify visible text, independently of herdr's lagging status detector.
        let info = herdr(&["agent", "get", &self.0])?;
        let raw = herdr_text(&["pane", "read", &self.0, "--source", "visible"])?;
        Ok(classify(
            &raw,
            str_of(&info["result"]["agent"], "agent"),
            "blocked",
        ))
    }
    fn key(&mut self, key: &str) -> Result<(), Failure> {
        herdr_text(&["pane", "send-keys", &self.0, key]).map(|_| ())
    }
    fn text(&mut self, text: &str) -> Result<(), Failure> {
        herdr_text(&["pane", "send-text", &self.0, text]).map(|_| ())
    }
}
fn checked(pane: &mut impl Pane, original: &Dialog) -> Result<Dialog, Failure> {
    let fresh = question(pane.dialog()?)?;
    if !same_question(original, &fresh) {
        return Err(refusal("the question changed; inspect it again"));
    }
    Ok(fresh)
}
fn selected(d: &Dialog) -> Result<usize, Failure> {
    let indices: Vec<usize> = d
        .options
        .iter()
        .enumerate()
        .filter(|(_, o)| o.selected)
        .map(|(i, _)| i)
        .collect();
    if indices.len() != 1 {
        return Err(refusal("cannot identify one selected option"));
    }
    Ok(indices[0])
}
fn navigate(pane: &mut impl Pane, original: &Dialog, index: usize) -> Result<(), Failure> {
    let dest = original
        .options
        .iter()
        .position(|o| o.index == index)
        .ok_or_else(|| refusal("option is absent from the current question"))?;
    for _ in 0..=original.options.len() {
        let fresh = checked(pane, original)?;
        let at = selected(&fresh)?;
        if at == dest {
            return Ok(());
        }
        pane.key(if at < dest { "down" } else { "up" })?;
        let start = Instant::now();
        loop {
            thread::sleep(Duration::from_millis(30));
            let after = checked(pane, original)?;
            if selected(&after)? != at {
                break;
            }
            if start.elapsed() > Duration::from_secs(2) {
                return Err(refusal("selection did not move; no answer submitted"));
            }
        }
    }
    Err(refusal("could not reach the requested option"))
}
fn verify_moved(pane: &mut impl Pane, original: &Dialog) -> Result<(), Failure> {
    let start = Instant::now();
    loop {
        thread::sleep(Duration::from_millis(30));
        match pane.after_dialog()? {
            None => return Ok(()),
            Some(d) if !same_question(original, &d) => return Ok(()),
            _ => {}
        }
        if start.elapsed() >= Duration::from_secs(5) {
            return Err(Failure::new(
                "answer_unverified",
                "the question screen has not moved on",
                "inspect with `vega mux dialog <target>`; do not blindly retry",
            ));
        }
    }
}
fn act(pane: &mut impl Pane, option: Option<usize>, text: Option<&str>) -> Result<(), Failure> {
    let original = question(pane.dialog()?)?;
    if let Some(index) = option {
        navigate(pane, &original, index)?;
        let fresh = checked(pane, &original)?;
        if fresh.options[selected(&fresh)?].index != index {
            return Err(refusal("selection changed before submission"));
        }
        pane.key("enter")?;
        verify_moved(pane, &original)
    } else if let Some(text) = text {
        let editor = original
            .options
            .iter()
            .find(|o| o.text == "Type something.")
            .filter(|_| original.agent == "claude")
            .ok_or_else(|| {
                refusal("this question's text editor is not recognized; use --option N")
            })?;
        let index = editor.index;
        navigate(pane, &original, index)?;
        let fresh = checked(pane, &original)?;
        if fresh.options[selected(&fresh)?].index != index {
            return Err(refusal("answer editor is not selected"));
        }
        // Claude's Type something row is itself the editor: Enter on an empty row declines.
        pane.text(text)?;
        let start = Instant::now();
        let typed = loop {
            thread::sleep(Duration::from_millis(30));
            let d = question(pane.dialog()?)?;
            let mut expected = original.clone();
            expected
                .options
                .iter_mut()
                .find(|o| o.index == index)
                .unwrap()
                .text = text.into();
            if same_question(&d, &expected) && d.options[selected(&d)?].index == index {
                break d;
            }
            if !same_question(&d, &original) || start.elapsed() >= Duration::from_secs(2) {
                return Err(refusal("could not verify the typed answer; no Enter sent"));
            }
        };
        let fresh = checked(pane, &typed)?;
        if fresh.options[selected(&fresh)?].index != index {
            return Err(refusal("answer editor changed before submission"));
        }
        pane.key("enter")?;
        verify_moved(pane, &typed)
    } else {
        Err(refusal("pass --option N or --text"))
    }
}
pub fn answer(target: &str, option: Option<usize>, text: Option<&str>) -> Result<(), Failure> {
    if text.is_some_and(|t| t.trim().is_empty() || t.chars().any(char::is_control)) {
        return Err(refusal(
            "answer text must be nonempty and contain no control characters",
        ));
    }
    let (pane, _) = read(target)?;
    act(&mut LivePane(pane), option, text)?;
    println!("answer: question screen moved on");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use vega_herdr::dialog::OptionLine;

    fn fixture() -> Dialog {
        Dialog {
            kind: Kind::Question,
            agent: "claude".into(),
            title: "Which colour?".into(),
            raw: String::new(),
            options: vec![
                OptionLine {
                    index: 1,
                    text: "Red".into(),
                    selected: true,
                },
                OptionLine {
                    index: 2,
                    text: "Blue".into(),
                    selected: false,
                },
                OptionLine {
                    index: 3,
                    text: "Type something.".into(),
                    selected: false,
                },
            ],
        }
    }
    struct Fake {
        d: Option<Dialog>,
        reads: usize,
        replace_on_read: Option<(usize, Dialog)>,
        keys: Vec<String>,
        texts: Vec<String>,
    }
    impl Fake {
        fn new() -> Self {
            Self {
                d: Some(fixture()),
                reads: 0,
                replace_on_read: None,
                keys: vec![],
                texts: vec![],
            }
        }
    }
    impl Pane for Fake {
        fn dialog(&mut self) -> Result<Option<Dialog>, Failure> {
            self.reads += 1;
            if let Some((n, d)) = &self.replace_on_read {
                if self.reads == *n {
                    self.d = Some(d.clone());
                }
            }
            Ok(self.d.clone())
        }
        fn key(&mut self, key: &str) -> Result<(), Failure> {
            self.keys.push(key.into());
            if key == "enter" {
                self.d = None;
            } else {
                let d = self.d.as_mut().unwrap();
                let at = selected(d).unwrap();
                let next = if key == "down" { at + 1 } else { at - 1 };
                for (i, o) in d.options.iter_mut().enumerate() {
                    o.selected = i == next;
                }
            }
            Ok(())
        }
        fn text(&mut self, text: &str) -> Result<(), Failure> {
            self.texts.push(text.into());
            let d = self.d.as_mut().unwrap();
            let at = selected(d).unwrap();
            d.options[at].text = text.into();
            Ok(())
        }
    }
    #[test]
    fn every_nonquestion_refused_without_input() {
        for kind in [
            Kind::Trust,
            Kind::HooksTrust,
            Kind::Update,
            Kind::Permission,
            Kind::Unknown,
        ] {
            for text in [None, Some("y1p")] {
                let mut p = Fake::new();
                p.d.as_mut().unwrap().kind = kind.clone();
                assert_eq!(
                    act(&mut p, if text.is_some() { None } else { Some(1) }, text)
                        .unwrap_err()
                        .code,
                    "usage"
                );
                assert!(p.keys.is_empty() && p.texts.is_empty());
            }
        }
        let mut p = Fake::new();
        p.d = None;
        assert!(act(&mut p, Some(1), None).is_err());
        assert!(p.keys.is_empty());
    }
    #[test]
    fn arrows_and_enter_only() {
        let mut p = Fake::new();
        act(&mut p, Some(2), None).unwrap();
        assert_eq!(p.keys, ["down", "enter"]);
        assert!(p.texts.is_empty());
        let mut p = Fake::new();
        p.d.as_mut()
            .unwrap()
            .options
            .iter_mut()
            .enumerate()
            .for_each(|(i, o)| o.selected = i == 2);
        act(&mut p, Some(1), None).unwrap();
        assert_eq!(p.keys, ["up", "up", "enter"]);
    }
    #[test]
    fn rechecks_before_moving_and_before_enter() {
        for (read, option, expected) in [(2, 2, vec![]), (4, 2, vec!["down"]), (3, 1, vec![])] {
            let mut p = Fake::new();
            let mut changed = fixture();
            changed.kind = Kind::Permission;
            p.replace_on_read = Some((read, changed));
            assert!(act(&mut p, Some(option), None).is_err());
            assert_eq!(p.keys, expected);
        }
        let mut p = Fake::new();
        let mut changed = fixture();
        changed.title = "Another question?".into();
        p.replace_on_read = Some((2, changed));
        assert!(act(&mut p, Some(1), None).is_err());
        assert!(p.keys.is_empty());
    }
    #[test]
    fn validates_option_and_selection_before_input() {
        let mut p = Fake::new();
        assert!(act(&mut p, Some(99), None).is_err());
        assert!(p.keys.is_empty());
        p.d.as_mut().unwrap().options[0].selected = false;
        assert!(act(&mut p, Some(1), None).is_err());
        assert!(p.keys.is_empty());
    }
    #[test]
    fn text_goes_only_to_verified_editor() {
        let mut p = Fake::new();
        act(&mut p, None, Some("Purple")).unwrap();
        assert_eq!(p.keys, ["down", "down", "enter"]);
        assert_eq!(p.texts, ["Purple"]);
        let mut p = Fake::new();
        let mut changed = fixture();
        changed.kind = Kind::Trust;
        p.replace_on_read = Some((7, changed));
        assert!(act(&mut p, None, Some("Purple")).is_err());
        assert!(p.texts.is_empty());
        assert!(!p.keys.iter().any(|k| k == "enter"));
    }
    #[test]
    fn text_must_be_visible_before_enter() {
        let mut p = Fake::new();
        let mut changed = fixture();
        changed.kind = Kind::Permission;
        p.replace_on_read = Some((8, changed));
        assert!(act(&mut p, None, Some("Purple")).is_err());
        assert_eq!(p.texts, ["Purple"]);
        assert!(!p.keys.iter().any(|k| k == "enter"));
    }
    #[test]
    fn unchanged_visible_question_is_not_verified() {
        struct Stalled;
        impl Pane for Stalled {
            // Status already became idle, but the visible question has not moved.
            fn dialog(&mut self) -> Result<Option<Dialog>, Failure> {
                Ok(None)
            }
            fn after_dialog(&mut self) -> Result<Option<Dialog>, Failure> {
                Ok(Some(fixture()))
            }
            fn key(&mut self, _: &str) -> Result<(), Failure> {
                panic!("verification must not send keys")
            }
            fn text(&mut self, _: &str) -> Result<(), Failure> {
                panic!("verification must not send text")
            }
        }
        assert_eq!(
            verify_moved(&mut Stalled, &fixture()).unwrap_err().code,
            "answer_unverified"
        );
    }
}
