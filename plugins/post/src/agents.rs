//! Agent status for the relay: which panes are free (`idle`/`done`), which
//! are busy, which closed. One thread, two modes:
//!
//! - stream: a minimal herdr socket client. `session.snapshot` for the full
//!   picture, then `events.subscribe` for status changes, focus and closed
//!   panes.
//!   herdr ends a subscription connection on any further request, so a new
//!   pane (or lost events, or a closed stream) means: snapshot again and
//!   subscribe again on a fresh connection.
//! - poll: `herdr agent list` every few seconds. Used when the schema is not
//!   one we know (no socket calls at all), and while the stream is down
//!   (the stream is retried every STREAM_RETRY).
//!
//! The socket protocol is the one place where herdr drift can break us, so
//! it stays small: two methods, a handful of fields, loose parsing.

use std::collections::HashMap;
use std::env;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tokio::sync::mpsc::UnboundedSender;

/// herdr `schema_version`s this client was written against (herdr 0.9.3).
const KNOWN_SCHEMAS: [u64; 1] = [1];
/// Subscription types we send; an unknown schema without them means poll.
const NEEDED: [&str; 4] = [
    "pane.agent_status_changed",
    "pane.closed",
    "pane.exited",
    "pane.created",
];
pub const POLL_EVERY: Duration = Duration::from_secs(3);
pub const STREAM_RETRY: Duration = Duration::from_secs(30);

