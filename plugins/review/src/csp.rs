//! Keep CSP bypass attached to the review tab for the lifetime of its inbox.
use std::net::TcpStream;
use std::path::Path;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tungstenite::{stream::MaybeTlsStream, Message, WebSocket};

use crate::{agent_browser, fs, thread, write_private, Failure};

type Socket = WebSocket<MaybeTlsStream<TcpStream>>;
const TIMEOUT: Duration = Duration::from_secs(30);

fn failure(message: impl std::fmt::Display) -> Failure {
    Failure::new(
        "csp_setup_failed",
        format!("review CSP bypass: {message}"),
        "vega review stop, then start again",
    )
}

fn wait_file(path: &Path) -> Result<String, Failure> {
    let deadline = Instant::now() + TIMEOUT;
    loop {
        if let Ok(text) = fs::read_to_string(path) {
            // The writer may still be filling the file.
            if !text.is_empty() {
                return Ok(text);
            }
        }
        if Instant::now() >= deadline {
            return Err(failure(format!("timed out waiting for {}", path.display())));
        }
        thread::sleep(Duration::from_millis(50));
    }
}

/// Called by start after launching the blank tab; wait before loading the app.
pub fn prepare(dir: &Path, session: &str) -> Result<(), Failure> {
    let endpoint = agent_browser(session, &["get", "cdp-url"])?;
    write_private(&dir.join("cdp-url.tmp"), &endpoint)?;
    fs::rename(dir.join("cdp-url.tmp"), dir.join("cdp-url")).map_err(failure)?;
    let result = wait_file(&dir.join("csp-ready"))?;
    if result != "ok" {
        return Err(failure(result));
    }
    Ok(())
}

/// Called by serve. The returned connection must stay alive across navigation:
/// Chromium restores this CDP session's bypass state when the renderer changes.
pub fn listen(dir: &Path, blank: &str) -> Result<Socket, Failure> {
    let result: Result<Socket, Failure> = (|| {
        let endpoint = wait_file(&dir.join("cdp-url"))?;
        let (mut socket, _) = tungstenite::connect(endpoint.as_str()).map_err(failure)?;
        if let MaybeTlsStream::Plain(stream) = socket.get_mut() {
            stream.set_read_timeout(Some(TIMEOUT)).map_err(failure)?;
            stream.set_write_timeout(Some(TIMEOUT)).map_err(failure)?;
        }
        let targets = call(&mut socket, 1, "Target.getTargets", json!({}), None)?;
        let target = targets["targetInfos"]
            .as_array()
            .and_then(|targets| {
                targets
                    .iter()
                    .find(|t| t["type"] == "page" && t["url"] == blank)
            })
            .and_then(|t| t["targetId"].as_str())
            .ok_or_else(|| failure("the review's blank tab was not found"))?;
        let attached = call(
            &mut socket,
            2,
            "Target.attachToTarget",
            json!({"targetId": target, "flatten": true}),
            None,
        )?;
        let session = attached["sessionId"]
            .as_str()
            .ok_or_else(|| failure("CDP returned no sessionId"))?;
        call(
            &mut socket,
            3,
            "Page.setBypassCSP",
            json!({"enabled": true}),
            Some(session),
        )?;
        Ok(socket)
    })();
    let status = match &result {
        Ok(_) => "ok".to_string(),
        Err(e) => e.message.clone(),
    };
    // Publish atomically: start must never mistake a partially written error for success.
    write_private(&dir.join("csp-ready.tmp"), &status)?;
    fs::rename(dir.join("csp-ready.tmp"), dir.join("csp-ready")).map_err(failure)?;
    result
}

fn call(
    socket: &mut Socket,
    id: u64,
    method: &str,
    params: Value,
    session: Option<&str>,
) -> Result<Value, Failure> {
    let mut request = json!({"id": id, "method": method, "params": params});
    if let Some(session) = session {
        request["sessionId"] = session.into();
    }
    socket
        .send(Message::Text(request.to_string().into()))
        .map_err(failure)?;
    loop {
        let message = socket.read().map_err(failure)?;
        let Message::Text(text) = message else {
            continue;
        };
        let response: Value = serde_json::from_str(&text).map_err(failure)?;
        if response["id"] != id {
            continue;
        }
        if let Some(error) = response.get("error") {
            return Err(failure(error));
        }
        return Ok(response["result"].clone());
    }
}
