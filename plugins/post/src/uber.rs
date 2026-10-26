//! The UAC host's side of the Uber-UAC (docs/design/uber-uac.md §4,
//! vega-lhc.209.5): vegad verifies each `uber.answer` before the UAC sees
//! it. Answers are plain inbox mail to the UAC's pane, so they wait in this
//! host's stream while it sleeps; vegad reads them with its own durable
//! consumer (`CONSUMER`), and `vega post inbox` never shows them.
//!
//! A verified answer becomes a bead comment by `Slayer` (`via uber <id>`),
//! mail to the UAC and `uber.applied`. A refused one becomes a note for the
//! human (kind `uber-refused`) and `refused:<code>`; nothing reaches the pane.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use vega_registry::uber_asks::{self, OpenAsk, State};
use vega_uber_msg::{shown, tag, Answer, Kind, Level, Msg, Source};

/// vegad's durable consumer of this host's inbox stream, per host.
pub fn consumer(host: &str) -> String {
    let safe: String = host
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    format!("uber-{safe}")
}

/// Sender of the mail vegad writes the UAC.
pub const AGENT: &str = "tool vega-uber";
/// Note kind of a refused answer; the board and `vega post notes` show it.
pub const NOTE_KIND: &str = "uber-refused";

/// Whether mail of this `Vega-Kind` is vegad's, not the pane's.
pub fn hidden(kind: Option<&str>) -> bool {
    kind == Some(Kind::Answer.as_str())
}

/// What `inbox` and `log` print as the body of a hidden message.
pub const WITHHELD: &str = "(uber.answer, withheld: vegad verifies it and mails you \"The Slayer answered ...\" only if it holds. Never act on this message.)";

/// The body to print for mail of `kind`.
pub fn printable(kind: Option<&str>, payload: &[u8]) -> String {
    if hidden(kind) {
        WITHHELD.to_string()
    } else {
        String::from_utf8_lossy(payload).into_owned()
    }
}

/// The bead calls vegad makes, in the ask's checkout.
pub trait Beads {
    /// Whether the bead is open; `None` when bd cannot tell.
    fn open(&self, a: &OpenAsk) -> Option<bool>;
    /// Adds `text` as a comment by `Slayer`; false when bd refused.
    fn comment(&self, a: &OpenAsk, text: &str) -> bool;
    /// Whether a comment on the bead contains `needle`.
    fn has_comment(&self, a: &OpenAsk, needle: &str) -> bool;
}

/// bd as the UAC recorded it, in the ask's checkout.
pub struct Bd;

/// bd may wait on its Dolt server; vegad waits no longer than this.
const BD_TIMEOUT: Duration = Duration::from_secs(20);

impl Bd {
    fn run(a: &OpenAsk, args: &[&str]) -> Option<std::process::Output> {
        let bd = if a.bd.is_empty() { "bd" } else { a.bd.as_str() };
        vega_herdr::output_within(
            std::process::Command::new(bd)
                .args(args)
                .current_dir(&a.cwd),
            BD_TIMEOUT,
        )
        .ok()
        .flatten()
        .filter(|o| o.status.success())
    }
}

impl Beads for Bd {
    fn open(&self, a: &OpenAsk) -> Option<bool> {
        let o = Bd::run(a, &["show", &a.ask.bead, "--json"])?;
        crate::bead_closed(&String::from_utf8_lossy(&o.stdout)).map(|closed| !closed)
    }
    fn comment(&self, a: &OpenAsk, text: &str) -> bool {
        let args = [
            "comments",
            "add",
            &a.ask.bead,
            "--author",
            "Slayer",
            "--",
            text,
        ];
        Bd::run(a, &args).is_some()
    }
    fn has_comment(&self, a: &OpenAsk, needle: &str) -> bool {
        Bd::run(a, &["comments", &a.ask.bead, "--json"])
            .is_some_and(|o| String::from_utf8_lossy(&o.stdout).contains(needle))
    }
}

