//! Lossy, bounded metadata-only OTLP logs. Never queues history bodies or retries
//! central publication. Collector stalls cannot hold the history outbox worker.
use serde_json::{json, Value};
use std::sync::mpsc;
use vega_herdr::otlp::Config;

pub struct Sink {
    tx: mpsc::SyncSender<Value>,
    saturated: std::sync::atomic::AtomicBool,
    log: fn(&str),
}
impl Sink {
    pub fn configured() -> Option<Self> {
        Self::from_config(vega_herdr::otlp::load(), crate::log)
    }
    fn from_config(config: Result<Option<Config>, String>, log: fn(&str)) -> Option<Self> {
        match config {
            Ok(Some(config)) => Some(Self::start(config, log)),
            Ok(None) => None,
            Err(_) => {
                log("history OTLP: invalid configuration; export disabled (see vega/otlp.toml)");
                None
            }
        }
    }
    fn start(config: Config, log: fn(&str)) -> Self {
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
                    log("history OTLP: metadata delivery failed; records dropped until recovery");
                } else if !error && failed {
                    log("history OTLP: metadata delivery recovered");
                }
                failed = error;
            }
        });
        Self {
            tx,
            saturated: std::sync::atomic::AtomicBool::new(false),
            log,
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
            (self.log)("history OTLP: queue full or worker unavailable; metadata dropped");
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
            | "land.phase"
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
    static TEST_SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());
    static LOGS: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
    fn capture(message: &str) {
        LOGS.lock().unwrap().push(message.to_owned());
    }
    fn sample(kind: &str) -> Value {
        json!({"schema":1,"id":"id","ts":1,"host":"h","kind":kind,"data":{"body":"secret"}})
    }
    fn listener() -> std::net::TcpListener {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        listener
    }
    fn accept(listener: &std::net::TcpListener) -> std::net::TcpStream {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            match listener.accept() {
                Ok((socket, _)) => {
                    // Accepted sockets inherit O_NONBLOCK on macOS; only the
                    // listener uses polling. Request reads must wait for bytes.
                    socket.set_nonblocking(false).unwrap();
                    return socket;
                }
                Err(e)
                    if e.kind() == std::io::ErrorKind::WouldBlock
                        && std::time::Instant::now() < deadline =>
                {
                    std::thread::sleep(std::time::Duration::from_millis(5))
                }
                Err(e) => panic!("test collector: {e}"),
            }
        }
    }
    fn respond(mut socket: std::net::TcpStream, status: &str) {
        use std::io::{Read, Write};
        socket
            .set_read_timeout(Some(std::time::Duration::from_secs(2)))
            .unwrap();
        let mut bytes = Vec::new();
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
        write!(
            socket,
            "HTTP/1.1 {status}\r\nConnection: close\r\nContent-Length: 2\r\n\r\n{{}}"
        )
        .unwrap();
    }
    fn wait_logs(count: usize) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while LOGS.lock().unwrap().len() < count {
            assert!(
                std::time::Instant::now() < deadline,
                "missing OTLP diagnostic"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }
    #[test]
    fn stalled_collector_cannot_block_submit_and_saturation_logs_once() {
        let _guard = TEST_SERIAL.lock().unwrap();
        LOGS.lock().unwrap().clear();
        let listener = listener();
        let sink = std::sync::Arc::new(Sink::start(
            Config {
                endpoint: format!("http://{}", listener.local_addr().unwrap()),
                timeout: std::time::Duration::from_secs(5),
            },
            capture,
        ));
        sink.submit(&sample("spawn"));
        // Hold this accepted connection unread: the worker waits for HTTP,
        // so exactly 64 subsequent records fit while submissions stay bounded.
        let socket = accept(&listener);
        let producer = sink.clone();
        let (done_tx, done_rx) = mpsc::channel();
        let submitter = std::thread::spawn(move || {
            for _ in 0..256 {
                producer.submit(&sample("spawn"));
            }
            done_tx.send(()).unwrap();
        });
        done_rx
            .recv_timeout(std::time::Duration::from_millis(500))
            .expect("collector stall blocked history publication");
        submitter.join().unwrap();
        let logs = LOGS.lock().unwrap().clone();
        assert_eq!(logs.len(), 1, "{logs:?}");
        assert!(logs[0].contains("queue full"));
        respond(socket, "503 Unavailable");
        for _ in 0..64 {
            respond(accept(&listener), "503 Unavailable");
        }
        drop(sink);
        wait_logs(2);
        let logs = LOGS.lock().unwrap().clone();
        assert_eq!(logs.iter().filter(|s| s.contains("queue full")).count(), 1);
        assert_eq!(
            logs.iter()
                .filter(|s| s.contains("delivery failed"))
                .count(),
            1
        );
    }
    #[test]
    fn delivery_failure_logs_once_then_recovery_logs_once() {
        let _guard = TEST_SERIAL.lock().unwrap();
        LOGS.lock().unwrap().clear();
        let listener = listener();
        let sink = Sink::start(
            Config {
                endpoint: format!("http://{}", listener.local_addr().unwrap()),
                timeout: std::time::Duration::from_secs(2),
            },
            capture,
        );
        for _ in 0..3 {
            sink.submit(&sample("land.phase"));
        }
        for status in ["503 Unavailable", "503 Unavailable", "200 OK"] {
            respond(accept(&listener), status);
        }
        drop(sink);
        wait_logs(2);
        let logs = LOGS.lock().unwrap().clone();
        assert_eq!(logs.len(), 2, "{logs:?}");
        assert!(logs[0].contains("delivery failed"));
        assert!(logs[1].contains("delivery recovered"));
    }
    #[test]
    fn disabled_and_invalid_configuration_never_start_an_exporter() {
        let _guard = TEST_SERIAL.lock().unwrap();
        LOGS.lock().unwrap().clear();
        assert!(Sink::from_config(Ok(None), capture).is_none());
        assert!(LOGS.lock().unwrap().is_empty());
        assert!(
            Sink::from_config(Err("invalid config with secret detail".into()), capture).is_none()
        );
        let logs = LOGS.lock().unwrap().clone();
        assert_eq!(logs.len(), 1);
        assert!(logs[0].contains("invalid configuration"));
        assert!(!logs[0].contains("secret"));
    }
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
        let sink = Sink::start(
            Config {
                endpoint,
                timeout: std::time::Duration::from_millis(500),
            },
            crate::log,
        );
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
        socket.set_nonblocking(false).unwrap();
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
