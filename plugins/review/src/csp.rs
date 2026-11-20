//! Keep CSP bypass attached to the review tab for the lifetime of its inbox.
use std::net::TcpStream;
use std::path::Path;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tungstenite::{stream::MaybeTlsStream, Message, WebSocket};

use crate::{fs, thread, write_private, Failure};

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
pub fn prepare(dir: &Path, endpoint: &str) -> Result<(), Failure> {
    write_private(&dir.join("cdp-url.tmp"), endpoint)?;
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
        let mut socket = connect(&endpoint).map_err(failure)?;
        let target = find_target(&mut socket, blank)?
            .ok_or_else(|| failure("the review's blank tab was not found"))?;
        write_private(&dir.join("target-id.tmp"), &target)?;
        fs::rename(dir.join("target-id.tmp"), dir.join("target-id")).map_err(failure)?;
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

fn connect(endpoint: &str) -> Result<Socket, tungstenite::Error> {
    let (mut socket, _) = tungstenite::connect(endpoint)?;
    if let MaybeTlsStream::Plain(stream) = socket.get_mut() {
        stream.set_read_timeout(Some(TIMEOUT))?;
        stream.set_write_timeout(Some(TIMEOUT))?;
    }
    Ok(socket)
}

fn targets(socket: &mut Socket, id: u64) -> Result<Vec<Value>, Failure> {
    let mut result = call(socket, id, "Target.getTargets", json!({}), None)?;
    match result["targetInfos"].take() {
        Value::Array(targets) => Ok(targets),
        _ => Err(failure("CDP returned no target list")),
    }
}

fn find_target(socket: &mut Socket, blank: &str) -> Result<Option<String>, Failure> {
    Ok(targets(socket, 1)?
        .iter()
        .find(|t| t["type"] == "page" && t["url"] == blank)
        .and_then(|t| t["targetId"].as_str())
        .map(str::to_owned))
}

/// Close the recorded target, even after navigation or a change of active tab.
/// Before CSP is ready, the unique blank URL also identifies the owned tab.
pub fn close_tab(dir: &Path, blank: &str) -> Result<(), Failure> {
    let read = |name| match fs::read_to_string(dir.join(name)) {
        Ok(value) => Ok(Some(value)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(failure(e)),
    };
    let Some(endpoint) = read("cdp-url")? else {
        return Ok(());
    };
    let mut socket = match connect(&endpoint) {
        Ok(socket) => socket,
        Err(tungstenite::Error::Io(e)) if e.kind() == std::io::ErrorKind::ConnectionRefused => {
            return Ok(())
        }
        // The browser websocket includes an instance ID. A replacement
        // browser on this port won't accept the old instance's path.
        Err(tungstenite::Error::Http(response)) if response.status().as_u16() == 404 => {
            return Ok(())
        }
        Err(e) => return Err(failure(e)),
    };
    let target = match read("target-id")? {
        Some(target) => Some(target),
        None => find_target(&mut socket, blank)?,
    };
    if let Some(target) = target {
        if targets(&mut socket, 2)?
            .iter()
            .any(|t| t["targetId"] == target)
        {
            let result = call(
                &mut socket,
                3,
                "Target.closeTarget",
                json!({"targetId": target}),
                None,
            );
            // A user can close the tab between listing and closing it.
            if !matches!(&result, Ok(value) if value["success"] == true)
                && targets(&mut socket, 4)?
                    .iter()
                    .any(|t| t["targetId"] == target)
            {
                result?;
                return Err(failure("CDP did not close the review tab"));
            }
        }
    }
    Ok(())
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
