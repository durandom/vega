//! Bounded local socket requests. Each JSON line has an absolute deadline,
//! including a peer that keeps sending bytes without completing its reply.
use std::io::{self, BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::{Duration, Instant};

use serde_json::Value;
use socket2::{Domain, SockAddr, Socket, Type};

pub const CALL_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_LINE: usize = 16 * 1024 * 1024;

pub struct Client(BufReader<UnixStream>);

impl Client {
    pub fn connect(path: &Path, timeout: Duration) -> io::Result<Self> {
        let socket = Socket::new(Domain::UNIX, Type::STREAM, None)?;
        socket.connect_timeout(&SockAddr::unix(path)?, timeout)?;
        Ok(Self(BufReader::new(socket.into())))
    }

    pub fn write(&mut self, value: &Value, timeout: Duration) -> io::Result<()> {
        let deadline = Instant::now() + timeout;
        let bytes = format!("{value}\n").into_bytes();
        let mut left = bytes.as_slice();
        while !left.is_empty() {
            self.0
                .get_ref()
                .set_write_timeout(Some(remaining(deadline)?))?;
            let n = self.0.get_mut().write(left)?;
            if n == 0 {
                return Err(io::ErrorKind::WriteZero.into());
            }
            left = &left[n..];
        }
        Ok(())
    }

    pub fn read(&mut self, timeout: Duration) -> io::Result<Value> {
        let deadline = Instant::now() + timeout;
        let mut line = Vec::new();
        loop {
            self.0
                .get_ref()
                .set_read_timeout(Some(remaining(deadline)?))?;
            let buf = self.0.fill_buf()?;
            if buf.is_empty() {
                return Err(io::ErrorKind::UnexpectedEof.into());
            }
            let end = buf.iter().position(|b| *b == b'\n');
            let n = end.map_or(buf.len(), |i| i + 1);
            if line.len() + n > MAX_LINE {
                return Err(io::Error::other("herdr reply exceeds 16 MiB"));
            }
            line.extend_from_slice(&buf[..n]);
            self.0.consume(n);
            if end.is_some() {
                if line.iter().all(u8::is_ascii_whitespace) {
                    line.clear();
                    continue;
                }
                return serde_json::from_slice(&line).map_err(io::Error::other);
            }
        }
    }
}

fn remaining(deadline: Instant) -> io::Result<Duration> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|d| !d.is_zero())
        .ok_or_else(|| io::ErrorKind::TimedOut.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixListener;

    #[test]
    fn a_peer_that_does_not_read_cannot_block_a_write_forever() {
        let (stream, _peer) = UnixStream::pair().unwrap();
        let mut client = Client(BufReader::new(stream));
        let start = Instant::now();
        assert!(client
            .write(
                &Value::String("x".repeat(8 * 1024 * 1024)),
                Duration::from_millis(100)
            )
            .is_err());
        assert!(start.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn stalled_and_trickling_replies_have_a_deadline_then_next_call_works() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("herdr.sock");
        let listener = UnixListener::bind(&path).unwrap();
        let peer = std::thread::spawn(move || {
            for trickle in [false, true] {
                let (mut stream, _) = listener.accept().unwrap();
                for _ in 0..30 {
                    if trickle && stream.write_all(b" ").is_err() {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
            }
            let (mut stream, _) = listener.accept().unwrap();
            stream.write_all(b"{\"result\":true}\n").unwrap();
        });
        for _ in 0..2 {
            let mut client = Client::connect(&path, CALL_TIMEOUT).unwrap();
            let start = Instant::now();
            assert!(client.read(Duration::from_millis(100)).is_err());
            assert!(start.elapsed() < Duration::from_secs(1));
            // Let the server finish the first connection before the next.
            std::thread::sleep(Duration::from_millis(350));
        }
        let mut client = Client::connect(&path, CALL_TIMEOUT).unwrap();
        assert_eq!(client.read(CALL_TIMEOUT).unwrap()["result"], true);
        peer.join().unwrap();
    }
}
