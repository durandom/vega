//! A transport adapter for agent-browser 0.43.0. Each daemon discovers one
//! verified page. Browser-wide operations and fallback target creation are
//! rejected even if the runtime requests them during automatic recovery.
use std::collections::BTreeMap;
use std::env;
use std::io::{self, BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tungstenite::{
    accept_hdr, client,
    handshake::server::{Request, Response},
    Message, WebSocket,
};
use vega_herdr::Failure;

use super::{failure, process, stale, wire::Cdp, Browser, Project};

// Leave room for the error to reach agent-browser before its 30 s IPC deadline.
const CAPTURE_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Serialize, Deserialize)]
pub(super) struct Binding {
    pub scope: Scope,
    pub target: String,
}

#[derive(Serialize, Deserialize)]
pub(super) enum Scope {
    Dev(Browser, Project),
    Shared { port: u16, path: String },
}

impl Binding {
    fn generation(&self) -> &str {
        match &self.scope {
            Scope::Dev(browser, _) => &browser.path,
            Scope::Shared { path, .. } => path,
        }
    }

    fn open(&self) -> Result<(Cdp, Value), Failure> {
        match &self.scope {
            Scope::Dev(browser, project) => {
                if !process::running(browser, &browser.profile)? {
                    return Err(stale("The dev browser has restarted"));
                }
                let mut cdp = Cdp::connect(browser)?;
                let info = cdp.info(project, &self.target)?;
                Ok((cdp, info))
            }
            Scope::Shared { port, path } => {
                let mut cdp =
                    Cdp::endpoint(*port, path).map_err(|_| stale("Shared browser disconnected"))?;
                let result = cdp
                    .call("Target.getTargetInfo", json!({"targetId": self.target}))
                    .map_err(|_| stale("Shared tab disappeared"))?;
                let info = &result["targetInfo"];
                if info["targetId"] != self.target || info["type"] != "page" {
                    return Err(stale("Invalid shared tab"));
                }
                Ok((cdp, info.clone()))
            }
        }
    }
}

pub(super) fn start(binding: &Binding) -> Result<u16, Failure> {
    let mut child = Command::new("nohup")
        .arg(env::current_exe().map_err(failure)?)
        .arg(super::PROXY)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(failure)?;
    serde_json::to_writer(child.stdin.take().expect("piped stdin"), binding).map_err(failure)?;
    let mut port = String::new();
    BufReader::new(child.stdout.take().expect("piped stdout"))
        .read_line(&mut port)
        .map_err(failure)?;
    port.trim()
        .parse()
        .map_err(|_| failure("The dev attachment adapter did not start"))
}

pub fn serve() -> Result<(), Failure> {
    let binding: Binding = serde_json::from_reader(io::stdin().lock()).map_err(failure)?;
    let listener = TcpListener::bind(("127.0.0.1", 0)).map_err(failure)?;
    listener.set_nonblocking(true).map_err(failure)?;
    binding.open()?;
    println!("{}", listener.local_addr().map_err(failure)?.port());
    io::stdout().flush().map_err(failure)?;
    let binding = Arc::new(binding);
    let mut checked = Instant::now();
    loop {
        if checked.elapsed() > Duration::from_secs(5) {
            // Terminate on confirmed loss of the binding. A transport timeout
            // does not change ownership; each new relay still revalidates it.
            match binding.open() {
                Err(error) if error.code == "stale_dev_session" => return Err(error),
                _ => {}
            }
            checked = Instant::now();
        }
        match listener.accept() {
            Ok((stream, _)) => {
                let binding = Arc::clone(&binding);
                thread::spawn(move || {
                    let _ = relay(stream, &binding, CAPTURE_TIMEOUT);
                });
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(50))
            }
            Err(e) => return Err(failure(e)),
        }
    }
}

pub(super) fn alive(port: u16, binding: &Binding) -> bool {
    let address = SocketAddr::from(([127, 0, 0, 1], port));
    let timeout = Duration::from_millis(250);
    let Ok(stream) = TcpStream::connect_timeout(&address, timeout) else {
        return false;
    };
    if stream.set_read_timeout(Some(timeout)).is_err()
        || stream.set_write_timeout(Some(timeout)).is_err()
    {
        return false;
    }
    let url = format!("ws://{address}/health/{}", binding.target);
    client(url, stream).is_ok_and(|(_, response)| {
        response
            .headers()
            .get("x-vega-dev-generation")
            .is_some_and(|value| value == binding.generation())
    })
}