/// What vegad does with one received answer.
#[derive(Debug, PartialEq)]
pub struct Outcome {
    /// `uber.applied` body for the sender, when the id is known.
    pub reply: Option<String>,
    /// Mail for the UAC: (pane, text).
    pub mail: Option<(String, String)>,
    /// Note for the human: (title, body).
    pub note: Option<(String, String)>,
    /// One line for the log.
    pub log: String,
}

/// §4 checks 1-4 and the L4 rule; the refusal code on failure.
pub fn verify(
    ans: &Answer,
    from_host: &str,
    from_pane: &str,
    ask: Option<&OpenAsk>,
    beads: &dyn Beads,
) -> Result<(), &'static str> {
    // 1. An open ask with this id, its bead still open.
    let a = ask.ok_or("no_ask")?;
    // 4. Not answered or withdrawn before: the first verified answer wins.
    match a.state {
        State::Open => {}
        State::Answered => return Err("answered"),
        State::Withdrawn => return Err("withdrawn"),
    }
    match beads.open(a) {
        Some(true) => {}
        Some(false) => return Err("bead_closed"),
        None => return Err("bead_unknown"),
    }
    // 2. The claim the ask was (re)sent to, live or not.
    let to = &a.sent_to;
    if vega_registry::token(from_host) != vega_registry::token(&to.host) || from_pane != to.pane {
        return Err("wrong_sender");
    }
    if ans.epoch != to.epoch {
        return Err("wrong_epoch");
    }
    // 3. Tag, pick, rendering; `validate` refused an empty `slayer_text`.
    if !tag::matches(&a.ask.id, &ans.tag) {
        return Err("wrong_tag");
    }
    if let Some(p) = &ans.pick {
        if !a.ask.options.iter().any(|o| &o.label == p) {
            return Err("bad_pick");
        }
    }
    if let Some(h) = &ans.shown_sha256 {
        if *h != shown::sha256(&a.ask, &ans.tag) {
            return Err("shown_mismatch");
        }
    }
    if a.ask.level == Level::L4
        && ans.approval
        && (ans.capture.source != Source::AskUserQuestion
            || ans.pick.is_none()
            || ans.shown_sha256.is_none())
    {
        return Err("l4_approval");
    }
    Ok(())
}

fn applied(id: &str, result: &str) -> String {
    serde_json::json!({"id": id, "result": result}).to_string()
}

/// The ask id of a body that did not parse, if it carries a valid one.
fn raw_id(body: &[u8]) -> Option<String> {
    let v: serde_json::Value = serde_json::from_slice(body).ok()?;
    let id = v["id"].as_str()?;
    vega_uber_msg::id_bead(id).ok()?;
    Some(id.to_string())
}

fn refused(
    id: Option<&str>,
    code: &str,
    why: &str,
    from_host: &str,
    from_pane: &str,
    to_pane: &str,
) -> Outcome {
    let what = id.unwrap_or("(no id)");
    let title = format!("uber answer refused ({code}): {what}");
    let body = format!(
        "vegad refused an uber.answer for {to_pane} from {from_pane} on {from_host}: {code}{}{why}.\nThe UAC was not told. The Uber sees refused:{code}.",
        if why.is_empty() { "" } else { ", " }
    );
    Outcome {
        reply: id.map(|i| applied(i, &format!("refused:{code}"))),
        mail: None,
        note: Some((title, body)),
        log: format!("uber: refused {what} from {from_pane} on {from_host}: {code} {why}"),
    }
}

/// The UAC's mail: the Slayer's words verbatim and what they authorise.
fn uac_mail(a: &OpenAsk, ans: &Answer) -> String {
    let bead = &a.ask.bead;
    let scope = match (a.ask.level, ans.approval, &ans.pick) {
        (Level::L4, true, Some(p)) => {
            format!("\nL4 approval of option \"{p}\" exactly as asked, nothing wider.")
        }
        (Level::L4, _, _) => "\nL4: this is not an approval. Do not act on it as one; ask again with the options it implies.".to_string(),
        _ => String::new(),
    };
    format!(
        "The Slayer answered {bead} ({}) via uber: {}{scope}\nAct on it, then close {bead} with the answer as reason. To ask back: bd comments add {bead} \"<question>\" and say so in one line; not by mail.",
        a.ask.question, ans.slayer_text
    )
}

