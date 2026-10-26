//! Uber-UAC post messages (docs/design/uber-uac.md §3): plain `vega post`
//! mail with a `Vega-Kind: uber.*` header and a JSON body.
//!
//! `validate` checks one body: `vega post send --kind` runs it before it
//! publishes, so `send` never stores an oversized or malformed body. Mail
//! published by other means is checked on receipt where a receiver exists:
//! the UAC-host vegad parses `uber.answer` with `validate` (vega-lhc.209.5).
//! The cap is on the body's bytes, the field limits on characters. Unknown
//! fields are refused.
//!
//! Semantic checks that need the open ask (tag, pick, epoch, L4 approval)
//! belong to the UAC-host vegad (§4), not here.
//!
//! Shared by both hosts: `shown` (the AskUserQuestion rendering and
//! `shown_sha256`) and `tag`. The Uber host's records: `received` (asks it
//! received), `capture` (the Slayer's input, and the answer built from it)
//! and `audit`.

pub mod audit;
pub mod capture;
pub mod received;
pub mod shown;

use serde::{Deserialize, Serialize};
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

pub mod tag;

/// The mail header that names the kind.
pub const HEADER: &str = "Vega-Kind";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Ask,
    Answer,
    Applied,
    Withdraw,
    Brief,
}

impl Kind {
    pub const ALL: [Kind; 5] = [
        Kind::Ask,
        Kind::Answer,
        Kind::Applied,
        Kind::Withdraw,
        Kind::Brief,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Ask => "uber.ask",
            Kind::Answer => "uber.answer",
            Kind::Applied => "uber.applied",
            Kind::Withdraw => "uber.withdraw",
            Kind::Brief => "uber.brief",
        }
    }

    pub fn parse(s: &str) -> Option<Kind> {
        Kind::ALL.into_iter().find(|k| k.as_str() == s)
    }

    /// The body's size cap in bytes.
    pub fn cap(self) -> usize {
        match self {
            Kind::Ask => 1024,
            Kind::Answer => 2048,
            Kind::Applied | Kind::Withdraw => 256,
            Kind::Brief => 128,
        }
    }
}

/// Why a body was refused: a stable code and a one-line reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    pub code: &'static str,
    pub message: String,
}

fn bad(message: impl Into<String>) -> Refusal {
    Refusal {
        code: "uber_malformed",
        message: message.into(),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Level {
    L3,
    L4,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Opt {
    pub label: String,
    pub detail: String,
}

/// UAC → Uber: one decision for the Slayer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Ask {
    pub id: String,
    pub project: String,
    pub bead: String,
    pub level: Level,
    /// Why it cannot wait; `None` for a batched ask.
    pub urgent: Option<String>,
    pub question: String,
    pub context: String,
    pub options: Vec<Opt>,
    /// 0-based index into `options`.
    pub recommend: usize,
    pub why: String,
    /// RFC 3339 or `no-rush`.
    pub deadline: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    /// A `PostToolUse` capture of `AskUserQuestion`.
    AskUserQuestion,
    /// A `UserPromptSubmit` capture: a typed prompt.
    UserPrompt,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Capture {
    pub source: Source,
    /// RFC 3339.
    pub at: String,
    pub sha256: String,
}

/// Uber → UAC: the Slayer's answer to exactly one ask.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Answer {
    pub id: String,
    pub tag: String,
    pub bead: String,
    pub epoch: u64,
    pub pick: Option<String>,
    pub approval: bool,
    pub notes: Option<String>,
    pub slayer_text: String,
    pub shown_sha256: Option<String>,
    pub capture: Capture,
}

/// UAC vegad → Uber: `applied` or `refused:<code>`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Applied {
    pub id: String,
    pub result: String,
}

/// UAC → Uber: the ask is answered elsewhere or obsolete.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Withdraw {
    pub id: String,
    pub reason: String,
}