// tungstenite's handshake callback fixes the HTTP error response type.
#[allow(clippy::result_large_err)]
fn relay(stream: TcpStream, binding: &Binding, capture_timeout: Duration) -> Result<(), Failure> {
    // Darwin inherits O_NONBLOCK from the listener. The relay uses socket
    // timeouts, including when a large reply temporarily fills the send buffer.
    stream.set_nonblocking(false).map_err(failure)?;
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .map_err(failure)?;
    stream
        .set_write_timeout(Some(Duration::from_secs(3)))
        .map_err(failure)?;
    let path = format!("/devtools/browser/{}", binding.target);
    let health_path = format!("/health/{}", binding.target);
    let mut health = false;
    let mut downstream = accept_hdr(stream, |req: &Request, mut res: Response| {
        health = req.uri().path() == health_path;
        if (!health && req.uri().path() != path) || req.headers().contains_key("origin") {
            return Err(tungstenite::http::Response::builder()
                .status(404)
                .body(None)
                .expect("static HTTP response"));
        }
        if health {
            if let Ok(generation) = binding.generation().parse() {
                res.headers_mut()
                    .insert("x-vega-dev-generation", generation);
            }
        }
        Ok(res)
    })
    .map_err(failure)?;
    if health {
        return Ok(());
    }
    // The runtime may reconnect, but only to this same verified binding.
    let (mut cdp, info) = binding.open()?;
    let attached = cdp.string(
        "Target.attachToTarget",
        json!({"targetId": binding.target, "flatten": true}),
        "sessionId",
    )?;
    let version = cdp.call("Browser.getVersion", json!({}))?;
    let window = cdp.call(
        "Browser.getWindowForTarget",
        json!({"targetId": binding.target}),
    )?;
    let mut upstream = cdp.socket;
    let delay = Some(Duration::from_millis(3));
    upstream
        .get_mut()
        .set_read_timeout(delay)
        .map_err(failure)?;
    downstream
        .get_mut()
        .set_read_timeout(delay)
        .map_err(failure)?;
    let mut pending = BTreeMap::<u64, Option<Instant>>::new();
    loop {
        let now = Instant::now();
        for (id, _) in pending.extract_if(.., |_, deadline| deadline.is_some_and(|at| now >= at)) {
            send(
                &mut downstream,
                &json!({"id": id, "sessionId": attached, "error": {
                    "code": -32000,
                    "message": "Chrome did not complete Page.captureScreenshot. Bring the dev tab and its window to the foreground, then retry."
                }}),
            )?;
        }
        if let Some(request) = read(&mut downstream)? {
            let Some(id) = request["id"].as_u64() else {
                return Err(failure("Invalid CDP request ID"));
            };
            let params = &request["params"];
            let method = request["method"].as_str().unwrap_or_default();
            let target_matches = params["targetId"] == binding.target;
            let session_id = request["sessionId"].as_str();
            let result = match (session_id, method) {
                (None, "Target.getTargets") => Some(json!({"targetInfos": [&info]})),
                (None, "Target.getTargetInfo") if target_matches => {
                    Some(json!({"targetInfo": &info}))
                }
                (None, "Target.attachToTarget") if target_matches && params["flatten"] == true => {
                    Some(json!({"sessionId": attached}))
                }
                (None, "Target.setDiscoverTargets" | "Target.setAutoAttach") => Some(json!({})),
                (None, "Browser.getVersion") => Some(version.clone()),
                (None, "Browser.getWindowForTarget") if target_matches => Some(window.clone()),
                _ if (session_id == Some(&attached)
                    && page_method(method)
                    && !internal_navigation(method, params))
                    || (session_id.is_none()
                        && method == "Target.activateTarget"
                        && target_matches) =>
                {
                    send(&mut upstream, &request)?;
                    let deadline = (method == "Page.captureScreenshot")
                        .then(|| Instant::now() + capture_timeout);
                    pending.insert(id, deadline);
                    None
                }
                _ => {
                    send(
                        &mut downstream,
                        &json!({"id": id, "error": {
                            "code": -32601, "message": "CDP operation is outside the dev tab binding"
                        }}),
                    )?;
                    None
                }
            };
            if let Some(result) = result {
                send(&mut downstream, &json!({"id": id, "result": result}))?;
            }
        }
        if let Some(reply) = read(&mut upstream)? {
            if reply["method"] == "Target.detachedFromTarget"
                && reply["params"]["sessionId"] == attached
            {
                return Err(stale("The attached dev tab is gone"));
            }
            let should_forward = match reply["id"].as_u64() {
                // A capture can finish after its deadline. Keep it from
                // generating a second response for the downstream request.
                Some(id) => pending.remove(&id).is_some(),
                None => reply["id"].is_null() && reply["sessionId"] == attached,
            };
            if should_forward {
                send(&mut downstream, &reply)?;
            }
        }
    }
}

