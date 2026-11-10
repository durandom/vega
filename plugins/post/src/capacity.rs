//! Periodic OTLP gauges for the host inbox and central history capacity.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_nats::jetstream::{self, stream::Info};
use serde_json::{json, Value};

const INTERVAL: Duration = Duration::from_secs(5 * 60);
const INBOX_STREAM_PREFIX: &str = "VEGA_INBOX_";
const HISTORY_STREAM: &str = "VEGA_HISTORY";

pub fn spawn() {
    let config = match vega_herdr::otlp::load() {
        Ok(Some(config)) => config,
        Ok(None) => return,
        Err(error) => {
            crate::log(&format!("capacity OTLP: off ({error})"));
            return;
        }
    };
    if let Err(error) = std::thread::Builder::new()
        .name("vega-capacity-otlp".into())
        .spawn(move || {
            let runtime = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime,
                Err(error) => {
                    crate::log(&format!("capacity OTLP: cannot start runtime: {error}"));
                    return;
                }
            };
            runtime.block_on(run(config));
        })
    {
        crate::log(&format!("capacity OTLP: cannot start worker: {error}"));
    }
}

async fn run(config: vega_herdr::otlp::Config) {
    let host = crate::host();
    let agent = ureq::AgentBuilder::new().timeout(config.timeout).build();
    let mut interval = tokio::time::interval(INTERVAL);
    loop {
        interval.tick().await;
        let client = match crate::connect("capacity", None).await {
            Ok(client) => client,
            Err(error) => {
                log_once(&format!("broker connection failed: {}", error.message));
                continue;
            }
        };
        let js = vega_registry::broker_context(client);
        let mut points = Vec::new();
        let inbox_name = format!("{INBOX_STREAM_PREFIX}{host}");
        match stream_info(&js, &inbox_name).await {
            Ok(info) => points.extend(stream_points("inbox", &host, &info)),
            Err(error) => log_once(&format!("stream info {inbox_name} unavailable: {error}")),
        }
        match stream_info(&js, HISTORY_STREAM).await {
            Ok(info) => points.extend(stream_points("history", &host, &info)),
            Err(error) => log_once(&format!(
                "stream info {HISTORY_STREAM} unavailable: {error}"
            )),
        }
        if points.is_empty() {
            continue;
        }
        let payload = metrics_payload(&host, points);
        if let Err(error) = export_metrics(&agent, &config.endpoint, payload) {
            log_once(&format!("OTLP export failed: {error}"));
        }
    }
}

fn log_once(message: &str) {
    static SAID: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
    let first = {
        let Ok(mut said) = SAID.lock() else { return };
        if said.iter().any(|previous| previous == message) {
            false
        } else {
            said.push(message.to_string());
            true
        }
    };
    if first {
        crate::log(&format!("capacity OTLP: {message}"));
    }
}

fn export_metrics(agent: &ureq::Agent, endpoint: &str, payload: Value) -> Result<(), String> {
    let url = format!("{}/v1/metrics", endpoint.trim_end_matches('/'));
    agent
        .post(&url)
        .send_json(payload)
        .map(|_| ())
        .map_err(|error| format!("{url}: {error}"))
}

async fn stream_info(js: &jetstream::Context, name: &str) -> Result<Info, String> {
    js.get_stream_no_info(name)
        .await
        .map_err(|error| error.to_string())?
        .get_info()
        .await
        .map_err(|error| error.to_string())
}

fn stream_points(kind: &str, host: &str, info: &Info) -> Vec<Value> {
    let attributes = vec![
        json!({"key":"host.name", "value":{"stringValue":host}}),
        json!({"key":"stream", "value":{"stringValue":kind}}),
        json!({"key":"stream.name", "value":{"stringValue":info.config.name}}),
    ];
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
        .to_string();
    [
        ("vega.jetstream.stream.bytes", "By", info.state.bytes as i64),
        (
            "vega.jetstream.stream.messages",
            "{message}",
            info.state.messages as i64,
        ),
        (
            "vega.jetstream.stream.max_bytes",
            "By",
            info.config.max_bytes,
        ),
    ]
    .into_iter()
    .map(|(name, unit, value)| {
        json!({
            "name": name,
            "unit": unit,
            "gauge": {"dataPoints": [{
                "timeUnixNano": timestamp,
                "asInt": value.to_string(),
                "attributes": attributes,
            }]}
        })
    })
    .collect()
}

