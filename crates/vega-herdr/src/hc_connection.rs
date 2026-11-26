//! Reuse hc's supported stdio RPC transport for the Windows relay's polling.
use super::*;
use std::{
    io::{self, BufRead, BufReader, Write},
    process::{Child, Stdio},
    sync::{mpsc, Arc, Mutex},
    thread,
};

#[derive(Default)]
pub struct Connection(Option<Session>);

impl Connection {
    /// Exactly one request is in flight. A failed exchange drops the old child
    /// before the next request can start a fresh stream.
    pub fn call(&mut self, method: &str, params: Value) -> Result<Value, Failure> {
        crate::hc_schema::params(method, &params).map_err(|e| {
            Failure::new("usage", e, "use the parameters in the pinned hc RPC schema")
        })?;
        if self.0.is_none() {
            self.0 = Some(Session::start().map_err(transport_error)?);
        }
        let session = self.0.as_ref().unwrap();
        let output = session.exchange(request(method, params));
        let reset = !matches!(&output, Ok(out) if out.line.is_ok());
        let result = output.and_then(|out| decode(method, out));
        if reset || result.as_ref().is_err_and(|e| e.code == "hc_bad_output") {
            self.0 = None;
        }
        result
    }
}

struct Session {
    child: Option<Child>,
    _tree: crate::platform::ProcessTree,
    requests: mpsc::Sender<Vec<u8>>,
    replies: mpsc::Receiver<io::Result<Vec<u8>>>,
    stderr: Arc<Mutex<Vec<u8>>>,
}

impl Session {
    fn start() -> io::Result<Self> {
        let mut cmd = Command::new("hc");
        crate::platform::isolate(&mut cmd);
        let mut child = cmd
            .arg("connect")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let tree = crate::platform::ProcessTree::attach(&mut child)?;
        let mut stdin = child.stdin.take().expect("piped stdin");
        let mut stdout = BufReader::new(child.stdout.take().expect("piped stdout"));
        let error_pipe = child.stderr.take().expect("piped stderr");
        let stderr = Arc::new(Mutex::new(Vec::new()));
        let tail = Arc::clone(&stderr);
        thread::spawn(move || crate::drain_rpc_stderr(error_pipe, &tail));
        let (requests, input) = mpsc::channel::<Vec<u8>>();
        let (output, replies) = mpsc::channel();
        thread::spawn(move || {
            while let Ok(request) = input.recv() {
                let result = stdin.write_all(&request).and_then(|()| {
                    let mut line = Vec::new();
                    if stdout.read_until(b'\n', &mut line)? == 0 {
                        return Err(io::Error::from(io::ErrorKind::UnexpectedEof));
                    }
                    Ok(line)
                });
                let closed = result.is_err();
                if output.send(result).is_err() || closed {
                    break;
                }
            }
        });
        Ok(Self {
            child: Some(child),
            _tree: tree,
            requests,
            replies,
            stderr,
        })
    }

    fn exchange(&self, request: Vec<u8>) -> Result<crate::RpcLine, Failure> {
        let line = if self.requests.send(request).is_err() {
            Err(io::Error::from(io::ErrorKind::BrokenPipe))
        } else {
            match self.replies.recv_timeout(DAEMON_CALL_TIMEOUT) {
                Ok(line) => line,
                Err(mpsc::RecvTimeoutError::Timeout) => return Err(timeout_error()),
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    Err(io::Error::from(io::ErrorKind::BrokenPipe))
                }
            }
        };
        let stderr = self.stderr.lock().expect("stderr tail lock").clone();
        Ok(crate::RpcLine { line, stderr })
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        // The job closes immediately after Drop and ends descendants that
        // might still hold the I/O threads' pipes.
        if let Some(child) = self.child.take() {
            crate::kill_and_reap(child);
        }
    }
}