fn internal_navigation(method: &str, params: &Value) -> bool {
    if method != "Page.navigate" {
        return false;
    }
    // Keep the extension's privileged APIs and Chrome's management UI out
    // of the scoped page WebSocket. The dedicated browser UI retains access.
    let url = params["url"].as_str().unwrap_or_default();
    let scheme = url
        .trim_start_matches(|c: char| c <= ' ')
        .split(':')
        .next()
        .unwrap_or_default()
        .replace(['\t', '\r', '\n'], "")
        .to_ascii_lowercase();
    matches!(
        scheme.as_str(),
        "chrome-extension" | "chrome" | "chrome-untrusted" | "devtools"
    )
}

fn page_method(method: &str) -> bool {
    // No browser, target discovery, storage partition, or system operations.
    let domain = method.split_once('.').map(|(domain, _)| domain);
    matches!(
        domain,
        Some(
            "Accessibility"
                | "Animation"
                | "CSS"
                | "DOM"
                | "DOMDebugger"
                | "DOMStorage"
                | "Emulation"
                | "Fetch"
                | "Input"
                | "Log"
                | "Network"
                | "Overlay"
                | "Page"
                | "Performance"
                | "Runtime"
                | "WebMCP"
        )
    ) && !matches!(
        method,
        "Page.setDownloadBehavior" | "Page.close" | "Page.crash"
    )
}

fn read(socket: &mut WebSocket<TcpStream>) -> Result<Option<Value>, Failure> {
    match socket.read() {
        Ok(Message::Text(raw)) => serde_json::from_str(&raw).map(Some).map_err(failure),
        Ok(Message::Close(_)) => Err(failure("Dev attachment disconnected")),
        Ok(_) => Ok(None),
        Err(tungstenite::Error::Io(e))
            if matches!(
                e.kind(),
                io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
            ) =>
        {
            Ok(None)
        }
        Err(e) => Err(failure(e)),
    }
}