fn metrics_payload(host: &str, metrics: Vec<Value>) -> Value {
    json!({
        "resourceMetrics": [{
            "resource": {"attributes": [
                {"key":"service.name", "value":{"stringValue":"vega.history"}},
                {"key":"host.name", "value":{"stringValue":host}},
            ]},
            "scopeMetrics": [{
                "scope": {"name":"vega.post.capacity"},
                "metrics": metrics,
            }]
        }]
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::{self, Entry};
    use crate::registry_read_test::{connect, start, user};

    #[test]
    fn emits_stream_bytes_messages_and_max_bytes_as_otlp_gauges() {
        let info: Info = serde_json::from_value(json!({
            "config": {
                "name":"VEGA_INBOX_proteus", "max_bytes":10737418240i64,
                "discard":"old", "retention":"limits", "storage":"file", "num_replicas":1
            },
            "created":"2026-11-05T00:00:00Z",
            "state": {
                "messages":17, "bytes":1234, "first_seq":1,
                "first_ts":"2026-11-05T00:00:00Z", "last_seq":17,
                "last_ts":"2026-11-05T00:00:00Z", "consumer_count":1
            }
        }))
        .unwrap();
        let metrics = stream_points("inbox", "proteus", &info);
        assert_eq!(metrics.len(), 3);
        assert_eq!(metrics[0]["name"], "vega.jetstream.stream.bytes");
        assert_eq!(metrics[0]["unit"], "By");
        assert_eq!(metrics[0]["gauge"]["dataPoints"][0]["asInt"], "1234");
        assert_eq!(metrics[1]["name"], "vega.jetstream.stream.messages");
        assert_eq!(metrics[1]["gauge"]["dataPoints"][0]["asInt"], "17");
        assert_eq!(metrics[2]["name"], "vega.jetstream.stream.max_bytes");
        assert_eq!(metrics[2]["gauge"]["dataPoints"][0]["asInt"], "10737418240");
        assert_eq!(
            metrics[0]["gauge"]["dataPoints"][0]["attributes"][0]["value"]["stringValue"],
            "proteus"
        );
        let payload = metrics_payload("proteus", metrics);
        assert_eq!(
            payload["resourceMetrics"][0]["scopeMetrics"][0]["metrics"]
                .as_array()
                .unwrap()
                .len(),
            3
        );
    }

    #[test]
    fn own_inbox_info_is_allowed_and_history_info_is_conditional() {
        let dir = tempfile::tempdir().unwrap();
        let (admin_seed, admin_key) = user();
        let (host_seed, host_key) = user();
        let authz = auth::render(
            &[Entry {
                host: "proteus".into(),
                key: host_key,
            }],
            Some(&admin_key),
            &["history".into(), "proteus".into()],
            &[],
            None,
            &[],
            &[],
            &[],
            None,
        );
        let (_server, url) = start(dir.path(), &authz);
        crate::runtime().unwrap().block_on(async {
            let admin = connect(&url, &admin_seed, crate::ADMIN_INBOX).await;
            let admin_js = jetstream::new(admin);
            admin_js
                .create_stream(crate::history::config())
                .await
                .unwrap();
            admin_js
                .create_stream(crate::stream_config("proteus"))
                .await
                .unwrap();

            let host = connect(&url, &host_seed, "_INBOX_proteus").await;
            let host_js = jetstream::new(host);
            let inbox = stream_info(&host_js, "VEGA_INBOX_proteus").await.unwrap();
            assert_eq!(stream_points("inbox", "proteus", &inbox).len(), 3);
            assert!(stream_info(&host_js, HISTORY_STREAM).await.is_err());

            let admin_history = stream_info(&admin_js, HISTORY_STREAM).await.unwrap();
            assert_eq!(stream_points("history", "proteus", &admin_history).len(), 3);
        });
    }

    #[test]
    fn sends_otlp_http_metrics_to_signal_endpoint() {
        use std::io::{Read, Write};
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            let mut buffer = [0; 4096];
            loop {
                let count = socket.read(&mut buffer).unwrap();
                request.extend_from_slice(&buffer[..count]);
                if request.windows(4).any(|window| window == b"\r\n\r\n") {
                    let split = request
                        .windows(4)
                        .position(|window| window == b"\r\n\r\n")
                        .unwrap();
                    let headers = String::from_utf8_lossy(&request[..split]);
                    let length: usize = headers
                        .lines()
                        .find_map(|line| {
                            line.to_ascii_lowercase()
                                .strip_prefix("content-length: ")
                                .and_then(|value| value.parse().ok())
                        })
                        .unwrap();
                    if request.len() >= split + 4 + length {
                        break;
                    }
                }
            }
            socket
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                .unwrap();
            request
        });
        let payload = metrics_payload("proteus", vec![json!({"name":"vega.test"})]);
        export_metrics(&ureq::Agent::new(), &format!("http://{address}/"), payload).unwrap();
        let request = server.join().unwrap();
        let split = request
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
            .unwrap();
        let headers = String::from_utf8_lossy(&request[..split]);
        assert!(headers.starts_with("POST /v1/metrics HTTP/1.1"));
        assert!(headers
            .to_ascii_lowercase()
            .contains("content-type: application/json"));
        let body: Value = serde_json::from_slice(&request[split + 4..]).unwrap();
        assert_eq!(
            body["resourceMetrics"][0]["resource"]["attributes"][1]["value"]["stringValue"],
            "proteus"
        );
    }
}
