//! The Uber host's side of the Uber-UAC (docs/design/uber-uac.md §3-4,
//! vega-lhc.209.6): vegad records each `uber.ask`, `uber.withdraw` and
//! `uber.applied` mailed to this host's Uber claim pane, so `vega uber
//! answer` and `vega uber audit` see them whatever the model does. The
//! records live in `vega_uber_msg::received`; the mail itself still reaches
//! the pane's inbox.

use std::path::Path;

use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;
use vega_registry::uber_state;
use vega_uber_msg::{audit, received, Kind};

/// Records one `uber.*` message to `to_pane` when that pane holds the
/// Uber claim in `claim_file` (of `host`). Returns the vegad log line, or
/// `None` for mail to any other pane.
#[allow(clippy::too_many_arguments)]
pub fn ingest(
    claim_file: &Path,
    host: &str,
    kind: Kind,
    body: &[u8],
    from_host: &str,
    from_pane: &str,
    to_pane: &str,
    now: &str,
) -> Option<String> {
    let claim = uber_state::local(claim_file, host)?;
    if claim["pane"].as_str() != Some(to_pane) || kind == Kind::Answer {
        return None;
    }
    let dir = claim_file.parent()?;
    let ev = match vega_uber_msg::validate(kind, body) {
        Ok(msg) => received::ingest(dir, &msg, from_host, from_pane, now),
        Err(r) => {
            let why = format!("{} from {from_pane} on {from_host}: {}", r.code, r.message);
            let ev = audit::Event::bare("ignored", now, "-", &why);
            audit::append(dir, &ev).map(|()| ev)
        }
    };
    Some(match ev {
        Ok(ev) => format!("uber: {} {} ({})", ev.event, ev.id, kind.as_str()),
        Err(e) => format!("uber: {} from {from_pane} not recorded: {e}", kind.as_str()),
    })
}

/// Now, RFC 3339 in UTC.
pub fn now() -> String {
    OffsetDateTime::now_utc()
        .replace_nanosecond(0)
        .ok()
        .and_then(|t| t.format(&Rfc3339).ok())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const NOW: &str = "2026-10-26T13:00:00Z";

    #[test]
    fn asks_to_the_uber_pane_are_recorded_and_others_are_not() {
        let dir = tempfile::tempdir().unwrap();
        let claim = dir.path().join("uber/claim.json");
        let ask = |id: &str| {
            json!({
                "id": id, "project": "github.com/durandom/vega", "bead": "vega-lhc.209",
                "level": "L3", "urgent": null, "question": "Push main?", "context": "",
                "options": [{"label": "yes", "detail": ""}, {"label": "no", "detail": ""}],
                "recommend": 0, "why": "", "deadline": "no-rush"
            })
            .to_string()
        };
        let id = vega_uber_msg::new_id("vega-lhc.209").unwrap();
        let run = |kind, body: &str, to: &str| {
            ingest(
                &claim,
                "ganymede",
                kind,
                body.as_bytes(),
                "proteus",
                "w3V:p1",
                to,
                NOW,
            )
        };
        // No claim on this host: nothing is recorded.
        assert_eq!(run(Kind::Ask, &ask(&id), "w1:p1"), None);
        uber_state::save(&claim, &uber_state::value("w1:p1", "ganymede", 0, 9)).unwrap();
        // Mail to another pane, and answers (the UAC host's), are not the Uber's.
        assert_eq!(run(Kind::Ask, &ask(&id), "w2:p1"), None);
        assert_eq!(run(Kind::Answer, "{}", "w1:p1"), None);
        let log = run(Kind::Ask, &ask(&id), "w1:p1").unwrap();
        assert_eq!(log, format!("uber: received {id} (uber.ask)"));
        let uber_dir = claim.parent().unwrap();
        let r = received::load(uber_dir, &id).unwrap();
        assert_eq!(
            (r.from_host.as_str(), r.from_pane.as_str()),
            ("proteus", "w3V:p1")
        );
        assert!(run(Kind::Ask, "not json", "w1:p1")
            .unwrap()
            .contains("ignored"));
        let w = json!({"id": id, "reason": "obsolete"}).to_string();
        assert!(run(Kind::Withdraw, &w, "w1:p1")
            .unwrap()
            .contains("withdrawn"));
        let events: Vec<String> = audit::last(uber_dir, 10)
            .into_iter()
            .map(|e| e.event)
            .collect();
        assert_eq!(events, ["received", "ignored", "withdrawn"]);
    }
}
