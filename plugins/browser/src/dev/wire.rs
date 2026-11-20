use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

use serde_json::{json, Value};
use tungstenite::{client, Message, WebSocket};
use vega_herdr::Failure;

use super::{failure, stale, Browser, Project};

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
        let timeout = Duration::from_secs(3);
        let stream = TcpStream::connect_timeout(&addr, timeout).map_err(failure)?;
        stream.set_read_timeout(Some(timeout)).map_err(failure)?;
        stream.set_write_timeout(Some(timeout)).map_err(failure)?;
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
        let id = self.send(session, method, params)?;
        loop {
            let reply = self.receive()?;
            if reply["id"] == id {
                return Self::result(method, reply);
            }
        }
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
        loop {
            if let Message::Text(raw) = self.socket.read().map_err(failure)? {
                return serde_json::from_str(&raw).map_err(failure);
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
