//! `vega uber ask`: the UAC hands a `slayer-decision` bead to the Uber-UAC
//! (docs/design/uber-uac.md §3 Routing, vega-lhc.209.4). It checks the
//! options and caps, writes the open ask to this host's state
//! (`vega_registry::uber_asks`) and mails it as `uber.ask` to the live claim.
//! No live claim (or no broker): `uber_absent`, nothing is stored or sent,
//! and the bead flow goes on as without an Uber.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use clap::Args;
use serde_json::Value;
use vega_herdr::{state as st, Failure};
use vega_registry::uber_asks::{self, OpenAsk, SentTo, State};
use vega_uber_msg::{Ask, Kind, Level, Opt};

use crate::{here, now_ms, read_view, with_registry};

/// bd answers in well under a second; a locked database must not hang the UAC.
const BD_TIMEOUT: Duration = Duration::from_secs(10);
/// `vega post send` connects patiently; give it a bit more.
const SEND_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Args, Debug)]
pub struct AskArgs {
    /// The open `slayer-decision` bead the question is recorded in
    #[arg(long)]
    pub bead: String,
    /// L3: the Slayer picks an option; L4: only the Slayer may approve
    #[arg(long, value_parser = parse_level)]
    pub level: Level,
    /// Why it cannot wait (production down, security, data loss, deadline under 1 h)
    #[arg(long)]
    pub urgent: Option<String>,
    /// RFC 3339 time, or no-rush; a passed deadline only marks the ask overdue
    #[arg(long)]
    pub deadline: String,
    /// The question, at most 120 characters [default: the bead's title]
    #[arg(long)]
    pub question: Option<String>,
    /// What the Slayer needs to know, at most 280 characters
    #[arg(long, default_value = "")]
    pub context: String,
    /// "<label>: <detail>", 2 or 3 times; label at most 40, detail at most 120 characters
    #[arg(long = "option", required = true)]
    pub options: Vec<String>,
    /// The recommended option, 0-based
    #[arg(long)]
    pub recommend: usize,
    /// Why that option, at most 120 characters
    #[arg(long, default_value = "")]
    pub why: String,
}

fn parse_level(s: &str) -> Result<Level, String> {
    match s {
        "L3" | "l3" => Ok(Level::L3),
        "L4" | "l4" => Ok(Level::L4),
        _ => Err("L3 or L4 (L0-L2: the UAC decides itself)".into()),
    }
}

/// `"<label>: <detail>"`; without `:` all of it is the label.
fn parse_option(s: &str) -> Opt {
    let (label, detail) = s.split_once(':').unwrap_or((s, ""));
    Opt {
        label: label.trim().to_string(),
        detail: detail.trim().to_string(),
    }
}

/// What `ask` needs of the bead.
#[derive(Debug, PartialEq)]
pub(crate) struct Bead {
    pub title: String,
    pub open: bool,
    pub decision: bool,
}

/// `bd show --json` output: the first issue's title, status and labels.
pub(crate) fn parse_bead(json: &str) -> Option<Bead> {
    let v: Value = serde_json::from_str(json).ok()?;
    let b = v.as_array()?.first()?;
    Some(Bead {
        title: b["title"].as_str()?.to_string(),
        open: b["status"] != "closed",
        decision: b["labels"]
            .as_array()
            .is_some_and(|l| l.iter().any(|x| x == "slayer-decision")),
    })
}

fn show_bead(cwd: &Path, id: &str) -> Result<Bead, Failure> {
    let out = vega_herdr::output_within(
        Command::new("bd")
            .args(["show", id, "--json"])
            .current_dir(cwd),
        BD_TIMEOUT,
    );
    let unknown = || {
        Failure::new(
            "no_bead",
            format!("bd does not show {id} here"),
            "record the question first: bd create ... --label slayer-decision",
        )
    };
    match out {
        Ok(Some(o)) if o.status.success() => parse_bead(&vega_plugin::unwrap_bd_envelope(
            &String::from_utf8_lossy(&o.stdout),
        ))
        .ok_or_else(unknown),
        Ok(Some(_)) => Err(unknown()),
        Ok(None) => Err(Failure::new("bd_timeout", "bd did not answer", "retry")),
        Err(e) => Err(Failure::new("no_bd", e.to_string(), "install beads (bd)")),
    }
}

