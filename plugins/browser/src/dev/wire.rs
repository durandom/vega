use std::io::ErrorKind;
use std::net::{SocketAddr, TcpStream};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tungstenite::{client, Message, WebSocket};
use vega_herdr::Failure;

use super::{failure, stale, Browser, Project};

const TIMEOUT: Duration = Duration::from_secs(3);

fn read_timeout(operation: &str) -> Failure {
    Failure::new(
        "dev_cdp_read_timeout",
        format!("Timed out waiting for Chrome CDP: {operation}"),
        "Check that the managed Chrome for Testing window is responsive, then retry",
    )
}

pub(super) struct Cdp {
    pub socket: WebSocket<TcpStream>,
    id: u64,
}

impl Cdp {
    pub fn connect(browser: &Browser) -> Result<Self, Failure> {
        Self::endpoint(browser.port, &browser.path)
    }

    pub fn endpoint(port: u16, path: &str) -> Result<Self, Failure> {
        let addr = SocketAddr::from(([127, 0, 0, 1], port));
        let stream = TcpStream::connect_timeout(&addr, TIMEOUT).map_err(failure)?;
        stream.set_read_timeout(Some(TIMEOUT)).map_err(failure)?;
        stream.set_write_timeout(Some(TIMEOUT)).map_err(failure)?;
        let (socket, _) = client(format!("ws://{addr}{path}"), stream).map_err(failure)?;
        Ok(Self { socket, id: 0 })
    }

    pub fn call(&mut self, method: &str, params: Value) -> Result<Value, Failure> {
        self.call_in(None, method, params)
    }

    pub fn call_in(
        &mut self,
        session: Option<&str>,
        method: &str,
        params: Value,
    ) -> Result<Value, Failure> {
        self.call_in_until(session, method, params, Instant::now() + TIMEOUT)
    }

    pub fn call_in_until(
        &mut self,
        session: Option<&str>,
        method: &str,
        params: Value,
        deadline: Instant,
    ) -> Result<Value, Failure> {
        let id = self.send(session, method, params)?;
        let result = loop {
            match self.receive_until(deadline, method) {
                Ok(reply) if reply["id"] == id => break Self::result(method, reply),
                Ok(_) => {}
                Err(error) => break Err(error),
            }
        };
        let reset = self.socket.get_ref().set_read_timeout(Some(TIMEOUT));
        result.and_then(|reply| reset.map(|()| reply).map_err(failure))
    }

    pub fn send(
        &mut self,
        session: Option<&str>,
        method: &str,
        params: Value,
    ) -> Result<u64, Failure> {
        self.id += 1;
        let mut request = json!({"id": self.id, "method": method, "params": params});
        if let Some(session) = session {
            request["sessionId"] = json!(session);
        }
        self.socket
            .send(Message::text(request.to_string()))
            .map_err(failure)?;
        Ok(self.id)
    }

    pub fn receive(&mut self) -> Result<Value, Failure> {
        self.receive_until(Instant::now() + TIMEOUT, "reading a message")
    }

    fn receive_until(&mut self, deadline: Instant, operation: &str) -> Result<Value, Failure> {
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(read_timeout(operation));
            }
            self.socket
                .get_ref()
                .set_read_timeout(Some(remaining))
                .map_err(failure)?;
            match self.socket.read() {
                Ok(Message::Text(raw)) => return serde_json::from_str(&raw).map_err(failure),
                Ok(_) => {}
                Err(tungstenite::Error::Io(error))
                    if matches!(error.kind(), ErrorKind::TimedOut | ErrorKind::WouldBlock) =>
                {
                    return Err(read_timeout(operation));
                }
                Err(error) => return Err(failure(error)),
            }
        }
    }

    pub fn result(method: &str, reply: Value) -> Result<Value, Failure> {
        if !reply["error"].is_null() {
            return Err(failure(format!("{method}: {}", reply["error"]["message"])));
        }
        Ok(reply["result"].clone())
    }

    pub fn info(&mut self, project: &Project, target: &str) -> Result<Value, Failure> {
        let targets = self.call("Target.getTargets", json!({"filter": [{"type": "page"}]}))?;
        let info = targets["targetInfos"]
            .as_array()
            .ok_or_else(|| failure("Chrome returned no target list"))?
            .iter()
            .find(|info| info["targetId"] == target)
            .ok_or_else(|| stale("The owned dev tab is gone"))?;
        if info["type"] != "page" || info["browserContextId"] != project.context {
            return Err(stale("The dev tab is outside its recorded context"));
        }
        if self.window(target)? != project.window {
            return Err(stale("The dev tab is outside its recorded window"));
        }
        Ok(info.clone())
    }

    pub fn window(&mut self, target: &str) -> Result<i64, Failure> {
        self.call("Browser.getWindowForTarget", json!({"targetId": target}))?["windowId"]
            .as_i64()
            .ok_or_else(|| failure("Chrome returned no window ID"))
    }

    pub fn string(&mut self, method: &str, params: Value, field: &str) -> Result<String, Failure> {
        self.call(method, params)?[field]
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| failure(format!("Chrome returned no {field}")))
    }
}