/// Uber → all live UACs: what happened since (RFC 3339).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Brief {
    pub since: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Msg {
    Ask(Ask),
    Answer(Answer),
    Applied(Applied),
    Withdraw(Withdraw),
    Brief(Brief),
}

/// A new ask id, `<bead>~<nuid>`: readable, and unguessable by the nuid.
pub fn new_id(bead: &str) -> Result<String, Refusal> {
    check_bead(bead)?;
    Ok(format!("{bead}~{}", nuid::next()))
}

/// The bead of an id `<bead>~<nuid>`.
pub fn id_bead(id: &str) -> Result<&str, Refusal> {
    let (bead, nuid) = id
        .split_once('~')
        .ok_or_else(|| bad(format!("id {id:?} is not <bead>~<nuid>")))?;
    check_bead(bead)?;
    if nuid.len() != 22 || !nuid.chars().all(|c| c.is_ascii_alphanumeric()) {
        return Err(bad(format!(
            "id {id:?}: the nuid is not 22 letters or digits"
        )));
    }
    Ok(bead)
}

fn check_bead(bead: &str) -> Result<(), Refusal> {
    let ok = !bead.is_empty()
        && bead.len() <= 64
        && bead
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_'));
    ok.then_some(())
        .ok_or_else(|| bad(format!("bead {bead:?} is not a bead id")))
}

/// `field` has 1..=`max` characters (0 allowed when `empty`), no control
/// characters; `multiline` allows line feeds and tabs, nothing else.
fn text(field: &str, v: &str, max: usize, empty: bool, multiline: bool) -> Result<(), Refusal> {
    let n = v.chars().count();
    if n > max {
        return Err(bad(format!("{field} has {n} characters, max {max}")));
    }
    if !empty && v.trim().is_empty() {
        return Err(bad(format!("{field} is empty")));
    }
    let allowed = |c: char| multiline && matches!(c, '\n' | '\t');
    if v.chars().any(|c| c.is_control() && !allowed(c)) {
        return Err(bad(format!(
            "{field} contains a {} control character",
            if multiline {
                "non-newline"
            } else {
                "line break or"
            }
        )));
    }
    Ok(())
}

fn rfc3339(field: &str, v: &str) -> Result<(), Refusal> {
    OffsetDateTime::parse(v, &Rfc3339)
        .map(drop)
        .map_err(|_| bad(format!("{field} {v:?} is not RFC 3339")))
}

fn sha256(field: &str, v: &str) -> Result<(), Refusal> {
    let ok = v.len() == 64 && v.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f'));
    ok.then_some(())
        .ok_or_else(|| bad(format!("{field} is not 64 lowercase hex digits")))
}

/// An id whose bead is the message's `bead`.
fn id_of(id: &str, bead: &str) -> Result<(), Refusal> {
    if id_bead(id)? != bead {
        return Err(bad(format!("id {id:?} is not of bead {bead}")));
    }
    Ok(())
}

/// The ask's tag for the phone: `#` and 1-4 of `[a-z0-9]`, e.g. `#a7`.
pub fn valid_tag(tag: &str) -> bool {
    tag.strip_prefix('#').is_some_and(|t| {
        (1..=4).contains(&t.len())
            && t.chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
    })
}