/// The `uber.ask` body from the arguments and the bead; checked against the
/// schema and the cap (vega_uber_msg), so nothing invalid is stored.
pub(crate) fn build(a: &AskArgs, bead: &Bead, project: &str, id: String) -> Result<Ask, Failure> {
    if !bead.open {
        return Err(Failure::new(
            "bead_closed",
            format!("{} is closed: nothing to ask", a.bead),
            "ask only for an open slayer-decision bead",
        ));
    }
    if !bead.decision {
        return Err(Failure::new(
            "not_decision",
            format!("{} has no slayer-decision label", a.bead),
            &format!("bd update {} --add-label slayer-decision", a.bead),
        ));
    }
    let ask = Ask {
        id,
        project: project.to_string(),
        bead: a.bead.clone(),
        level: a.level,
        urgent: a.urgent.clone(),
        question: a.question.clone().unwrap_or_else(|| bead.title.clone()),
        context: a.context.clone(),
        options: a.options.iter().map(|o| parse_option(o)).collect(),
        recommend: a.recommend,
        why: a.why.clone(),
        deadline: a.deadline.clone(),
    };
    let body = serde_json::to_string(&ask).expect("an Ask serializes");
    vega_uber_msg::validate(Kind::Ask, body.as_bytes()).map_err(|r| {
        Failure::new(
            r.code,
            format!("{}; nothing was asked", r.message),
            "shorten or fix the flags; caps: docs/design/uber-uac.md §3",
        )
    })?;
    Ok(ask)
}

/// An open ask for `bead` already exists: one ask per decision.
fn already(dir: &Path, bead: &str) -> Option<OpenAsk> {
    uber_asks::list(dir)
        .into_iter()
        .find(|a| a.state == State::Open && a.ask.bead == bead)
}

fn absent(why: &str) -> Failure {
    Failure::new(
        "uber_absent",
        format!("{why}; nothing was asked or stored"),
        "decide through the bead as without an Uber: the board and your pane carry it",
    )
}

/// The installed `vega-post` next to this binary, else PATH.
fn post_bin() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|e| Some(e.parent()?.join("vega-post")))
        .filter(|p| p.is_file())
        .or_else(|| vega_plugin::find_on_path("vega-post"))
        .unwrap_or_else(|| "vega-post".into())
}

/// Mails `body` as `uber.ask` to exactly the recorded claim.
pub(crate) fn send(to: &SentTo, body: &str) -> Result<(), Failure> {
    let out = vega_herdr::output_within(
        Command::new(post_bin()).args([
            "send",
            "--host",
            &to.host,
            "--kind",
            Kind::Ask.as_str(),
            &to.pane,
            body,
        ]),
        SEND_TIMEOUT,
    );
    match out {
        Ok(Some(o)) if o.status.success() => Ok(()),
        Ok(Some(o)) => {
            let text = String::from_utf8_lossy(&o.stdout).to_string()
                + &String::from_utf8_lossy(&o.stderr);
            Err(Failure::new(
                "send_failed",
                format!(
                    "vega post send failed: {}",
                    text.split_whitespace().collect::<Vec<_>>().join(" ")
                ),
                "nothing was stored; retry, or decide through the bead",
            ))
        }
        Ok(None) => Err(Failure::new(
            "send_failed",
            "vega post send timed out",
            "nothing was stored; retry, or decide through the bead",
        )),
        Err(e) => Err(Failure::new(
            "send_failed",
            e.to_string(),
            "install vega (mise run install)",
        )),
    }
}

/// Who asks, from where: the UAC's pane, host and checkout.
pub(crate) struct Uac {
    pub pane: String,
    pub host: String,
    pub cwd: PathBuf,
}

