//! A transport adapter for agent-browser 0.43.0. Each daemon discovers one
//! verified page. Browser-wide operations and fallback target creation are
//! rejected even if the runtime requests them during automatic recovery.
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

#[derive(Serialize, Deserialize)]
pub(super) struct Binding {
    pub browser: Browser,
    pub project: Project,
    pub target: String,
}

impl Binding {
    fn open(&self) -> Result<(Cdp, Value), Failure> {
        if !process::running(&self.browser, &self.browser.profile)? {
            return Err(stale("The dev browser has restarted"));
        }
        let mut cdp = Cdp::connect(&self.browser)?;
        let info = cdp.info(&self.project, &self.target)?;
        Ok((cdp, info))
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
                    let _ = relay(stream, &binding);
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
            .is_some_and(|value| value == binding.browser.path.as_str())
    })
}

// tungstenite's handshake callback fixes the HTTP error response type.
#[allow(clippy::result_large_err)]
fn relay(stream: TcpStream, binding: &Binding) -> Result<(), Failure> {
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
            if let Ok(generation) = binding.browser.path.parse() {
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
    loop {
        if let Some(request) = read(&mut downstream)? {
            let id = &request["id"];
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
                (Some(id), _) if id == attached && page_method(method) => {
                    send(&mut upstream, &request)?;
                    None
                }
                (None, "Target.activateTarget") if target_matches => {
                    send(&mut upstream, &request)?;
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
            if !reply["id"].is_null() || reply["sessionId"] == attached {
                send(&mut downstream, &reply)?;
            }
        }
    }
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
