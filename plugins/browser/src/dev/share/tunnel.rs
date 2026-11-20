//! A private OpenSSH master owns exactly one dynamically allocated reverse forward.
use super::failure;
use std::{
    io,
    path::{Path, PathBuf},
    process::{Child, Command, Output, Stdio},
    thread,
    time::{Duration, Instant},
};
use vega_herdr::Failure;

pub(super) struct Tunnel {
    child: Child,
    socket: PathBuf,
}

impl Tunnel {
    pub fn open(host: &str, dir: &Path, listener: &Path) -> Result<(Self, u16), Failure> {
        let socket = dir.join("ssh");
        let mut cmd = Command::new("ssh");
        cmd.args(["-T", "-M", "-S"])
            .arg(&socket)
            .args([
                "-o",
                "BatchMode=yes",
                "-o",
                "StrictHostKeyChecking=yes",
                "-o",
                "ConnectTimeout=5",
                "-o",
                "ControlPersist=no",
                "-o",
                "ClearAllForwardings=yes",
                "-o",
                "ForwardAgent=no",
                "-o",
                "ForwardX11=no",
                "-o",
                "PermitLocalCommand=no",
                "-o",
                "ServerAliveInterval=10",
                "-o",
                "ServerAliveCountMax=2",
                "--",
                host,
                "cat >/dev/null",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let mut tunnel = Self {
            child: cmd.spawn().map_err(failure)?,
            socket,
        };
        let deadline = Instant::now() + Duration::from_secs(12);
        while !tunnel.socket.exists() {
            if !tunnel.alive().map_err(failure)? || Instant::now() >= deadline {
                return Err(failure("SSH share master did not start"));
            }
            thread::sleep(Duration::from_millis(40));
        }
        let forward = format!("127.0.0.1:0:{}", listener.display());
        let out = bounded(
            tunnel
                .control()
                .args(["-O", "forward", "-R", &forward, "--", host]),
        )?;
        let port = String::from_utf8_lossy(&out.stdout)
            .trim()
            .parse::<u16>()
            .ok()
            .filter(|p| *p > 0 && out.status.success())
            .ok_or_else(|| failure("SSH refused the share reverse forward"))?;
        // GatewayPorts=yes on sshd can override the requested bind. Verify the
        // actual socket before disclosing the capability. Refuse unsupported hosts.
        let probe = format!(
            "if command -v ss >/dev/null; then ss -Hlnt 'sport = :{port}' | awk '{{print $4}}'; \
             else lsof -nP -a -iTCP:{port} -sTCP:LISTEN -Fn | sed -n 's/^n//p'; fi"
        );
        let probe = bounded(tunnel.control().args(["-T", "--", host, &probe]))?;
        if !probe.status.success()
            || String::from_utf8_lossy(&probe.stdout).trim() != format!("127.0.0.1:{port}")
        {
            return Err(failure(
                "Could not verify a loopback-only share listener on the SSH host (requires ss or lsof)",
            ));
        }
        Ok((tunnel, port))
    }
    fn control(&self) -> Command {
        let mut cmd = Command::new("ssh");
        cmd.args(["-F", "/dev/null", "-S"]).arg(&self.socket).args([
            "-o",
            "BatchMode=yes",
            "-o",
            "ControlMaster=no",
            "-o",
            "ProxyCommand=false",
        ]);
        cmd
    }
    pub fn alive(&mut self) -> io::Result<bool> {
        Ok(self.child.try_wait()?.is_none())
    }
}
impl Drop for Tunnel {
    fn drop(&mut self) {
        // EOF also tears down the remote cat if the supervisor dies abruptly.
        drop(self.child.stdin.take());
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn bounded(command: &mut Command) -> Result<Output, Failure> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(failure)?;
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        if child.try_wait().map_err(failure)?.is_some() {
            return child.wait_with_output().map_err(failure);
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(failure("Timed out verifying SSH share forwarding"));
        }
        thread::sleep(Duration::from_millis(25));
    }
}
