//! The UAC host's open asks in vegad (docs/design/uber-uac.md §2-3,
//! vega-lhc.209.4). This host's ask files are the truth
//! (`vega_registry::uber_asks`); each round vegad
//! - withdraws an open ask whose bead closed (answered through the bead, the
//!   board or the pane): `uber.withdraw` to the claim it was sent to;
//! - resends an open ask to a new live claim (New epoch): `uber.ask` again,
//!   then records the new claim, so only its answer counts from then on.
//!
//! Without a live claim nothing is resent; the asks wait for the next Uber.
//! Mail goes out as the UAC pane that asked, so the Uber's answer reaches it.

use std::time::Duration;

use async_nats::{jetstream, HeaderMap};
use vega_registry::uber_asks::{self, OpenAsk, SentTo, State};
use vega_registry::UberClaim;
use vega_uber_msg::{Kind, Withdraw};

use crate::{h, inbox_subject, log, now_ms};

/// One `bd show` per open ask and round; a locked database must not stall vegad.
const BD_TIMEOUT: Duration = Duration::from_secs(5);
/// The withdraw reason when the bead closed.
pub const BEAD_CLOSED: &str = "bead closed: answered elsewhere";

/// What one round does with one open ask.
#[derive(Debug, PartialEq)]
pub enum Action {
    Withdraw { id: String, to: SentTo },
    Resend { id: String, to: SentTo },
}

/// The round's actions: `closed` says whether an ask's bead is closed
/// (`None`: bd could not tell, so the ask stays).
pub fn plan(
    asks: &[OpenAsk],
    live: Option<&UberClaim>,
    mut closed: impl FnMut(&OpenAsk) -> Option<bool>,
) -> Vec<Action> {
    asks.iter()
        .filter(|a| a.state == State::Open)
        .filter_map(|a| {
            let id = a.ask.id.clone();
            if closed(a) == Some(true) {
                return Some(Action::Withdraw {
                    id,
                    to: a.sent_to.clone(),
                });
            }
            let to = SentTo::from(live?);
            (to != a.sent_to).then_some(Action::Resend { id, to })
        })
        .collect()
}

/// Whether bd shows the ask's bead closed; `None` when bd cannot tell.
fn bead_closed(a: &OpenAsk) -> Option<bool> {
    let out = vega_herdr::output_within(
        std::process::Command::new(&a.bd)
            .args(["show", &a.ask.bead, "--json"])
            .current_dir(&a.cwd),
        BD_TIMEOUT,
    );
    match out {
        Ok(Some(o)) if o.status.success() => {
            crate::bead_closed(&String::from_utf8_lossy(&o.stdout))
        }
        _ => None,
    }
}

/// The mail's body and its stable `Nats-Msg-Id`: a retry after a lost ack
/// is deduplicated by the broker.
fn body(a: &OpenAsk, action: &Action) -> (Kind, String, String) {
    match action {
        Action::Withdraw { id, .. } => {
            let w = Withdraw {
                id: id.clone(),
                reason: BEAD_CLOSED.into(),
            };
            let json = serde_json::to_string(&w).expect("a Withdraw serializes");
            (Kind::Withdraw, json, format!("uber.withdraw:{id}"))
        }
        Action::Resend { id, to } => {
            let json = serde_json::to_string(&a.ask).expect("an Ask serializes");
            (Kind::Ask, json, format!("uber.ask:{id}:{}", to.epoch))
        }
    }
}

/// Mails one action as the asking UAC pane; true when the broker took it.
async fn publish(js: &jetstream::Context, host: &str, a: &OpenAsk, action: &Action) -> bool {
    let (Action::Withdraw { to, .. } | Action::Resend { to, .. }) = action;
    let (kind, json, msg_id) = body(a, action);
    let mut headers = HeaderMap::new();
    headers.insert("Nats-Msg-Id", msg_id.as_str());
    headers.insert(h::FROM_PANE, a.uac_pane.as_str());
    headers.insert(h::FROM_AGENT, "vegad");
    headers.insert(h::FROM_CWD, a.cwd.display().to_string().as_str());
    headers.insert(h::SENT_MS, now_ms().to_string().as_str());
    headers.insert(h::KIND, kind.as_str());
    let subject = inbox_subject(&to.host, &to.pane, host);
    let sent = match js.publish_with_headers(subject, headers, json.into()).await {
        Ok(ack) => ack.await.map(drop).map_err(|e| e.to_string()),
        Err(e) => Err(e.to_string()),
    };
    if let Err(e) = &sent {
        log(&format!(
            "uber: {} {} to {} on {} failed, retrying: {e}",
            kind.as_str(),
            a.ask.id,
            to.pane,
            to.host
        ));
    }
    sent.is_ok()
}