/// What the watcher tells the relay.
#[derive(Debug, PartialEq)]
pub enum Event {
    /// Full picture: every live pane and its agent status. Panes not in it
    /// are gone.
    Snapshot(HashMap<String, String>),
    /// One pane's agent status changed.
    Status(String, String),
    /// The pane closed or its process exited.
    Gone(String),
    /// The pane got the focus (any client, any cause).
    Focused(String),
    /// The watcher switched mode.
    Mode(Mode),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Mode {
    Stream,
    /// Polling, and why.
    Poll(String),
}

impl Mode {
    pub fn describe(&self) -> String {
        match self {
            Mode::Stream => "stream (herdr events.subscribe)".into(),
            Mode::Poll(why) => format!(
                "poll (herdr agent list every {}s; {why})",
                POLL_EVERY.as_secs()
            ),
        }
    }
}

pub fn spawn(tx: UnboundedSender<Event>) {
    thread::spawn(move || run(&tx));
}

fn run(tx: &UnboundedSender<Event>) {
    if let Err(why) = check_schema() {
        let _ = tx.send(Event::Mode(Mode::Poll(why)));
        loop {
            poll_once(tx);
            thread::sleep(POLL_EVERY);
        }
    }
    let mut quick_resyncs = 0u32;
    loop {
        match stream(tx) {
            // Resync at once (new pane, lost events), but do not spin if
            // herdr keeps refusing the subscription (pane closed meanwhile).
            Ok(()) => {
                quick_resyncs += 1;
                if quick_resyncs > 5 {
                    thread::sleep(Duration::from_millis(500));
                }
            }
            Err(why) => {
                quick_resyncs = 0;
                let _ = tx.send(Event::Mode(Mode::Poll(format!(
                    "stream down: {why}; retry every {}s",
                    STREAM_RETRY.as_secs()
                ))));
                let until = Instant::now() + STREAM_RETRY;
                while Instant::now() < until {
                    poll_once(tx);
                    thread::sleep(POLL_EVERY);
                }
            }
        }
    }
}

/// `herdr api schema --json`: Ok if we know its `schema_version` and it has
/// the subscriptions we use.
fn check_schema() -> Result<(), String> {
    let schema = vega_herdr::herdr(&["api", "schema", "--json"])
        .map_err(|f| format!("herdr api schema failed: {}", f.message))?;
    schema_known(&schema)
}

fn schema_known(schema: &Value) -> Result<(), String> {
    let version = schema["schema_version"].as_u64();
    if !version.is_some_and(|v| KNOWN_SCHEMAS.contains(&v)) {
        return Err(format!(
            "unknown herdr schema_version {}",
            version.map_or("none".into(), |v| v.to_string())
        ));
    }
    let text = schema.to_string();
    if let Some(missing) = NEEDED.iter().find(|n| !text.contains(&format!("\"{n}\""))) {
        return Err(format!("herdr schema lacks {missing}"));
    }
    Ok(())
}

fn socket_path() -> String {
    if let Ok(p) = env::var("HERDR_SOCKET_PATH") {
        if !p.is_empty() {
            return p;
        }
    }
    format!(
        "{}/herdr/herdr.sock",
        crate::config_dir().unwrap_or_else(|| ".".into())
    )
}

fn connect() -> Result<UnixStream, String> {
    let path = socket_path();
    UnixStream::connect(&path).map_err(|e| format!("connect {path}: {e}"))
}

fn request(conn: &mut UnixStream, method: &str, params: Value) -> Result<(), String> {
    let line = json!({"id": "vegad", "method": method, "params": params}).to_string();
    conn.write_all(format!("{line}\n").as_bytes())
        .map_err(|e| format!("write: {e}"))
}

/// One stream session. Ok(()) asks for a resync, Err for a fallback to poll.
fn stream(tx: &UnboundedSender<Event>) -> Result<(), String> {
    let panes = snapshot()?;
    let ids: Vec<String> = panes.keys().cloned().collect();
    let _ = tx.send(Event::Snapshot(panes));

    let mut conn = connect()?;
    request(&mut conn, "events.subscribe", subscriptions(&ids))?;
    let mut lines = BufReader::new(conn).lines();
    let first = next_json(&mut lines)?;
    if let Some(err) = first.get("error") {
        // A pane closed between snapshot and subscribe: try again.
        if err["code"] == "pane_not_found" {
            return Ok(());
        }
        return Err(format!("subscribe refused: {}", err["message"]));
    }
    let _ = tx.send(Event::Mode(Mode::Stream));
    loop {
        let v = next_json(&mut lines)?;
        match parse_event(&v) {
            Parsed::Event(e) => {
                let _ = tx.send(e);
            }
            Parsed::Resync => return Ok(()),
            Parsed::Ignore => {}
        }
    }
}

fn next_json(lines: &mut std::io::Lines<BufReader<UnixStream>>) -> Result<Value, String> {
    loop {
        match lines.next() {
            None => return Err("herdr closed the event stream".into()),
            Some(Err(e)) => return Err(format!("read: {e}")),
            Some(Ok(l)) if l.trim().is_empty() => continue,
            Some(Ok(l)) => {
                return serde_json::from_str(&l)
                    .map_err(|_| "herdr sent a line that is not JSON".into())
            }
        }
    }
}

fn subscriptions(panes: &[String]) -> Value {
    let mut subs: Vec<Value> = [
        "pane.closed",
        "pane.exited",
        "pane.created",
        "pane.agent_detected",
        "pane.focused",
    ]
    .iter()
    .map(|t| json!({"type": t}))
    .collect();
    subs.extend(
        panes
            .iter()
            .map(|p| json!({"type": "pane.agent_status_changed", "pane_id": p})),
    );
    json!({ "subscriptions": subs })
}

/// `session.snapshot` over the socket: every pane and its agent status.
fn snapshot() -> Result<HashMap<String, String>, String> {
    let mut conn = connect()?;
    request(&mut conn, "session.snapshot", json!({}))?;
    let mut lines = BufReader::new(conn).lines();
    let v = next_json(&mut lines)?;
    if let Some(err) = v.get("error") {
        return Err(format!("session.snapshot refused: {}", err["message"]));
    }
    panes_of(&v["result"]["snapshot"]["panes"])
        .ok_or_else(|| "session.snapshot has no panes list".into())
}

/// `[{pane_id, agent_status}, ...]` as a map; a missing status is `unknown`.
fn panes_of(list: &Value) -> Option<HashMap<String, String>> {
    Some(
        list.as_array()?
            .iter()
            .filter_map(|p| {
                let id = p["pane_id"].as_str()?;
                let status = p["agent_status"].as_str().unwrap_or("unknown");
                Some((id.to_string(), status.to_string()))
            })
            .collect(),
    )
}

#[derive(Debug, PartialEq)]
enum Parsed {
    Event(Event),
    Resync,
    Ignore,
}

/// One stream line. herdr names events `pane.closed` in subscriptions and
/// `pane_closed` in envelopes; accept both.
fn parse_event(v: &Value) -> Parsed {
    let name = v["event"]
        .as_str()
        .or_else(|| v["type"].as_str())
        .unwrap_or("")
        .replace('.', "_");
    let data = &v["data"];
    let pane = data["pane_id"]
        .as_str()
        .or_else(|| data["pane"]["pane_id"].as_str())
        .map(String::from);
    match (name.as_str(), pane) {
        ("pane_agent_status_changed", Some(p)) => match data["agent_status"].as_str() {
            Some(s) => Parsed::Event(Event::Status(p, s.to_string())),
            None => Parsed::Ignore,
        },
        ("pane_closed" | "pane_exited", Some(p)) => Parsed::Event(Event::Gone(p)),
        ("pane_focused", Some(p)) => Parsed::Event(Event::Focused(p)),
        // A new pane needs its own status subscription.
        ("pane_created" | "pane_agent_detected", _) => Parsed::Resync,
        (n, _) if n.contains("lost") || n.contains("lagged") => Parsed::Resync,
        _ => Parsed::Ignore,
    }
}

/// `herdr agent list` once; failures leave the last picture in place.
/// Set while polls fail: vegad outlives herdr, so an outage logs one line,
/// not one per poll.
static POLL_FAILING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn poll_once(tx: &UnboundedSender<Event>) {
    use std::sync::atomic::Ordering;
    match vega_herdr::herdr(&["agent", "list"]) {
        Ok(v) => {
            if POLL_FAILING.swap(false, Ordering::Relaxed) {
                crate::log("poll: herdr answers again");
            }
            if let Some(panes) = panes_of(&v["result"]["agents"]) {
                let _ = tx.send(Event::Snapshot(panes));
            }
        }
        Err(f) => {
            if !POLL_FAILING.swap(true, Ordering::Relaxed) {
                crate::log(&format!(
                    "poll failed: {} {} (quiet until herdr answers)",
                    f.code, f.message
                ));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_must_be_known_and_complete() {
        let ok = json!({"schema_version": 1, "x": NEEDED});
        assert_eq!(schema_known(&ok), Ok(()));
        let new = json!({"schema_version": 2, "x": NEEDED});
        assert!(schema_known(&new).unwrap_err().contains("schema_version 2"));
        let lacking = json!({"schema_version": 1, "x": ["pane.closed"]});
        assert!(schema_known(&lacking).unwrap_err().contains("lacks"));
        assert!(schema_known(&json!({})).unwrap_err().contains("none"));
    }

    #[test]
    fn events_parse_in_both_spellings() {
        let status = json!({"event": "pane.agent_status_changed",
            "data": {"pane_id": "w1:p2", "workspace_id": "w1", "agent_status": "idle"}});
        assert_eq!(
            parse_event(&status),
            Parsed::Event(Event::Status("w1:p2".into(), "idle".into()))
        );
        let closed =
            json!({"event": "pane_closed", "data": {"pane_id": "w1:p2", "workspace_id": "w1"}});
        assert_eq!(
            parse_event(&closed),
            Parsed::Event(Event::Gone("w1:p2".into()))
        );
        let exited = json!({"event": "pane.exited", "data": {"pane_id": "w1:p3"}});
        assert_eq!(
            parse_event(&exited),
            Parsed::Event(Event::Gone("w1:p3".into()))
        );
        let created = json!({"event": "pane_created", "data": {"pane": {"pane_id": "w1:p4"}}});
        assert_eq!(parse_event(&created), Parsed::Resync);
        assert_eq!(
            parse_event(&json!({"event": "events_lost"})),
            Parsed::Resync
        );
        let focused =
            json!({"event": "pane.focused", "data": {"pane_id": "w1:p5", "workspace_id": "w1"}});
        assert_eq!(
            parse_event(&focused),
            Parsed::Event(Event::Focused("w1:p5".into()))
        );
        assert_eq!(
            parse_event(&json!({"event": "pane_focused"})),
            Parsed::Ignore
        );
        assert_eq!(
            parse_event(&json!({"event": "tab_focused", "data": {"pane_id": "w1:p5"}})),
            Parsed::Ignore
        );
    }

    #[test]
    fn panes_default_to_unknown() {
        let m = panes_of(&json!([{"pane_id": "a:b", "agent_status": "done"}, {"pane_id": "a:c"}]))
            .unwrap();
        assert_eq!(m["a:b"], "done");
        assert_eq!(m["a:c"], "unknown");
        assert!(panes_of(&json!(null)).is_none());
    }

    #[test]
    fn status_subscriptions_are_per_pane() {
        let s = subscriptions(&["w1:p1".into()]);
        let subs = s["subscriptions"].as_array().unwrap();
        assert!(subs.contains(&json!({"type": "pane.agent_status_changed", "pane_id": "w1:p1"})));
        assert!(subs.contains(&json!({"type": "pane.closed"})));
    }
}