/// A refusal code in `refused:<code>`: `[a-z0-9_]`, 1-40.
fn valid_code(code: &str) -> bool {
    (1..=40).contains(&code.len())
        && code
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

impl Ask {
    fn check(&self) -> Result<(), Refusal> {
        check_bead(&self.bead)?;
        id_of(&self.id, &self.bead)?;
        text("project", &self.project, 200, false, false)?;
        if let Some(u) = &self.urgent {
            text("urgent", u, 120, false, false)?;
        }
        text("question", &self.question, 120, false, false)?;
        text("context", &self.context, 280, true, false)?;
        if !(2..=3).contains(&self.options.len()) {
            return Err(bad(format!("{} options, need 2 or 3", self.options.len())));
        }
        for (i, o) in self.options.iter().enumerate() {
            text(&format!("options[{i}].label"), &o.label, 40, false, false)?;
            text(&format!("options[{i}].detail"), &o.detail, 120, true, false)?;
            if self.options[..i].iter().any(|p| p.label == o.label) {
                return Err(bad(format!("option label {:?} is given twice", o.label)));
            }
        }
        if self.recommend >= self.options.len() {
            return Err(bad(format!(
                "recommend {} is not an option index (0-based)",
                self.recommend
            )));
        }
        text("why", &self.why, 120, true, false)?;
        if self.deadline != "no-rush" {
            rfc3339("deadline", &self.deadline)?;
        }
        Ok(())
    }
}

impl Answer {
    fn check(&self) -> Result<(), Refusal> {
        check_bead(&self.bead)?;
        id_of(&self.id, &self.bead)?;
        if !valid_tag(&self.tag) {
            return Err(bad(format!(
                "tag {:?} is not # and 1-4 of [a-z0-9]",
                self.tag
            )));
        }
        if self.epoch == 0 {
            return Err(bad("epoch is 0"));
        }
        if let Some(p) = &self.pick {
            text("pick", p, 40, false, false)?;
        }
        if let Some(n) = &self.notes {
            text("notes", n, 1024, true, true)?;
        }
        text("slayer_text", &self.slayer_text, 1024, false, true)?;
        if let Some(h) = &self.shown_sha256 {
            sha256("shown_sha256", h)?;
        }
        rfc3339("capture.at", &self.capture.at)?;
        sha256("capture.sha256", &self.capture.sha256)
    }
}

impl Applied {
    fn check(&self) -> Result<(), Refusal> {
        id_bead(&self.id)?;
        match self.result.strip_prefix("refused:") {
            None if self.result == "applied" => Ok(()),
            Some(code) if valid_code(code) => Ok(()),
            _ => Err(bad(format!(
                "result {:?} is not applied or refused:<code>",
                self.result
            ))),
        }
    }
}

/// Checks a body of `kind`: the cap first, then the JSON shape, then the
/// fields. The typed message on success.
pub fn validate(kind: Kind, body: &[u8]) -> Result<Msg, Refusal> {
    if body.len() > kind.cap() {
        return Err(Refusal {
            code: "uber_too_large",
            message: format!(
                "{} body has {} bytes, max {}",
                kind.as_str(),
                body.len(),
                kind.cap()
            ),
        });
    }
    let shape = |e: serde_json::Error| bad(format!("{} body: {e}", kind.as_str()));
    Ok(match kind {
        Kind::Ask => {
            let m: Ask = serde_json::from_slice(body).map_err(shape)?;
            m.check()?;
            Msg::Ask(m)
        }
        Kind::Answer => {
            let m: Answer = serde_json::from_slice(body).map_err(shape)?;
            m.check()?;
            Msg::Answer(m)
        }
        Kind::Applied => {
            let m: Applied = serde_json::from_slice(body).map_err(shape)?;
            m.check()?;
            Msg::Applied(m)
        }
        Kind::Withdraw => {
            let m: Withdraw = serde_json::from_slice(body).map_err(shape)?;
            id_bead(&m.id)?;
            text("reason", &m.reason, 120, false, false)?;
            Msg::Withdraw(m)
        }
        Kind::Brief => {
            let m: Brief = serde_json::from_slice(body).map_err(shape)?;
            rfc3339("since", &m.since)?;
            Msg::Brief(m)
        }
    })
}

/// `validate` for a header value: an unknown kind is refused too.
pub fn validate_kind(kind: &str, body: &[u8]) -> Result<Msg, Refusal> {
    let k = Kind::parse(kind).ok_or_else(|| Refusal {
        code: "uber_kind",
        message: format!(
            "kind {kind:?} is not one of {}",
            Kind::ALL.map(Kind::as_str).join(", ")
        ),
    })?;
    validate(k, body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    const ID: &str = "vega-lhc.209~AbCdEfGhIjKlMnOpQrStUv";
    const HEX: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    fn ask() -> Value {
        json!({
            "id": ID, "project": "github.com/durandom/vega", "bead": "vega-lhc.209",
            "level": "L4", "urgent": null, "question": "Push main?",
            "context": "Gate green.", "recommend": 0, "why": "Tested.",
            "options": [{"label": "yes", "detail": "push"}, {"label": "no", "detail": "wait"}],
            "deadline": "2026-10-26T14:00:00+01:00"
        })
    }

    fn answer() -> Value {
        json!({
            "id": ID, "tag": "#a7", "bead": "vega-lhc.209", "epoch": 1793019257322u64,
            "pick": "yes", "approval": true, "notes": null, "slayer_text": "yes",
            "shown_sha256": HEX,
            "capture": {"source": "ask_user_question", "at": "2026-10-26T13:00:00Z", "sha256": HEX}
        })
    }

    fn check(kind: Kind, v: &Value) -> Result<Msg, Refusal> {
        validate(kind, &serde_json::to_vec(v).unwrap())
    }

    fn refused(kind: Kind, v: Value, needle: &str) {
        let e = check(kind, &v).expect_err(needle);
        assert!(e.message.contains(needle), "{needle}: {}", e.message);
    }

    #[test]
    fn kinds_and_caps() {
        for k in Kind::ALL {
            assert_eq!(Kind::parse(k.as_str()), Some(k));
        }
        assert_eq!(Kind::parse("uber.tell"), None);
        assert_eq!(
            validate_kind("uber.tell", b"{}").unwrap_err().code,
            "uber_kind"
        );
        assert_eq!(
            [
                Kind::Ask,
                Kind::Answer,
                Kind::Applied,
                Kind::Withdraw,
                Kind::Brief
            ]
            .map(Kind::cap),
            [1024, 2048, 256, 256, 128]
        );
    }

    #[test]
    fn ids_are_bead_and_nuid() {
        let id = new_id("vega-lhc.209.3").unwrap();
        assert_eq!(id_bead(&id).unwrap(), "vega-lhc.209.3");
        assert_ne!(id, new_id("vega-lhc.209.3").unwrap());
        assert!(new_id("a b").is_err());
        assert!(id_bead("vega-lhc.209").is_err());
        assert!(id_bead("vega-lhc.209~short").is_err());
    }

    #[test]
    fn valid_bodies_parse() {
        assert!(matches!(check(Kind::Ask, &ask()), Ok(Msg::Ask(_))));
        let mut a = ask();
        a["deadline"] = json!("no-rush");
        a["urgent"] = json!("production down");
        a["options"]
            .as_array_mut()
            .unwrap()
            .push(json!({"label": "later", "detail": ""}));
        assert!(check(Kind::Ask, &a).is_ok());
        assert!(matches!(check(Kind::Answer, &answer()), Ok(Msg::Answer(_))));
        let mut typed = answer();
        typed["pick"] = Value::Null;
        typed["approval"] = json!(false);
        typed["shown_sha256"] = Value::Null;
        typed["slayer_text"] = json!("#a7 nein,\nerst morgen");
        typed["capture"]["source"] = json!("user_prompt");
        assert!(check(Kind::Answer, &typed).is_ok());
        for r in ["applied", "refused:answered"] {
            assert!(check(Kind::Applied, &json!({"id": ID, "result": r})).is_ok());
        }
        assert!(check(Kind::Withdraw, &json!({"id": ID, "reason": "obsolete"})).is_ok());
        assert!(check(Kind::Brief, &json!({"since": "2026-10-26T08:00:00Z"})).is_ok());
    }

    #[test]
    fn oversized_bodies_are_refused_before_parsing() {
        let mut a = ask();
        a["context"] = json!("x".repeat(2000));
        let e = check(Kind::Ask, &a).unwrap_err();
        assert_eq!(e.code, "uber_too_large");
        // Within every field limit, still over the byte cap: umlauts are 2 bytes.
        let mut a = ask();
        a["question"] = json!("ä".repeat(120));
        a["context"] = json!("ö".repeat(280));
        a["why"] = json!("ü".repeat(120));
        assert_eq!(check(Kind::Ask, &a).unwrap_err().code, "uber_too_large");
        let e = check(Kind::Brief, &json!({"since": "x".repeat(200)})).unwrap_err();
        assert_eq!(e.code, "uber_too_large");
    }

    #[test]
    fn malformed_asks_are_refused() {
        let with = |k: &str, v: Value| {
            let mut a = ask();
            a[k] = v;
            a
        };
        refused(
            Kind::Ask,
            with("question", json!("q".repeat(121))),
            "max 120",
        );
        refused(Kind::Ask, with("question", json!(" ")), "question is empty");
        refused(Kind::Ask, with("question", json!("a\nb")), "line break");
        refused(
            Kind::Ask,
            with("context", json!("c".repeat(281))),
            "max 280",
        );
        refused(Kind::Ask, with("level", json!("L2")), "unknown variant");
        refused(
            Kind::Ask,
            with("recommend", json!(2)),
            "not an option index",
        );
        refused(Kind::Ask, with("deadline", json!("tomorrow")), "RFC 3339");
        refused(Kind::Ask, with("bead", json!("other-1")), "is not of bead");
        refused(Kind::Ask, with("extra", json!(1)), "unknown field");
        let one = json!([{"label": "yes", "detail": ""}]);
        refused(Kind::Ask, with("options", one), "need 2 or 3");
        let twice = json!([{"label": "yes", "detail": ""}, {"label": "yes", "detail": ""}]);
        refused(Kind::Ask, with("options", twice), "given twice");
        let long = json!([{"label": "l".repeat(41), "detail": ""}, {"label": "n", "detail": ""}]);
        refused(Kind::Ask, with("options", long), "options[0].label");
        let mut missing = ask();
        missing.as_object_mut().unwrap().remove("why");
        refused(Kind::Ask, missing, "missing field");
        assert_eq!(
            validate(Kind::Ask, b"not json").unwrap_err().code,
            "uber_malformed"
        );
    }

    #[test]
    fn malformed_answers_and_others_are_refused() {
        let with = |k: &str, v: Value| {
            let mut a = answer();
            a[k] = v;
            a
        };
        refused(
            Kind::Answer,
            with("slayer_text", json!("")),
            "slayer_text is empty",
        );
        for c in ["\u{0}", "\u{1b}[2J", "a\r\nb"] {
            refused(
                Kind::Answer,
                with("slayer_text", json!(c)),
                "non-newline control",
            );
            refused(Kind::Answer, with("notes", json!(c)), "non-newline control");
        }
        assert!(check(Kind::Answer, &with("notes", json!("a\n\tb"))).is_ok());
        refused(Kind::Answer, with("tag", json!("a7")), "tag");
        refused(Kind::Answer, with("tag", json!("#A7")), "tag");
        refused(Kind::Answer, with("epoch", json!(0)), "epoch is 0");
        refused(
            Kind::Answer,
            with("shown_sha256", json!("abc")),
            "shown_sha256",
        );
        refused(Kind::Answer, with("id", json!("x~y")), "nuid");
        let mut a = answer();
        a["capture"]["source"] = json!("model");
        refused(Kind::Answer, a, "unknown variant");
        let r = |r: &str| json!({"id": ID, "result": r});
        refused(Kind::Applied, r("done"), "applied or refused");
        refused(Kind::Applied, r("refused:"), "applied or refused");
        refused(Kind::Applied, r("refused:Bad Code"), "applied or refused");
        refused(
            Kind::Withdraw,
            json!({"id": ID, "reason": ""}),
            "reason is empty",
        );
        refused(Kind::Brief, json!({"since": "yesterday"}), "RFC 3339");
    }
}
