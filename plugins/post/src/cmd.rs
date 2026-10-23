//! The command channel (vegad M3b, vega-lhc.18): core NATS request/reply on
//! `vega.cmd.<to>.<kind>.<from>`, no JetStream. `vega post cmd` sends a JSON
//! request (stdin, at most 4 KiB) and waits for the reply. The relay of the
//! target host serves `vega.cmd.<host>.>`: it takes the sender from the
//! subject (the broker enforces it per key), allows only the kinds in
//! HANDLERS, and runs that handler with fixed argv and the JSON on stdin.
//!
//! The handler owns the meaning and its checks (for `open`: vega-open's
//! remote-open maps the sender's paths to mounts and refuses anything
//! outside its roots). The relay only routes, limits and reports.

use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

pub const MAX_PAYLOAD: usize = 4096;
pub const HANDLER_TIMEOUT: Duration = Duration::from_secs(10);

/// Allowed kinds and their handler argv; the relay appends `<from host>`.
pub const HANDLERS: &[(&str, &[&str])] = &[
    ("open", &["vega-open", "remote-open", "--from"]),
    (
        "orchestration",
        &["vega-orchestration", "remote-update", "--from"],
    ),
];

pub fn subject(to: &str, kind: &str, from: &str) -> String {
    format!("vega.cmd.{to}.{kind}.{from}")
}

/// `vega.cmd.<to>.<kind>.<from>` -> (to, kind, from).
pub fn parse_subject(s: &str) -> Option<(&str, &str, &str)> {
    let mut t = s.split('.');
    match (t.next(), t.next(), t.next(), t.next(), t.next(), t.next()) {
        (Some("vega"), Some("cmd"), Some(to), Some(kind), Some(from), None) => {
            Some((to, kind, from))
        }
        _ => None,
    }
}

pub fn valid_kind(k: &str) -> bool {
    !k.is_empty()
        && k.len() <= 32
        && k.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

fn reply(ok: bool, code: &str, message: &str) -> Vec<u8> {
    json!({"ok": ok, "code": code, "message": message})
        .to_string()
        .into_bytes()
}

/// What the serving relay answers to one request.
pub fn serve(kind: &str, from: &str, payload: &[u8]) -> Vec<u8> {
    let Some((_, argv)) = HANDLERS.iter().find(|(k, _)| *k == kind) else {
        return reply(false, "denied", &format!("kind {kind} is not served here"));
    };
    if payload.len() > MAX_PAYLOAD {
        return reply(false, "denied", "payload larger than 4 KiB");
    }
    if serde_json::from_slice::<Value>(payload).is_err() {
        return reply(false, "denied", "payload is not JSON");
    }
    let response_limit = if kind == "orchestration" {
        MAX_PAYLOAD
    } else {
        200
    };
    match run(argv, from, payload, response_limit) {
        Ok((0, line)) => reply(true, "ok", &line),
        Ok((code, line)) => reply(false, "handler_failed", &format!("exit {code}: {line}")),
        Err(why) => reply(false, "handler_failed", &why),
    }
}

/// Runs the handler with the sender appended, the JSON on stdin, within
/// HANDLER_TIMEOUT. Returns the exit code and the first stdout line.
fn run(
    argv: &[&str],
    from: &str,
    payload: &[u8],
    response_limit: usize,
) -> Result<(i32, String), String> {
    let mut child = Command::new(argv[0])
        .args(&argv[1..])
        .arg(from)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("{}: {e}", argv[0]))?;
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(payload);
    }
    let mut stdout = child.stdout.take();
    let reader = std::thread::spawn(move || {
        let mut s = String::new();
        if let Some(o) = stdout.as_mut() {
            let _ = o.read_to_string(&mut s);
        }
        s
    });
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) if start.elapsed() < HANDLER_TIMEOUT => {
                std::thread::sleep(Duration::from_millis(20))
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("timed out after {} s", HANDLER_TIMEOUT.as_secs()));
            }
            Err(e) => return Err(e.to_string()),
        }
    };
    let out = reader.join().unwrap_or_default();
    let line = out
        .lines()
        .next()
        .unwrap_or("")
        .chars()
        .take(response_limit)
        .collect();
    Ok((status.code().unwrap_or(-1), line))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subjects_round_trip() {
        let s = subject("proteus", "open", "deimos");
        assert_eq!(s, "vega.cmd.proteus.open.deimos");
        assert_eq!(parse_subject(&s), Some(("proteus", "open", "deimos")));
        assert_eq!(parse_subject("vega.cmd.a.b"), None);
        assert_eq!(parse_subject("vega.cmd.a.b.c.d"), None);
    }

    #[test]
    fn kinds_are_plain_words() {
        assert!(valid_kind("open"));
        assert!(!valid_kind("open.x"));
        assert!(!valid_kind("*"));
        assert!(!valid_kind(""));
    }

    #[test]
    fn unknown_kind_big_or_bad_payload_is_denied() {
        let v: Value = serde_json::from_slice(&serve("shell", "x", b"{}")).unwrap();
        assert_eq!(v["code"], "denied");
        let big = vec![b' '; MAX_PAYLOAD + 1];
        let v: Value = serde_json::from_slice(&serve("open", "x", &big)).unwrap();
        assert_eq!(v["code"], "denied");
        let v: Value = serde_json::from_slice(&serve("open", "x", b"not json")).unwrap();
        assert_eq!(v["code"], "denied");
    }

    #[test]
    fn structured_wait_receipts_are_not_cut_at_the_human_handler_limit() {
        let receipt =
            json!({"id":format!("{}:{}","a".repeat(220),"b".repeat(32)),"status":"waiting"});
        let script = format!("cat >/dev/null\nprintf '%s\\n' '{}'", receipt);
        let (code, line) = run(
            &["/bin/sh", "-c", &script, "handler"],
            "worker",
            b"{}",
            MAX_PAYLOAD,
        )
        .unwrap();
        assert_eq!(code, 0);
        assert_eq!(serde_json::from_str::<Value>(&line).unwrap(), receipt);
        assert!(HANDLERS.iter().any(|(kind, _)| *kind == "orchestration"));
    }
}