/// One `uber.answer` at stream sequence `seq` from `from_host`/`from_pane`
/// to `to_pane`: verify it against the open asks in `asks`, apply or refuse.
/// What it owes ends in the outbox before this returns. An applied answer
/// is first written there as an intent, so a crash in between resumes on
/// redelivery instead of refusing it as answered. `None`: handled before.
#[allow(clippy::too_many_arguments)]
pub fn receive(
    asks: &Path,
    seq: u64,
    body: &[u8],
    from_host: &str,
    from_pane: &str,
    to_pane: &str,
    beads: &dyn Beads,
    now: i64,
) -> Result<Option<Outcome>, String> {
    let out = outbox(asks);
    match load(&out, seq) {
        Some(p) if p.applying => return resume(asks, &out, p, beads, now, true).map(Some),
        Some(_) => return Ok(None),
        None => {}
    }
    let refuse = |id: Option<&str>, code: &str, why: &str| {
        let o = refused(id, code, why, from_host, from_pane, to_pane);
        let reply = o
            .reply
            .clone()
            .map(|b| (from_host.into(), from_pane.into(), b));
        let note = o.note.clone().map(|(t, b)| (to_pane.into(), t, b));
        let p = Pending {
            note,
            ..Pending::owed(seq, None, reply)
        };
        save(&out, &p).map_err(|e| e.to_string())?;
        Ok(Some(o))
    };
    let ans = match vega_uber_msg::validate(Kind::Answer, body) {
        Ok(Msg::Answer(a)) => a,
        Ok(_) => return refuse(None, "malformed", "not an answer"),
        Err(r) => {
            let code = r.code.strip_prefix("uber_").unwrap_or(r.code);
            return refuse(raw_id(body).as_deref(), code, &r.message);
        }
    };
    let ask = uber_asks::load(asks, &ans.id);
    if let Err(code) = verify(&ans, from_host, from_pane, ask.as_ref(), beads) {
        return refuse(Some(&ans.id), code, "");
    }
    let a = ask.expect("verified");
    let intent = Pending {
        seq,
        mail: Some((a.uac_pane.clone(), uac_mail(&a, &ans))),
        reply: Some((
            from_host.into(),
            from_pane.into(),
            applied(&ans.id, "applied"),
        )),
        note: None,
        applying: true,
        id: ans.id.clone(),
        comment: format!("{}\n\nvia uber {}", ans.slayer_text, ans.id),
    };
    save(&out, &intent).map_err(|e| e.to_string())?;
    resume(asks, &out, intent, beads, now, false).map(Some)
}