/// One round over this host's open asks.
pub async fn round(js: &jetstream::Context, host: &str, view: Option<&vega_registry::View>) {
    let Some(dir) = uber_asks::dir() else { return };
    let now = (now_ms() / 1000) as i64;
    uber_asks::prune(&dir, now);
    let asks = uber_asks::list(&dir);
    if !asks.iter().any(|a| a.state == State::Open) {
        return;
    }
    let live = view.and_then(|v| v.live_uber());
    for action in plan(&asks, live.as_ref(), bead_closed) {
        let (Action::Withdraw { id, to } | Action::Resend { id, to }) = &action;
        let Some(a) = asks.iter().find(|a| a.ask.id == *id) else {
            continue;
        };
        if !publish(js, host, a, &action).await {
            continue;
        }
        let done = match &action {
            Action::Withdraw { .. } => uber_asks::close(&dir, id, State::Withdrawn, now),
            Action::Resend { .. } => uber_asks::resend(&dir, id, to.clone(), now),
        };
        match done {
            Ok(true) => log(&format!(
                "uber: {} {id} to {} on {} (epoch {})",
                match action {
                    Action::Withdraw { .. } => "withdrew",
                    Action::Resend { .. } => "resent",
                },
                to.pane,
                to.host,
                to.epoch
            )),
            Ok(false) => log(&format!("uber: {id} was closed meanwhile")),
            Err(e) => log(&format!("uber: cannot record {id}: {e}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vega_uber_msg::{Ask, Level, Opt};

    fn ask(bead: &str, to: (&str, &str, u64)) -> OpenAsk {
        let id = format!("{bead}~ABCDEFGHIJKLMNOPQRSTUV");
        OpenAsk {
            schema: 1,
            ask: Ask {
                id,
                project: "vega".into(),
                bead: bead.into(),
                level: Level::L3,
                urgent: None,
                question: "Q?".into(),
                context: String::new(),
                options: vec![
                    Opt {
                        label: "a".into(),
                        detail: String::new(),
                    },
                    Opt {
                        label: "b".into(),
                        detail: String::new(),
                    },
                ],
                recommend: 0,
                why: String::new(),
                deadline: "no-rush".into(),
            },
            uac_pane: "w1:p1".into(),
            uac_host: "proteus".into(),
            cwd: "/src/vega".into(),
            bd: "bd".into(),
            sent_to: SentTo {
                host: to.0.into(),
                pane: to.1.into(),
                epoch: to.2,
            },
            sent_at: 1,
            state: State::Open,
            closed_at: None,
        }
    }

    fn claim(host: &str, pane: &str, epoch: u64) -> UberClaim {
        UberClaim {
            host: host.into(),
            pane: pane.into(),
            epoch,
        }
    }

    #[test]
    fn a_new_live_epoch_resends_only_open_asks_of_older_claims() {
        let old = ask("v-1", ("ganymede", "w9:p1", 10));
        let current = ask("v-2", ("titan", "w2:p1", 20));
        let mut answered = ask("v-3", ("ganymede", "w9:p1", 10));
        answered.state = State::Answered;
        let asks = [old.clone(), current, answered];
        let live = claim("titan", "w2:p1", 20);
        let actions = plan(&asks, Some(&live), |_| Some(false));
        assert_eq!(
            actions,
            [Action::Resend {
                id: old.ask.id,
                to: SentTo::from(&live)
            }]
        );
    }

    #[test]
    fn without_a_live_claim_nothing_is_resent() {
        let asks = [ask("v-1", ("ganymede", "w9:p1", 10))];
        assert!(plan(&asks, None, |_| Some(false)).is_empty());
        assert!(plan(&asks, None, |_| None).is_empty());
    }

    #[test]
    fn a_closed_bead_withdraws_from_the_claim_it_was_sent_to() {
        let a = ask("v-1", ("ganymede", "w9:p1", 10));
        let live = claim("titan", "w2:p1", 20);
        let actions = plan(std::slice::from_ref(&a), Some(&live), |_| Some(true));
        assert_eq!(
            actions,
            [Action::Withdraw {
                id: a.ask.id.clone(),
                to: a.sent_to.clone()
            }]
        );
    }

    #[test]
    fn the_bodies_pass_their_schema_and_retries_share_a_msg_id() {
        let a = ask("v-1", ("ganymede", "w9:p1", 10));
        let to = SentTo::from(&claim("titan", "w2:p1", 20));
        for action in [
            Action::Withdraw {
                id: a.ask.id.clone(),
                to: a.sent_to.clone(),
            },
            Action::Resend {
                id: a.ask.id.clone(),
                to,
            },
        ] {
            let (kind, json, msg_id) = body(&a, &action);
            assert!(
                vega_uber_msg::validate(kind, json.as_bytes()).is_ok(),
                "{json}"
            );
            assert_eq!(body(&a, &action).2, msg_id);
        }
    }
}