/// Resolves the live claim in `view` (an error: the registry is away),
/// stores the open ask and sends it with `send`; a failed send removes it.
pub(crate) fn ask_to(
    dir: &Path,
    body: Ask,
    uac: Uac,
    view: Result<vega_registry::View, String>,
    now: i64,
    send: impl FnOnce(&SentTo, &str) -> Result<(), Failure>,
) -> Result<OpenAsk, Failure> {
    let uber = view
        .map_err(|e| absent(&format!("registry: {e}")))?
        .live_uber()
        .ok_or_else(|| absent("no live Uber-UAC claim"))?;
    let record = OpenAsk {
        schema: 1,
        ask: body,
        uac_pane: uac.pane,
        uac_host: uac.host,
        cwd: uac.cwd,
        sent_to: SentTo::from(&uber),
        sent_at: now,
        state: State::Open,
        closed_at: None,
    };
    let io = |e: std::io::Error| Failure::new("state_io", e.to_string(), "check the state dir");
    uber_asks::save(dir, &record).map_err(io)?;
    let json = serde_json::to_string(&record.ask).expect("an Ask serializes");
    if let Err(f) = send(&record.sent_to, &json) {
        let _ = uber_asks::remove(dir, &record.ask.id);
        return Err(f);
    }
    Ok(record)
}

