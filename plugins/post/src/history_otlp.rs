//! Lossy, bounded metadata-only OTLP logs. Never handles history bodies or retries
//! central publication. Collector stalls cannot hold the history outbox worker.
use serde_json::{json, Value};
use std::sync::mpsc;
use vega_herdr::otlp::Config;

pub struct Sink {
    tx: mpsc::SyncSender<Value>,
    saturated: std::sync::atomic::AtomicBool,
}
impl Sink {
    pub fn configured() -> Option<Self> {
        match vega_herdr::otlp::load() {
            Ok(Some(config)) => Some(Self::start(config)),
            Ok(None) => None,
            Err(_) => {
                crate::log(
                    "history OTLP: invalid configuration; export disabled (see vega/otlp.toml)",
                );
                None
            }
        }
    }
    fn start(config: Config) -> Self {
        let (tx, rx) = mpsc::sync_channel::<Value>(64);
        std::thread::spawn(move || {
            let agent = ureq::AgentBuilder::new()
                .timeout(config.timeout)
                .redirects(0)
                .build();
            let url = format!("{}/v1/logs", config.endpoint);
            let mut failed = false;
            for payload in rx {
                // Do not log collector responses or event content.
                let error = agent.post(&url).send_json(payload).is_err();
                if error && !failed {
                    crate::log(
                        "history OTLP: metadata delivery failed; records dropped until recovery",
                    );
                } else if !error && failed {
                    crate::log("history OTLP: metadata delivery recovered");
                }
                failed = error;
            }
        });
        Self {
            tx,
            saturated: std::sync::atomic::AtomicBool::new(false),
        }
    }
    pub fn submit(&self, event: &Value) {
        let Some(payload) = metadata(event) else {
            return;
        };
        let full = self.tx.try_send(payload).is_err();
        if full
            && !self
                .saturated
                .swap(true, std::sync::atomic::Ordering::Relaxed)
        {
            crate::log("history OTLP: queue full or worker unavailable; metadata dropped");
        } else if !full {
            self.saturated
                .store(false, std::sync::atomic::Ordering::Relaxed);
        }
    }
}

fn string(key: &str, value: &str) -> Value {
    json!({"key":key,"value":{"stringValue":value}})
}
/// Closed attribute allowlist: never copy the event object or arbitrary data.
fn metadata(event: &Value) -> Option<Value> {
    let kind = event["kind"].as_str()?;
    if !matches!(
        kind,
        "spawn"
            | "mux.send"
            | "mail.sent"
            | "mail.local"
            | "lineage.start"
            | "lineage.change"
            | "lineage.end"
            | "land"
            | "deploy"
    ) || event["schema"] != 1
    {
        return None;
    }
    let ts = event["ts"].as_u64()?.checked_mul(1_000_000)?;
    let host = event["host"].as_str()?;
    let mut attrs = vec![
        json!({"key":"vega.schema","value":{"intValue":"1"}}),
        string("vega.event.id", event["id"].as_str()?),
    ];
    for key in ["project", "bead", "pane", "session"] {
        if let Some(value) = event[key].as_str() {
            attrs.push(string(&format!("vega.{key}"), value));
        }
    }
    Some(json!({"resourceLogs":[{
        "resource":{"attributes":[string("service.name","vega.history"), string("host.name",host)]},
        "scopeLogs":[{"scope":{"name":"vega.history"},"logRecords":[{
            "timeUnixNano":ts.to_string(),"severityNumber":9,"severityText":"INFO",
            "eventName":kind,"body":{"stringValue":kind},"attributes":attrs
        }]}]
    }]}))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn closed_allowlist_excludes_all_event_content() {
        let e = json!({"schema":1,"id":"id","ts":1234,"host":"h","kind":"mail.sent","project":"p","bead":"b","pane":"w:p","session":"s","extra":"secret-top-level","data":{"body":"secret-body","brief":"secret-brief","headers":{"Authorization":"secret-header"},"cwd":"secret-path","text":"secret-text","value":"secret-lineage"}});
        let payload = metadata(&e).unwrap();
        assert!(!payload.to_string().contains("secret"));
        let resource = &payload["resourceLogs"][0];
        assert_eq!(
            resource["resource"]["attributes"].as_array().unwrap().len(),
            2
        );
        let record = &resource["scopeLogs"][0]["logRecords"][0];
        assert_eq!(record["body"]["stringValue"], "mail.sent");
        assert_eq!(record["timeUnixNano"], "1234000000");
        let keys: Vec<_> = record["attributes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a["key"].as_str().unwrap())
            .collect();
        assert_eq!(
            keys,
            [
                "vega.schema",
                "vega.event.id",
                "vega.project",
                "vega.bead",
                "vega.pane",
                "vega.session"
            ]
        );
        let mut unknown = e;
        unknown["kind"] = json!("secret-kind");
        assert!(metadata(&unknown).is_none());
        unknown["kind"] = json!("spawn");
        unknown["project"] = Value::Null;
        assert_eq!(
            metadata(&unknown).unwrap()["resourceLogs"][0]["scopeLogs"][0]["logRecords"][0]
                ["attributes"]
                .as_array()
                .unwrap()
                .len(),
            5
        );
    }
    #[test]
    fn http_worker_sends_only_metadata_to_logs_route() {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let sink = Sink::start(Config {
            endpoint,
            timeout: std::time::Duration::from_millis(500),
        });
        sink.submit(&json!({"schema":1,"id":"id","ts":1,"host":"h","kind":"spawn","data":{"brief":"secret"}}));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let (mut socket, _) = loop {
            match listener.accept() {
                Ok(pair) => break pair,
                Err(e)
                    if e.kind() == std::io::ErrorKind::WouldBlock
                        && std::time::Instant::now() < deadline =>
                {
                    std::thread::sleep(std::time::Duration::from_millis(10))
                }
                Err(e) => panic!("OTLP test listener: {e}"),
            }
        };
        socket
            .set_read_timeout(Some(std::time::Duration::from_secs(2)))
            .unwrap();
        let mut bytes = vec![];
        let mut buf = [0; 4096];
        loop {
            let n = socket.read(&mut buf).unwrap();
            assert_ne!(n, 0);
            bytes.extend_from_slice(&buf[..n]);
            if let Some(split) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                let head = String::from_utf8_lossy(&bytes[..split]);
                let length: usize = head
                    .lines()
                    .find_map(|s| {
                        s.to_ascii_lowercase()
                            .strip_prefix("content-length: ")
                            .map(str::to_owned)
                    })
                    .unwrap()
                    .parse()
                    .unwrap();
                if bytes.len() >= split + 4 + length {
                    break;
                }
            }
        }
        let request = String::from_utf8(bytes).unwrap();
        assert!(request.starts_with("POST /v1/logs HTTP/1.1"));
        assert!(!request.contains("secret"));
        assert!(request.contains("resourceLogs"));
        socket
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}")
            .unwrap();
    }
}