fn send(socket: &mut WebSocket<TcpStream>, value: &Value) -> Result<(), Failure> {
    socket
        .send(Message::text(value.to_string()))
        .map_err(failure)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_fake_relay(
        capture_timeout: Duration,
        chrome_commands: impl FnOnce(&mut WebSocket<TcpStream>) + Send,
        agent_commands: impl FnOnce(&mut WebSocket<TcpStream>),
    ) {
        let chrome = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        chrome.set_nonblocking(true).unwrap();
        let adapter = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let address = adapter.local_addr().unwrap();
        let binding = Binding {
            scope: Scope::Shared {
                port: chrome.local_addr().unwrap().port(),
                path: "/devtools/browser/fake".into(),
            },
            target: "owned".into(),
        };
        thread::scope(|threads| {
            threads.spawn(move || {
                let deadline = Instant::now() + Duration::from_secs(5);
                let stream = loop {
                    match chrome.accept() {
                        Ok((stream, _)) => break stream,
                        Err(error)
                            if error.kind() == io::ErrorKind::WouldBlock
                                && Instant::now() < deadline =>
                        {
                            thread::sleep(Duration::from_millis(5));
                        }
                        Err(error) => panic!("Expected upstream connection: {error}"),
                    }
                };
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut socket = tungstenite::accept(stream).unwrap();
                for (method, result) in [
                    (
                        "Target.getTargetInfo",
                        json!({"targetInfo": {"targetId": "owned", "type": "page"}}),
                    ),
                    ("Target.attachToTarget", json!({"sessionId": "attached"})),
                    ("Browser.getVersion", json!({})),
                    ("Browser.getWindowForTarget", json!({})),
                ] {
                    let request = read(&mut socket).unwrap().unwrap();
                    assert_eq!(request["method"], method);
                    send(&mut socket, &json!({"id": request["id"], "result": result})).unwrap();
                }
                chrome_commands(&mut socket);
            });
            threads.spawn(move || {
                let (stream, _) = adapter.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                // Let the handshake arrive before reproducing Darwin's socket
                // mode so the test exercises backpressure on the reply.
                assert_eq!(stream.peek(&mut [0]).unwrap(), 1);
                // Reproduce Darwin's accepted socket mode on every platform.
                stream.set_nonblocking(true).unwrap();
                let _ = relay(stream, &binding, capture_timeout);
            });

            let stream = TcpStream::connect(address).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let (mut socket, _) =
                client(format!("ws://{address}/devtools/browser/owned"), stream).unwrap();
            agent_commands(&mut socket);
        });
    }

    #[test]
    fn screenshot_reply_survives_downstream_backpressure() {
        let size = 12 * 1024 * 1024;
        let (capturing, screenshot_requested) = std::sync::mpsc::channel();
        with_fake_relay(
            CAPTURE_TIMEOUT,
            move |socket| {
                let request = read(socket).unwrap().unwrap();
                assert_eq!(request["method"], "Page.captureScreenshot");
                assert_eq!(request["sessionId"], "attached");
                capturing.send(()).unwrap();
                send(socket, &json!({
                    "id": request["id"], "sessionId": "attached", "result": {"data": "a".repeat(size)}
                })).unwrap();
                // Keep the upstream available for the next command on this relay.
                let _ = socket.read();
            },
            |socket| {
                send(
                    socket,
                    &json!({"id": 51, "sessionId": "attached", "method": "Page.captureScreenshot"}),
                )
                .unwrap();
                screenshot_requested
                    .recv_timeout(Duration::from_secs(5))
                    .unwrap();
                thread::sleep(Duration::from_millis(400));
                let reply = read(socket).unwrap().unwrap();
                assert_eq!(reply["id"], 51);
                assert_eq!(reply["result"]["data"].as_str().unwrap().len(), size);
                send(socket, &json!({"id": 52, "method": "Browser.getVersion"})).unwrap();
                assert_eq!(read(socket).unwrap().unwrap()["id"], 52);
            },
        );
    }

    #[test]
    fn missing_capture_reply_times_out_without_losing_the_attachment() {
        with_fake_relay(
            Duration::from_millis(100),
            |socket| {
                let capture = read(socket).unwrap().unwrap();
                assert_eq!(capture["method"], "Page.captureScreenshot");
                let eval = read(socket).unwrap().unwrap();
                assert_eq!(eval["method"], "Runtime.evaluate");
                send(
                    socket,
                    &json!({
                        "id": capture["id"], "sessionId": "attached", "result": {"data": "late"}
                    }),
                )
                .unwrap();
                send(socket, &json!({
                    "id": eval["id"], "sessionId": "attached", "result": {"result": {"value": 42}}
                })).unwrap();
                let _ = socket.read();
            },
            |socket| {
                send(
                    socket,
                    &json!({"id": 51, "sessionId": "attached", "method": "Page.captureScreenshot"}),
                )
                .unwrap();
                let reply = read(socket).unwrap().unwrap();
                assert_eq!(reply["id"], 51);
                assert_eq!(reply["sessionId"], "attached");
                assert_eq!(reply["error"]["code"], -32000);
                send(
                    socket,
                    &json!({"id": 52, "sessionId": "attached", "method": "Runtime.evaluate"}),
                )
                .unwrap();
                let reply = read(socket).unwrap().unwrap();
                assert_eq!(reply["id"], 52);
                assert_eq!(reply["result"]["result"]["value"], 42);
            },
        );
    }
}
