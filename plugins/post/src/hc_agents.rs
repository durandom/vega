//! hc events.subscribe through its JSON-lines RPC transport, with bounded silence and
//! polling fallback. Only local terminals enter this host's relay/registry.
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use crate::agents::{Event, POLL_EVERY, STREAM_RETRY};
use crate::terminals;
use serde_json::Value;
use tokio::sync::mpsc::UnboundedSender;

type Panes = HashMap<String, String>;

pub fn spawn(tx: UnboundedSender<Event>) {
    thread::spawn(move || {
        while !tx.is_closed() {
            match stream(&tx) {
                Ok(()) => thread::sleep(Duration::from_millis(100)),
                Err(why) => {
                    let _ = tx.send(Event::HcMode(format!("poll (terminal.list; {why})")));
                    let until = Instant::now() + STREAM_RETRY;
                    while !tx.is_closed() && Instant::now() < until {
                        if let Ok(agents) = terminals::hc_list() {
                            terminals::native_parents(&agents);
                            let _ = tx.send(Event::HcSnapshot(statuses(&agents)));
                        }
                        thread::sleep(POLL_EVERY);
                    }
                }
            }
        }
    });
}

fn statuses(agents: &[Value]) -> Panes {
    agents
        .iter()
        .filter_map(|a| {
            Some((
                a["pane_id"].as_str()?.into(),
                a["agent_status"].as_str()?.into(),
            ))
        })
        .collect()
}

#[derive(Debug, PartialEq)]
enum Parsed {
    Snapshot(Panes, u64),
    Event(Event, u64),
    Resync,
    Ignore,
}

fn parse(v: &Value, seq: u64) -> Result<Parsed, String> {
    if v.get("terminals").is_some() {
        return terminals::hc_agents(v)
            .map(|a| Parsed::Snapshot(statuses(&a), v["seq"].as_u64().unwrap_or(0)))
            .map_err(|e| e.message);
    }
    match v["method"].as_str() {
        Some("missed") => return Ok(Parsed::Resync),
        Some("heartbeat") => return Ok(Parsed::Ignore),
        Some("event") => {}
        _ => return Err("unknown hc stream envelope".into()),
    }
    let e = &v["params"];
    let next = e["seq"].as_u64().ok_or("event without sequence")?;
    if next <= seq {
        return Ok(Parsed::Ignore);
    }
    let Some(pane) = e["terminal"]
        .as_str()
        .filter(|p| vega_herdr::pane::is_hc_id(p))
    else {
        return Ok(Parsed::Ignore);
    };
    let status = |v: &Value| {
        v["status"]["state"]
            .as_str()
            .unwrap_or("unknown")
            .to_string()
    };
    Ok(match e["type"].as_str() {
        Some("created" | "restarted") => {
            Parsed::Event(Event::Status(pane.into(), status(&e["data"])), next)
        }
        Some("status") => Parsed::Event(Event::Status(pane.into(), status(&e["data"])), next),
        Some("finished" | "removed" | "lost") => Parsed::Event(Event::Gone(pane.into()), next),
        _ => Parsed::Ignore,
    })
}

fn stream(tx: &UnboundedSender<Event>) -> Result<(), String> {
    let mut child = Command::new("hc")
        .arg("connect")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| e.to_string())?;
    // Keep stdin open for this subscription; closing it ends `hc connect`.
    let mut stdin = child.stdin.take().expect("piped");
    if let Err(e) = writeln!(
        stdin,
        "{}",
        serde_json::json!({"jsonrpc":"2.0","id":1,"method":"events.subscribe","params":{}})
    ) {
        let _ = child.kill();
        let _ = child.wait();
        return Err(e.to_string());
    }
    let stdout = child.stdout.take().expect("piped");
    let (lines_tx, lines) = mpsc::channel();
    let reader = thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let failed = line.is_err();
            if lines_tx.send(line).is_err() || failed {
                break;
            }
        }
    });
    let result = (|| {
        let mut seq = 0;
        let mut synced = false;
        loop {
            if tx.is_closed() {
                return Err("relay stopped".into());
            }
            let line = lines
                .recv_timeout(STREAM_RETRY)
                .map_err(|e| e.to_string())?
                .map_err(|e| e.to_string())?;
            let mut v: Value = serde_json::from_str(&line).map_err(|e| e.to_string())?;
            if !synced {
                if v["jsonrpc"] != "2.0" || v["id"] != 1 {
                    return Err("invalid events.subscribe reply".into());
                }
                let result = v.get("result").ok_or("events.subscribe failed")?;
                vega_herdr::hc::validate_result("events.subscribe", result)
                    .map_err(|e| e.message)?;
                v = result.clone();
            }
            match parse(&v, seq)? {
                Parsed::Snapshot(panes, at) => {
                    if let Ok(agents) = terminals::hc_agents(&v) {
                        terminals::native_parents(&agents);
                    }
                    seq = at;
                    synced = true;
                    let _ = tx.send(Event::HcSnapshot(panes));
                    let _ = tx.send(Event::HcMode("stream (hc events.subscribe)".into()));
                }
                Parsed::Event(e, at) if synced => {
                    if v["params"]["type"] == "created" || v["params"]["type"] == "restarted" {
                        let snapshot =
                            serde_json::json!({"terminals":[v["params"]["data"].clone()]});
                        if let Ok(agents) = terminals::hc_agents(&snapshot) {
                            terminals::native_parents(&agents);
                        }
                    }
                    seq = at;
                    let _ = tx.send(e);
                }
                Parsed::Resync => return Ok(()),
                Parsed::Ignore if synced => {}
                _ => return Err("hc stream has no initial snapshot".into()),
            }
        }
    })();
    let _ = child.kill();
    let _ = child.wait();
    drop(lines);
    let _ = reader.join();
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn snapshot_filters_remote_finished_and_normalizes_kinds() {
        let v = json!({"seq":9,"terminals":[
            {"address":"t1","state":"live","status":{"app":"claude-code","state":"idle"}},
            {"address":"io/t1","state":"live","status":{"state":"working"}},
            {"address":"t2","state":"finished"}
        ]});
        assert_eq!(
            parse(&v, 0).unwrap(),
            Parsed::Snapshot(HashMap::from([("t1".into(), "idle".into())]), 9)
        );
        let a = terminals::hc_agents(&v).unwrap();
        assert_eq!(a[0]["agent"], "claude");
        assert_eq!(a[0]["address"], "hc:t1");
    }

    #[test]
    fn missed_resyncs_and_old_events_cannot_overwrite_snapshot() {
        let e = json!({"method":"event","params":{"seq":5,"type":"status","terminal":"t1","data":{"status":{"state":"blocked"}}}});
        assert_eq!(parse(&e, 5).unwrap(), Parsed::Ignore);
        assert_eq!(
            parse(&e, 4).unwrap(),
            Parsed::Event(Event::Status("t1".into(), "blocked".into()), 5)
        );
        assert_eq!(
            parse(&json!({"method":"missed","params":{}}), 5).unwrap(),
            Parsed::Resync
        );
        assert!(parse(&json!({"error":{}}), 0).is_err());
    }
}
