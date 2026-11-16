//! CDP reachability of 127.0.0.1 ports without curl: a one-shot HTTP/1.1 GET
//! of `/json/version` with a two-second deadline, the probe the old skill's
//! scripts did with `curl -fsS -m 2` / `curl -s -m 2`. HTTP/1.0 is not an
//! option: Chrome 146 closes a 1.0 request without a response (vega-nic.19),
//! which read as `Held` although CDP answers.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

/// Two seconds, as `curl -m 2` in the old scripts.
const DEADLINE: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Probe {
    /// CDP answers (`curl -fsS` exit 0).
    Cdp,
    /// Nothing listens on the port (curl exit 7).
    Nothing,
    /// Something answers that does not speak CDP, or the deadline hit
    /// (curl exit 22 or 28).
    Held,
}

/// Probe 127.0.0.1:<port> for a DevTools HTTP endpoint.
pub fn probe(port: u16) -> Probe {
    let Ok(mut stream) = TcpStream::connect_timeout(
        &format!("127.0.0.1:{port}")
            .parse()
            .expect("socket address parses"),
        DEADLINE,
    ) else {
        return Probe::Nothing;
    };
    let _ = stream.set_read_timeout(Some(DEADLINE));
    let _ = stream.set_write_timeout(Some(DEADLINE));
    let request = format!(
        "GET /json/version HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n"
    );
    if stream.write_all(request.as_bytes()).is_err() {
        return Probe::Held;
    }
    let mut buf = [0u8; 512];
    match stream.read(&mut buf) {
        Ok(0) | Err(_) => Probe::Held,
        Ok(n) => {
            let head = String::from_utf8_lossy(&buf[..n]);
            match head.split_whitespace().nth(1) {
                Some("200") => Probe::Cdp,
                _ => Probe::Held,
            }
        }
    }
}

/// Whether anything answers on the port (the old `curl -s -m 2` without `-f`).
pub fn connected(port: u16) -> bool {
    probe(port) != Probe::Nothing
}