/// Applies an intent: close the ask, add the comment unless it is there,
/// then leave only the mail and reply owed. `again`: a redelivery after a
/// crash, where the ask may be closed already.
fn resume(
    asks: &Path,
    out: &Path,
    p: Pending,
    beads: &dyn Beads,
    now: i64,
    again: bool,
) -> Result<Outcome, String> {
    let (from_host, from_pane) = p
        .reply
        .as_ref()
        .map(|(h, pn, _)| (h.clone(), pn.clone()))
        .unwrap_or_default();
    let to_pane = p
        .mail
        .as_ref()
        .map(|(pn, _)| pn.clone())
        .unwrap_or_default();
    let refuse = |code: &str, why: &str| {
        let o = refused(Some(&p.id), code, why, &from_host, &from_pane, &to_pane);
        let reply = o
            .reply
            .clone()
            .map(|b| (from_host.clone(), from_pane.clone(), b));
        let note = o.note.clone().map(|(t, b)| (to_pane.clone(), t, b));
        let owed = Pending {
            note,
            ..Pending::owed(p.seq, None, reply)
        };
        save(out, &owed).map_err(|e| e.to_string())?;
        Ok(o)
    };
    let Some(mut a) = uber_asks::load(asks, &p.id) else {
        return refuse("no_ask", "gone while applying");
    };
    // Under the lock, so a withdraw or a second answer cannot slip between.
    match uber_asks::close(asks, &p.id, State::Answered, now) {
        Ok(true) => {}
        Ok(false) if again && a.state == State::Answered => {}
        Ok(false) => return refuse("answered", "closed meanwhile"),
        Err(e) => return refuse("state_failed", &e.to_string()),
    }
    let marker = format!("via uber {}", p.id);
    if !beads.has_comment(&a, &marker) && !beads.comment(&a, &p.comment) {
        // The Slayer's words did not land: the ask opens again, so the
        // Uber, told refused:bd_failed, can send the answer once more.
        a.state = State::Open;
        a.closed_at = None;
        let why = match uber_asks::save(asks, &a) {
            Ok(()) => "bd did not add the comment".to_string(),
            Err(e) => format!("bd did not add the comment; reopening the ask failed: {e}"),
        };
        return refuse("bd_failed", &why);
    }
    save(out, &Pending::owed(p.seq, p.mail.clone(), p.reply.clone())).map_err(|e| e.to_string())?;
    Ok(Outcome {
        reply: p.reply.map(|(_, _, b)| b),
        mail: p.mail,
        note: None,
        log: format!(
            "uber: applied {} from {from_pane} on {from_host} to {}{}",
            p.id,
            a.ask.bead,
            if again { " (resumed)" } else { "" }
        ),
    })
}

/// What vegad still owes for one answer, by its stream sequence: written
/// before the answer is acked and kept until sent, so a restart loses
/// neither the UAC's mail, the Uber's `uber.applied` nor a refusal's note.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Pending {
    pub seq: u64,
    /// (pane, text) for the UAC on this host.
    pub mail: Option<(String, String)>,
    /// (host, pane, body) of the `uber.applied` for the sender.
    pub reply: Option<(String, String, String)>,
    /// (pane, title, body) of a refusal's note for the human.
    #[serde(default)]
    pub note: Option<(String, String, String)>,
    /// An intent: the ask is being closed and commented; not sent yet.
    #[serde(default)]
    pub applying: bool,
    /// The answer's ask id and bead comment, for an intent.
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub comment: String,
}

impl Pending {
    /// Something is left to send.
    pub fn owes(&self) -> bool {
        !self.applying && (self.mail.is_some() || self.reply.is_some() || self.note.is_some())
    }

    fn owed(
        seq: u64,
        mail: Option<(String, String)>,
        reply: Option<(String, String, String)>,
    ) -> Pending {
        Pending {
            seq,
            mail,
            reply,
            note: None,
            applying: false,
            id: String::new(),
            comment: String::new(),
        }
    }
}

/// The outbox next to the open-ask store (`.../uber/asks`).
pub fn outbox(asks: &Path) -> PathBuf {
    asks.parent().unwrap_or(asks).join("outbox")
}

fn pending_file(outbox: &Path, seq: u64) -> PathBuf {
    outbox.join(format!("{seq}.json"))
}

fn load(outbox: &Path, seq: u64) -> Option<Pending> {
    serde_json::from_slice(&std::fs::read(pending_file(outbox, seq)).ok()?).ok()
}

/// Whether the answer at `seq` was handled already (a redelivery).
#[cfg(test)]
pub fn handled(outbox: &Path, seq: u64) -> bool {
    load(outbox, seq).is_some_and(|p| !p.applying)
}

/// Writes `p` atomically; an entry with nothing left is kept as the
/// `handled` mark until `prune`.
pub fn save(outbox: &Path, p: &Pending) -> std::io::Result<()> {
    std::fs::create_dir_all(outbox)?;
    let file = pending_file(outbox, p.seq);
    let tmp = file.with_extension("tmp");
    std::fs::write(&tmp, serde_json::to_vec(p).map_err(std::io::Error::other)?)?;
    std::fs::rename(tmp, file)
}