pub(crate) fn ask(a: AskArgs) -> Result<(), Failure> {
    let cwd = std::env::current_dir()
        .map_err(|e| Failure::new("no_cwd", e.to_string(), "run it in the project checkout"))?;
    let host = st::post_host();
    let project = vega_herdr::project::resolve_optional(&cwd, &host)?.ok_or_else(|| {
        Failure::new(
            "no_project",
            "not in a project checkout",
            "run vega uber ask in the project's checkout",
        )
    })?;
    let dir = uber_asks::dir()
        .ok_or_else(|| Failure::new("no_home", "no state dir", "set HOME or XDG_STATE_HOME"))?;
    if let Some(o) = already(&dir, &a.bead) {
        return Err(Failure::new(
            "already_asked",
            format!(
                "{} is already asked ({}, sent to {} on {})",
                a.bead, o.ask.id, o.sent_to.pane, o.sent_to.host
            ),
            "one ask per decision; it is withdrawn when the bead closes",
        ));
    }
    let bead = show_bead(&cwd, &a.bead)?;
    let id = vega_uber_msg::new_id(&a.bead)
        .map_err(|r| Failure::new(r.code, r.message, "pass a bead id"))?;
    let body = build(&a, &bead, project.id.as_str(), id)?;
    let here = here()?;
    let view = with_registry(async |client, kv| {
        read_view(client, kv, &here)
            .await
            .map_err(|e| Failure::new("no_broker", e, ""))
    })
    .map_err(|f| f.message);
    let uac = Uac {
        pane: here.pane.clone(),
        host,
        cwd: project.location.checkout.clone(),
    };
    let r = ask_to(&dir, body, uac, view, (now_ms() / 1000) as i64, send)?;
    let to = &r.sent_to;
    println!("asked: {}", r.ask.id);
    println!("uber: {} on {}, epoch {}", to.pane, to.host, to.epoch);
    println!(
        "bead: {} stays the record; when it closes, vegad withdraws the ask",
        a.bead
    );
    println!("help: park the waiting sentinel (vega uac park); the answer arrives as a Slayer comment on the bead and a mail");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use vega_registry::{uber_state, View};

    fn args(options: &[&str]) -> AskArgs {
        AskArgs {
            bead: "vega-12".into(),
            level: Level::L4,
            urgent: None,
            deadline: "no-rush".into(),
            question: None,
            context: "main is red".into(),
            options: options.iter().map(|o| o.to_string()).collect(),
            recommend: 0,
            why: String::new(),
        }
    }

    fn decision() -> Bead {
        Bead {
            title: "Push the fix to main?".into(),
            open: true,
            decision: true,
        }
    }

    fn body() -> Ask {
        let id = vega_uber_msg::new_id("vega-12").unwrap();
        build(&args(&["yes: push", "no"]), &decision(), "vega", id).unwrap()
    }

    fn uac(dir: &Path) -> Uac {
        Uac {
            pane: "w1:p1".into(),
            host: "proteus".into(),
            cwd: dir.into(),
        }
    }

    fn live(host: &str, pane: &str, epoch: u64) -> View {
        let mut view = View::default();
        let now = crate::now_ms() / 1000;
        view.entries
            .insert(vega_registry::beat_key(host), json!({"at": now}));
        view.entries.insert(
            vega_registry::uber_key(host),
            uber_state::value(pane, host, 0, epoch),
        );
        view.entries.insert(
            vega_registry::agent_key(host, pane),
            json!({"pane": pane, "status": "idle"}),
        );
        view
    }

    #[test]
    fn options_split_at_the_first_colon() {
        let o = parse_option("yes: push: now ");
        assert_eq!((o.label.as_str(), o.detail.as_str()), ("yes", "push: now"));
        let o = parse_option("later");
        assert_eq!((o.label.as_str(), o.detail.as_str()), ("later", ""));
    }

    #[test]
    fn bead_json_gives_title_state_and_the_decision_label() {
        let json = r#"[{"id":"vega-12","title":"T","status":"open","labels":["slayer-decision"]}]"#;
        let b = parse_bead(json).unwrap();
        assert_eq!((b.title.as_str(), b.open, b.decision), ("T", true, true));
        let closed = parse_bead(r#"[{"title":"T","status":"closed"}]"#).unwrap();
        assert!(!closed.open && !closed.decision);
        assert_eq!(parse_bead("[]"), None);
    }

    #[test]
    fn the_body_takes_the_bead_title_and_passes_the_schema() {
        let b = body();
        assert_eq!(b.question, "Push the fix to main?");
        assert_eq!(b.options[0].detail, "push");
        assert_eq!(b.level, Level::L4);
    }

    #[test]
    fn invalid_asks_are_refused_before_anything_is_stored() {
        let id = || vega_uber_msg::new_id("vega-12").unwrap();
        let one = build(&args(&["yes"]), &decision(), "vega", id()).unwrap_err();
        assert_eq!(one.code, "uber_malformed");
        let mut a = args(&["a", "b"]);
        a.recommend = 2;
        assert_eq!(
            build(&a, &decision(), "vega", id()).unwrap_err().code,
            "uber_malformed"
        );
        let closed = Bead {
            open: false,
            ..decision()
        };
        assert_eq!(
            build(&a, &closed, "vega", id()).unwrap_err().code,
            "bead_closed"
        );
        let plain = Bead {
            decision: false,
            ..decision()
        };
        assert_eq!(
            build(&a, &plain, "vega", id()).unwrap_err().code,
            "not_decision"
        );
    }

    #[test]
    fn without_a_live_uber_the_ask_is_uber_absent_and_nothing_is_stored_or_sent() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        let never = |_: &SentTo, _: &str| -> Result<(), Failure> { panic!("sent") };
        let f = ask_to(d, body(), uac(d), Ok(View::default()), 1, never).unwrap_err();
        assert_eq!(f.code, "uber_absent");
        let f = ask_to(d, body(), uac(d), Err("broker down".into()), 1, never).unwrap_err();
        assert_eq!(f.code, "uber_absent");
        assert!(uber_asks::list(d).is_empty());
    }

    #[test]
    fn the_ask_is_stored_for_the_live_claim_and_dropped_when_the_send_fails() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        let mut sent = None;
        let r = ask_to(
            d,
            body(),
            uac(d),
            Ok(live("ganymede", "w9:p1", 77)),
            5,
            |to, json| {
                sent = Some((to.clone(), json.to_string()));
                Ok(())
            },
        )
        .unwrap();
        let (to, json) = sent.unwrap();
        assert_eq!(
            (to.host.as_str(), to.pane.as_str(), to.epoch),
            ("ganymede", "w9:p1", 77)
        );
        assert!(vega_uber_msg::validate(Kind::Ask, json.as_bytes()).is_ok());
        assert_eq!(uber_asks::load(d, &r.ask.id), Some(r.clone()));
        assert_eq!(already(d, "vega-12").map(|a| a.ask.id), Some(r.ask.id));

        let other = tempfile::tempdir().unwrap();
        let fail = |_: &SentTo, _: &str| Err(Failure::new("send_failed", "x", ""));
        let o = other.path();
        let f = ask_to(
            o,
            body(),
            uac(o),
            Ok(live("ganymede", "w9:p1", 77)),
            5,
            fail,
        );
        assert_eq!(f.unwrap_err().code, "send_failed");
        assert!(uber_asks::list(o).is_empty());
    }
}