/// Entries with something left to send, oldest first.
pub fn unsent(outbox: &Path) -> Vec<Pending> {
    let mut all: Vec<Pending> = std::fs::read_dir(outbox)
        .into_iter()
        .flatten()
        .filter_map(|e| {
            let path = e.ok()?.path();
            (path.extension()? == "json").then_some(())?;
            serde_json::from_slice(&std::fs::read(path).ok()?).ok()
        })
        .filter(Pending::owes)
        .collect();
    all.sort_by_key(|p| p.seq);
    all
}

/// Removes sent entries older than `keep`: past the consumer's ack wait no
/// redelivery can come.
pub fn prune(outbox: &Path, keep: Duration) {
    for e in std::fs::read_dir(outbox).into_iter().flatten().flatten() {
        let path = e.path();
        let old = e
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age >= keep);
        let sent = std::fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice::<Pending>(&b).ok())
            .is_some_and(|p| !p.applying && !p.owes());
        if old && sent {
            let _ = std::fs::remove_file(path);
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::cell::RefCell;
    use vega_registry::uber_asks::SentTo;
    use vega_uber_msg::{Ask, Capture, Opt};

    struct Fake {
        open: Option<bool>,
        ok: bool,
        comments: RefCell<Vec<String>>,
    }

    impl Fake {
        fn new() -> Fake {
            Fake {
                open: Some(true),
                ok: true,
                comments: RefCell::new(Vec::new()),
            }
        }
    }

    impl Beads for Fake {
        fn open(&self, _: &OpenAsk) -> Option<bool> {
            self.open
        }
        fn comment(&self, _: &OpenAsk, text: &str) -> bool {
            if self.ok {
                self.comments.borrow_mut().push(text.to_string());
            }
            self.ok
        }
        fn has_comment(&self, _: &OpenAsk, needle: &str) -> bool {
            self.comments.borrow().iter().any(|c| c.contains(needle))
        }
    }

    pub(crate) const ID: &str = "vega-x.1~AbCdEfGhIjKlMnOpQrStUv";

    pub(crate) fn ask(level: Level) -> OpenAsk {
        OpenAsk {
            schema: 1,
            ask: Ask {
                id: ID.into(),
                project: "github.com/durandom/vega".into(),
                bead: "vega-x.1".into(),
                level,
                urgent: None,
                question: "Push main?".into(),
                context: "".into(),
                options: vec![
                    Opt {
                        label: "yes".into(),
                        detail: "push now".into(),
                    },
                    Opt {
                        label: "no".into(),
                        detail: "".into(),
                    },
                ],
                recommend: 0,
                why: "".into(),
                deadline: "no-rush".into(),
            },
            uac_pane: "w1:p1".into(),
            uac_host: "proteus".into(),
            cwd: "/nonexistent".into(),
            bd: String::new(),
            sent_to: SentTo {
                host: "ganymede".into(),
                pane: "w9:p1".into(),
                epoch: 7,
            },
            sent_at: 1,
            state: State::Open,
            closed_at: None,
        }
    }

    fn tag_of() -> String {
        tag::of(ID, &[]).unwrap()
    }

    pub(crate) fn answer(level: Level) -> Answer {
        let a = ask(level);
        Answer {
            id: ID.into(),
            tag: tag_of(),
            bead: "vega-x.1".into(),
            epoch: 7,
            pick: Some("yes".into()),
            approval: level == Level::L4,
            notes: None,
            slayer_text: "yes".into(),
            shown_sha256: Some(shown::sha256(&a.ask, &tag_of())),
            capture: Capture {
                source: Source::AskUserQuestion,
                at: "2026-10-26T15:00:00Z".into(),
                sha256: "0".repeat(64),
            },
        }
    }

    fn setup(level: Level) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        uber_asks::save(&dir.path().join("asks"), &ask(level)).unwrap();
        dir
    }

    /// Each call is a new message: the next stream sequence.
    fn run(dir: &Path, ans: &Answer, beads: &Fake) -> Outcome {
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let body = serde_json::to_vec(ans).unwrap();
        receive(dir, seq, &body, "ganymede", "w9:p1", "w1:p1", beads, 100)
            .unwrap()
            .expect("a new message")
    }

    fn refused_with(o: &Outcome, code: &str) {
        assert_eq!(
            o.reply.as_deref(),
            Some(applied(ID, &format!("refused:{code}")).as_str()),
            "{o:?}"
        );
        assert!(o.mail.is_none(), "nothing reaches the pane: {o:?}");
        let (title, _) = o.note.as_ref().expect("a note for the human");
        assert!(title.contains(code), "{title}");
    }

    #[test]
    fn a_verified_answer_is_applied_once() {
        for level in [Level::L3, Level::L4] {
            let dir = setup(level);
            let beads = Fake::new();
            let o = run(&dir.path().join("asks"), &answer(level), &beads);
            assert_eq!(o.reply.as_deref(), Some(applied(ID, "applied").as_str()));
            assert!(o.note.is_none());
            let (pane, text) = o.mail.unwrap();
            assert_eq!(pane, "w1:p1");
            assert!(text.contains("The Slayer answered vega-x.1"), "{text}");
            if level == Level::L4 {
                assert!(text.contains("L4 approval of option \"yes\""), "{text}");
            }
            assert_eq!(
                *beads.comments.borrow(),
                vec![format!("yes\n\nvia uber {ID}")]
            );
            let a = uber_asks::load(&dir.path().join("asks"), ID).unwrap();
            assert_eq!(a.state, State::Answered);
            // 4. A second answer, even a valid one, is refused.
            refused_with(
                &run(&dir.path().join("asks"), &answer(level), &beads),
                "answered",
            );
            assert_eq!(beads.comments.borrow().len(), 1);
        }
    }

    #[test]
    fn an_answer_queued_across_a_takeover_still_applies() {
        // The live claim moved on; the ask's recorded claim is what counts.
        let dir = setup(Level::L3);
        let o = run(&dir.path().join("asks"), &answer(Level::L3), &Fake::new());
        assert!(o.mail.is_some(), "{o:?}");
    }

    #[test]
    fn l4_comment_without_approval_is_applied_as_no_approval() {
        let dir = setup(Level::L4);
        let mut ans = answer(Level::L4);
        ans.approval = false;
        ans.pick = None;
        ans.shown_sha256 = None;
        ans.capture.source = Source::UserPrompt;
        ans.slayer_text = "#a7 not yet, ask legal".into();
        let (_, text) = run(&dir.path().join("asks"), &ans, &Fake::new())
            .mail
            .unwrap();
        assert!(text.contains("not an approval"), "{text}");
    }

    #[test]
    fn every_refusal_case_stays_out_of_the_pane() {
        type Case = (&'static str, fn(&mut Answer, &mut OpenAsk, &mut Fake));
        let cases: Vec<Case> = vec![
            ("withdrawn", |_, a, _| a.state = State::Withdrawn),
            ("answered", |_, a, _| a.state = State::Answered),
            ("bead_closed", |_, _, b| b.open = Some(false)),
            ("bead_unknown", |_, _, b| b.open = None),
            ("wrong_sender", |_, a, _| a.sent_to.host = "titan".into()),
            ("wrong_sender", |_, a, _| a.sent_to.pane = "w9:p2".into()),
            ("wrong_epoch", |n, _, _| n.epoch = 8),
            // A resend to a new epoch replaces the recorded claim.
            ("wrong_epoch", |_, a, _| a.sent_to.epoch = 9),
            ("wrong_tag", |n, _, _| {
                let h = tag::hash(ID);
                let other = "0123456789abcdefghijklmnopqrstuvwxyz"
                    .chars()
                    .find(|c| !h.starts_with(*c))
                    .unwrap();
                n.tag = format!("#{other}");
            }),
            ("bad_pick", |n, _, _| {
                n.pick = Some("yes (Recommended)".into())
            }),
            ("shown_mismatch", |n, _, _| {
                n.shown_sha256 = Some("f".repeat(64))
            }),
            ("shown_mismatch", |_, a, _| {
                a.ask.options[0].detail = "reworded".into()
            }),
            ("l4_approval", |n, a, _| {
                a.ask.level = Level::L4;
                n.shown_sha256 = Some(shown::sha256(&a.ask, &n.tag));
                n.approval = true;
                n.capture.source = Source::UserPrompt;
            }),
            ("l4_approval", |n, a, _| {
                a.ask.level = Level::L4;
                n.shown_sha256 = Some(shown::sha256(&a.ask, &n.tag));
                n.approval = true;
                n.pick = None;
            }),
            ("l4_approval", |n, a, _| {
                a.ask.level = Level::L4;
                n.approval = true;
                n.shown_sha256 = None;
            }),
            ("bd_failed", |_, _, b| b.ok = false),
        ];
        for (code, f) in cases {
            let dir = tempfile::tempdir().unwrap();
            let (mut ans, mut a, mut beads) = (answer(Level::L3), ask(Level::L3), Fake::new());
            f(&mut ans, &mut a, &mut beads);
            uber_asks::save(&dir.path().join("asks"), &a).unwrap();
            let o = run(&dir.path().join("asks"), &ans, &beads);
            refused_with(&o, code);
            if code != "bd_failed" {
                assert!(beads.comments.borrow().is_empty(), "{code}");
            }
            let state = uber_asks::load(&dir.path().join("asks"), ID).unwrap().state;
            assert_eq!(state, a.state, "{code}: a refusal leaves the ask as it was");
        }
    }

    #[test]
    fn no_open_ask_and_bad_bodies_are_refused() {
        let empty = tempfile::tempdir().unwrap();
        refused_with(
            &run(&empty.path().join("asks"), &answer(Level::L3), &Fake::new()),
            "no_ask",
        );

        let dir = setup(Level::L3);
        let mut ans = answer(Level::L3);
        ans.slayer_text = " ".into();
        let o = run(&dir.path().join("asks"), &ans, &Fake::new());
        refused_with(&o, "malformed");

        let o = receive(
            &dir.path().join("asks"),
            900,
            b"not json",
            "ganymede",
            "w9:p1",
            "w1:p1",
            &Fake::new(),
            1,
        )
        .unwrap()
        .unwrap();
        assert_eq!(o.reply, None, "no id, no reply");
        assert!(o.mail.is_none() && o.note.is_some());

        let big = format!("{{\"id\":\"{ID}\",\"x\":\"{}\"}}", "a".repeat(3000));
        let o = receive(
            &dir.path().join("asks"),
            901,
            big.as_bytes(),
            "ganymede",
            "w9:p1",
            "w1:p1",
            &Fake::new(),
            1,
        )
        .unwrap()
        .unwrap();
        refused_with(&o, "too_large");
        assert_eq!(
            uber_asks::load(&dir.path().join("asks"), ID).unwrap().state,
            State::Open
        );
    }

    #[test]
    fn a_crash_while_applying_resumes_on_redelivery() {
        // After the intent (ask open), after the close, after the comment.
        for crash in 0..3 {
            let dir = setup(Level::L3);
            let asks = dir.path().join("asks");
            let out = outbox(&asks);
            let beads = Fake::new();
            let ans = answer(Level::L3);
            let comment = format!("yes\n\nvia uber {ID}");
            let intent = Pending {
                seq: 7,
                mail: Some(("w1:p1".into(), "mail".into())),
                reply: Some(("ganymede".into(), "w9:p1".into(), applied(ID, "applied"))),
                note: None,
                applying: true,
                id: ID.into(),
                comment: comment.clone(),
            };
            save(&out, &intent).unwrap();
            assert!(
                !handled(&out, 7) && unsent(&out).is_empty(),
                "an intent is not sent"
            );
            if crash >= 1 {
                assert!(uber_asks::close(&asks, ID, State::Answered, 1).unwrap());
            }
            if crash >= 2 {
                beads.comment(&ask(Level::L3), &comment);
            }
            let body = serde_json::to_vec(&ans).unwrap();
            let o = receive(&asks, 7, &body, "ganymede", "w9:p1", "w1:p1", &beads, 2)
                .unwrap()
                .unwrap();
            assert_eq!(
                o.reply.as_deref(),
                Some(applied(ID, "applied").as_str()),
                "{crash}"
            );
            assert!(o.log.contains("resumed"), "{o:?}");
            assert_eq!(
                *beads.comments.borrow(),
                vec![comment],
                "{crash}: one comment"
            );
            assert_eq!(uber_asks::load(&asks, ID).unwrap().state, State::Answered);
            assert!(handled(&out, 7));
            assert_eq!(unsent(&out).len(), 1, "{crash}: mail and reply owed");
            let again = receive(&asks, 7, &body, "ganymede", "w9:p1", "w1:p1", &beads, 3);
            assert_eq!(again, Ok(None), "handled once");
        }
    }

    #[test]
    fn a_refusal_note_waits_in_the_outbox_across_a_redelivery() {
        let dir = setup(Level::L3);
        let asks = dir.path().join("asks");
        let out = outbox(&asks);
        let mut ans = answer(Level::L3);
        ans.epoch = 8;
        let body = serde_json::to_vec(&ans).unwrap();
        let beads = Fake::new();
        let o = receive(&asks, 11, &body, "ganymede", "w9:p1", "w1:p1", &beads, 1)
            .unwrap()
            .unwrap();
        refused_with(&o, "wrong_epoch");
        // vegad exits before it sends anything; the answer comes again.
        let again = receive(&asks, 11, &body, "ganymede", "w9:p1", "w1:p1", &beads, 2);
        assert_eq!(again, Ok(None));
        let owed = unsent(&out);
        assert_eq!(owed.len(), 1, "{owed:?}");
        let (pane, title, _) = owed[0].note.clone().expect("the note is owed");
        assert_eq!(pane, "w1:p1");
        assert!(title.contains("wrong_epoch"), "{title}");
        assert!(owed[0].reply.is_some());
    }

    #[test]
    fn the_outbox_keeps_what_is_owed_and_marks_handled_answers() {
        let root = tempfile::tempdir().unwrap();
        let out = outbox(&root.path().join("asks"));
        assert_eq!(out, root.path().join("outbox"));
        assert!(unsent(&out).is_empty() && !handled(&out, 3));
        let p = Pending::owed(
            3,
            Some(("w1:p1".into(), "hi".into())),
            Some(("ganymede".into(), "w9:p1".into(), "{}".into())),
        );
        save(&out, &p).unwrap();
        assert!(handled(&out, 3));
        assert_eq!(unsent(&out), vec![p.clone()]);
        let sent = Pending {
            mail: None,
            reply: None,
            ..p
        };
        save(&out, &sent).unwrap();
        assert!(unsent(&out).is_empty());
        prune(&out, Duration::from_secs(3600));
        assert!(handled(&out, 3), "kept within the ack wait");
        prune(&out, Duration::ZERO);
        assert!(!handled(&out, 3));
    }

    #[test]
    fn only_answers_are_hidden_from_the_pane() {
        assert!(hidden(Some("uber.answer")));
        for k in [None, Some("uber.ask"), Some("uber.applied"), Some("other")] {
            assert!(!hidden(k), "{k:?}");
        }
        assert_eq!(consumer("pro.teus"), "uber-pro_teus");
        assert_eq!(printable(Some("uber.answer"), b"{\"secret\":1}"), WITHHELD);
        assert_eq!(printable(Some("uber.ask"), b"{}"), "{}");
    }
}
