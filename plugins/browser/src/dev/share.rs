//! A share capability reaches a bounded command broker, never a Chrome socket.
mod page;
mod policy;
mod tunnel;
use super::{failure, load, passed, process, profile, usage, Browser, Project};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    env,
    fs::{self, File, OpenOptions},
    io::{self, BufRead, BufReader, Read, Write},
    net::{Shutdown, TcpStream},
    os::unix::{
        fs::{DirBuilderExt, MetadataExt, PermissionsExt},
        net::{UnixListener, UnixStream},
    },
    path::{Path, PathBuf},
    process::{Command, ExitCode, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant, SystemTime},
};
use subtle::ConstantTimeEq;
use tungstenite::{
    accept_hdr_with_config, client,
    handshake::server::{Request, Response},
    protocol::WebSocketConfig,
    Message,
};
use vega_herdr::Failure;

pub const WORKER: &str = "--internal-dev-share";
const MAX_RELAYS: usize = 16;
#[derive(Serialize, Deserialize)]
struct Config {
    generation: String,
    id: String,
    host: String,
    origins: policy::Origins,
    ttl: u64,
}

fn valid_host(host: &str) -> bool {
    !host.is_empty()
        && !host.starts_with('-')
        && host
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"@._-".contains(&b))
}

fn directory(profile: &Path, id: &str) -> PathBuf {
    let mut hash = Sha256::new();
    hash.update(profile.as_os_str().as_encoded_bytes());
    hash.update([0]);
    hash.update(id.as_bytes());
    Path::new("/tmp").join(format!(
        "vega-share-{}-{}",
        unsafe { libc::geteuid() },
        &format!("{:x}", hash.finalize())[..16]
    ))
}
fn local(dir: &Path, op: &str) -> io::Result<String> {
    let mut stream = UnixStream::connect(dir.join("control"))?;
    stream.set_read_timeout(Some(Duration::from_secs(90)))?;
    writeln!(stream, "{op}")?;
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line)?;
    if line.is_empty() {
        return Err(io::Error::other("Share supervisor disconnected"));
    }
    Ok(line.trim().into())
}
fn inactive(e: &io::Error) -> bool {
    matches!(
        e.kind(),
        io::ErrorKind::ConnectionRefused | io::ErrorKind::NotFound
    )
}
pub fn command(args: &[String]) -> Result<ExitCode, Failure> {
    let profile = profile()?;
    let profile = profile.canonicalize().unwrap_or(profile);
    let data = load(&profile)?;
    let args = args.iter().map(String::as_str).collect::<Vec<_>>();
    let rows = || data.projects.iter().map(|(id, p)| (id.as_str(), p));
    if args == ["status"] || args == ["--status"] {
        let mut shares = Vec::new();
        for (id, _) in rows() {
            match local(&directory(&profile, id), "status") {
                Ok(row) => shares.push(serde_json::from_str::<Value>(&row).map_err(failure)?),
                Err(e) if inactive(&e) => {}
                Err(e) => return Err(failure(e)),
            }
        }
        println!("{}", json!({"shares":shares}));
        return passed();
    }
    let stop = args.first() == Some(&"--stop");
    let name = args
        .get(usize::from(stop))
        .ok_or_else(|| usage("Provide a live dev project ID or label"))?;
    let mut matches = rows().filter(|(id, p)| id == name || p.label == *name);
    let Some((id, _)) = matches.next() else {
        if stop && args.len() == 2 {
            println!("stopped: true");
            return passed();
        }
        return Err(usage("Start a local dev session for this project first"));
    };
    if matches.next().is_some() {
        return Err(usage(
            "Use the canonical project ID for this ambiguous label",
        ));
    }
    let dir = directory(&profile, id);
    if stop {
        if args.len() != 2 {
            return Err(usage("Use dev share --stop <project>"));
        }
        match local(&dir, "stop") {
            Ok(s) if s == "stopped" => {}
            Err(e) if inactive(&e) => {}
            _ => return Err(failure("Share revocation did not complete")),
        }
        println!("stopped: true");
        return passed();
    }
    let browser = data.browser.ok_or_else(|| failure("No live dev browser"))?;
    let mut origins = policy::Origins::new();
    let mut host = None;
    let mut ttl = None;
    for pair in args[1..].chunks(2) {
        match pair {
            ["--origin", raw] => {
                origins.insert(policy::origin(raw, browser.port)?);
            }
            ["--to", value] if host.is_none() && valid_host(value) => {
                host = Some(value.to_string())
            }
            ["--ttl", value] if ttl.is_none() => {
                let (number, scale) = if let Some(n) = value.strip_suffix('h') {
                    (n, 3600)
                } else if let Some(n) = value.strip_suffix('m') {
                    (n, 60)
                } else {
                    (value.strip_suffix('s').unwrap_or(value), 1)
                };
                ttl = Some(
                    number
                        .parse::<u64>()
                        .ok()
                        .and_then(|n| n.checked_mul(scale))
                        .filter(|n| (1..=86400).contains(n))
                        .ok_or_else(|| usage("Share TTL must be 1s through 24h"))?,
                );
            }
            _ => {
                return Err(usage(
                    "Use dev share <project> --to <ssh-host> --origin <origin> [--ttl 8h]",
                ))
            }
        }
    }
    if origins.is_empty() {
        return Err(usage("Provide at least one explicit --origin"));
    }
    if !process::running(&browser, &profile)? {
        return Err(failure("Dev browser is no longer live"));
    }
    let cfg = Config {
        generation: browser.path,
        id: id.into(),
        host: host.ok_or_else(|| usage("--to requires an SSH destination"))?,
        origins,
        ttl: ttl.unwrap_or(28800),
    };
    let mut child = Command::new("nohup")
        .arg(env::current_exe().map_err(failure)?)
        .arg(WORKER)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(failure)?;
    serde_json::to_writer(child.stdin.take().unwrap(), &cfg).map_err(failure)?;
    let mut line = String::new();
    BufReader::new(child.stdout.take().unwrap())
        .read_line(&mut line)
        .map_err(failure)?;
    if !line.starts_with("VEGA_BROWSER_SHARE=ws://127.0.0.1:") {
        return Err(failure(line.trim().to_string()));
    }
    print!("{line}");
    passed()
}

#[derive(Clone, Copy)]
struct Expiry {
    monotonic: Instant,
    wall: SystemTime,
}

impl Expiry {
    fn after(ttl: Duration) -> Self {
        Self {
            monotonic: Instant::now() + ttl,
            wall: SystemTime::now() + ttl,
        }
    }

    fn remaining(self) -> Duration {
        self.wall
            .duration_since(SystemTime::now())
            .unwrap_or_default()
            .min(self.monotonic.saturating_duration_since(Instant::now()))
    }
}

struct Relay {
    socket: UnixStream,
    authenticated: Arc<AtomicBool>,
    deadline: Instant,
    task: thread::JoinHandle<()>,
}

impl Drop for Relay {
    fn drop(&mut self) {
        let _ = self.socket.shutdown(Shutdown::Both);
    }
}

pub fn worker() -> Result<(), Failure> {
    let cfg: Config = serde_json::from_reader(io::stdin().lock()).map_err(failure)?;
    let profile = profile()?.canonicalize().map_err(failure)?;
    let registry_lock = super::lock(&profile)?;
    let registry = load(&profile)?;
    let browser = registry
        .browser
        .ok_or_else(|| failure("Missing managed browser"))?;
    let mut adapter_ports = super::shared::adapter_ports()?;
    adapter_ports.extend(
        registry
            .sessions
            .values()
            .filter(|session| session.generation == browser.path)
            .flat_map(|session| session.tabs.values().map(|tab| tab.port)),
    );
    let (id, project) = registry
        .projects
        .into_iter()
        .find(|(key, _)| key.as_str() == cfg.id)
        .ok_or_else(|| failure("Missing project context"))?;
    if cfg.generation != browser.path
        || !valid_host(&cfg.host)
        || !(1..=86400).contains(&cfg.ttl)
        || cfg.origins.is_empty()
        || cfg.origins.iter().any(|o| {
            policy::origin(o, browser.port).as_ref().ok() != Some(o)
                || url::Url::parse(o)
                    .ok()
                    .and_then(|url| url.port_or_known_default())
                    .is_some_and(|port| adapter_ports.contains(&port))
        })
    {
        return Err(failure("Invalid or stale share configuration"));
    }
    let dir = directory(&profile, &cfg.id);
    if let Err(e) = fs::DirBuilder::new().mode(0o700).create(&dir) {
        if e.kind() != io::ErrorKind::AlreadyExists {
            return Err(failure(e));
        }
    }
    let meta = fs::symlink_metadata(&dir).map_err(failure)?;
    if !meta.is_dir()
        || meta.uid() != unsafe { libc::geteuid() }
        || meta.permissions().mode() & 0o077 != 0
    {
        return Err(failure("Unsafe share control directory"));
    }
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(dir.join("lock"))
        .map_err(failure)?;
    lock.try_lock()
        .map_err(|_| failure("Project already has a share supervisor"))?;
    let path = dir.join("control");
    if path.exists() {
        fs::remove_file(&path).map_err(failure)?;
    }
    let ctl = UnixListener::bind(&path).map_err(failure)?;
    ctl.set_nonblocking(true).map_err(failure)?;
    let private = tempfile::tempdir_in(&dir).map_err(failure)?;
    let endpoint = private.path().join("broker");
    let listener = UnixListener::bind(&endpoint).map_err(failure)?;
    listener.set_nonblocking(true).map_err(failure)?;
    let mut secret = [0u8; 32];
    File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut secret))
        .map_err(failure)?;
    let capability = format!(
        "/share/{}",
        secret
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    );
    if !process::running(&browser, &profile)? {
        return Err(failure("Stale share browser"));
    }
    let live = Arc::new(AtomicBool::new(true));
    let pages = Arc::new(Mutex::new(page::Pages::new(
        browser.clone(),
        project.clone(),
        cfg.origins.clone(),
        Arc::clone(&live),
    )?));
    drop(registry_lock);
    let result = (|| {
        let (mut ssh, port) = tunnel::Tunnel::open(&cfg.host, private.path(), &endpoint)?;
        writeln!(
            io::stdout().lock(),
            "VEGA_BROWSER_SHARE=ws://127.0.0.1:{port}{capability} agent-browser snapshot -i"
        )
        .map_err(failure)?;
        io::stdout().flush().map_err(failure)?;
        let expiry = Expiry::after(Duration::from_secs(cfg.ttl));
        let mut checked = Instant::now();
        let mut active: Vec<Relay> = Vec::new();
        let mut stop = None;
        loop {
            if expiry.remaining().is_zero()
                || pages.is_poisoned()
                || !live.load(Ordering::Acquire)
                || !ssh.alive().map_err(failure)?
            {
                break;
            }
            if checked.elapsed() > Duration::from_secs(1) {
                if !process::running(&browser, &profile).unwrap_or(false) {
                    break;
                }
                checked = Instant::now();
            }
            if let Ok((mut stream, _)) = ctl.accept() {
                stream.set_nonblocking(false).map_err(failure)?;
                stream
                    .set_read_timeout(Some(Duration::from_millis(100)))
                    .map_err(failure)?;
                let mut line = String::new();
                let _ = BufReader::new((&stream).take(16)).read_line(&mut line);
                if line == "stop\n" {
                    stop = Some(stream);
                    break;
                }
                if line == "status\n" {
                    let _ = writeln!(
                        stream,
                        "{}",
                        json!({"project":cfg.id,"host":cfg.host,"remaining_seconds":expiry.remaining().as_secs()})
                    );
                }
            }
            active.retain(|relay| {
                !relay.task.is_finished()
                    && (relay.authenticated.load(Ordering::Acquire)
                        || Instant::now() < relay.deadline)
            });
            if let Ok((stream, _)) = listener.accept() {
                // Keep the handshake pool bounded. A full pool of incomplete
                // handshakes still admits a fresh client by retiring the oldest.
                if active.len() == MAX_RELAYS {
                    if let Some(index) = active
                        .iter()
                        .position(|r| !r.authenticated.load(Ordering::Acquire))
                    {
                        active.remove(index);
                    }
                }
                if active.len() < MAX_RELAYS {
                    let socket = stream.try_clone().map_err(failure)?;
                    let authenticated = Arc::new(AtomicBool::new(false));
                    let deadline = Instant::now() + Duration::from_secs(2);
                    let cap = capability.clone();
                    let label = project.label.clone();
                    let pages = Arc::clone(&pages);
                    let accepted = Arc::clone(&authenticated);
                    let task = thread::spawn(move || {
                        let _ = relay(stream, &cap, &label, pages, accepted, deadline, expiry);
                    });
                    active.push(Relay {
                        socket,
                        authenticated,
                        deadline,
                        task,
                    });
                }
            }
            thread::sleep(Duration::from_millis(15));
        }
        live.store(false, Ordering::Release);
        drop(ssh);
        drop(listener);
        drop(active);
        Ok(stop)
    })();
    live.store(false, Ordering::Release);
    pages
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .close();
    // A local session may have closed its last tab while the share was active.
    // Reconcile only this generation and context while allocation is serialized.
    if process::running(&browser, &profile).unwrap_or(false) {
        let _guard = super::lock(&profile)?;
        let mut data = load(&profile)?;
        if data
            .browser
            .as_ref()
            .is_some_and(|b| b.path == browser.path)
            && data
                .projects
                .get(&id)
                .is_some_and(|p| p.context == project.context)
            && !data
                .sessions
                .values()
                .any(|s| s.project == id && s.generation == browser.path)
            && super::dispose_unused(&mut super::Cdp::connect(&browser)?, &project)?
        {
            data.projects.remove(&id);
            super::save(&profile, &data)?;
        }
    }
    let _ = fs::remove_file(path);
    if let Some(mut reply) = result? {
        let _ = writeln!(reply, "stopped");
    }
    Ok(())
}

#[allow(clippy::result_large_err)]
fn relay(
    stream: UnixStream,
    cap: &str,
    label: &str,
    pages: Arc<Mutex<page::Pages>>,
    authenticated: Arc<AtomicBool>,
    deadline: Instant,
    expiry: Expiry,
) -> Result<(), Failure> {
    stream.set_nonblocking(false).map_err(failure)?;
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .map_err(failure)?;
    stream
        .set_write_timeout(Some(Duration::from_secs(2)))
        .map_err(failure)?;
    let config = WebSocketConfig::default()
        .max_message_size(Some(32768))
        .max_frame_size(Some(32768));
    let mut ws = accept_hdr_with_config(
        stream,
        |req: &Request, res: Response| {
            if Instant::now() < deadline
                && !expiry.remaining().is_zero()
                && req
                    .uri()
                    .path_and_query()
                    .is_some_and(|s| bool::from(s.as_str().as_bytes().ct_eq(cap.as_bytes())))
                && !req.headers().contains_key("origin")
            {
                Ok(res)
            } else {
                Err(tungstenite::http::Response::builder()
                    .status(404)
                    .body(None)
                    .unwrap())
            }
        },
        Some(config),
    )
    .map_err(failure)?;
    authenticated.store(true, Ordering::Release);
    let msg = ws.read().map_err(failure)?;
    let req: Value = serde_json::from_str(msg.to_text().map_err(failure)?).map_err(failure)?;
    let reply = dispatch(&req, label, |args| {
        let mut actor = pages.lock().ok()?;
        if expiry.remaining().is_zero() {
            return None;
        }
        actor.command(&args).ok()
    });
    ws.send(Message::text(reply.to_string())).map_err(failure)
}

fn dispatch(req: &Value, label: &str, run: impl FnOnce(Vec<String>) -> Option<Value>) -> Value {
    let result =
        if req["method"] == "Vega.command" && req["id"] == 1 && req.get("sessionId").is_none() {
            serde_json::from_value::<Vec<String>>(req["params"]["args"].clone())
                .ok()
                .filter(|a| a.len() <= 5)
                .and_then(run)
        } else {
            None
        };
    match result {
        Some(v) => json!({"id":1,"result":v}),
        None => {
            json!({"id":1,"error":{"code":-32601,"message":format!("Command denied or failed in browser share {label}")}})
        }
    }
}

pub fn remote(endpoint: &str, args: &[String]) -> Result<ExitCode, Failure> {
    let url = url::Url::parse(endpoint).map_err(|_| failure("Invalid browser share capability"))?;
    if url.scheme() != "ws"
        || url.host_str() != Some("127.0.0.1")
        || url.port().is_none()
        || !url
            .path()
            .strip_prefix("/share/")
            .is_some_and(|v| v.len() == 64 && v.bytes().all(|c| c.is_ascii_hexdigit()))
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(failure("Invalid browser share capability"));
    }
    let mut argv = Vec::new();
    let mut iter = args.iter();
    let mut as_json = false;
    while let Some(arg) = iter.next() {
        if argv.is_empty() && arg == "--session" {
            iter.next();
        } else if argv.is_empty() && arg == "--json" {
            as_json = true;
        } else {
            argv.push(arg.clone());
        }
    }
    let dest = if argv.first().is_some_and(|s| s == "screenshot") && argv.len() == 2 {
        argv.pop()
    } else {
        None
    };
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], url.port().unwrap()));
    let socket = TcpStream::connect_timeout(&addr, Duration::from_secs(3))
        .map_err(|_| failure("Share listener is unavailable"))?;
    socket
        .set_read_timeout(Some(Duration::from_secs(20)))
        .map_err(failure)?;
    let (mut ws, _) = client(endpoint, socket).map_err(|_| failure("Share capability rejected"))?;
    ws.send(Message::text(
        json!({"id":1,"method":"Vega.command","params":{"args":argv}}).to_string(),
    ))
    .map_err(failure)?;
    let msg = ws
        .read()
        .map_err(|_| failure("Browser share disconnected"))?;
    let reply: Value = serde_json::from_str(msg.to_text().map_err(failure)?).map_err(failure)?;
    if reply.get("error").is_some() {
        return Err(failure(
            reply["error"]["message"]
                .as_str()
                .unwrap_or("Browser share failed"),
        ));
    }
    let mut data = reply["result"].clone();
    if let Some(path) = dest {
        let bytes = STANDARD
            .decode(
                data["png"]
                    .as_str()
                    .ok_or_else(|| failure("Share screenshot failed"))?,
            )
            .map_err(failure)?;
        fs::write(&path, bytes).map_err(failure)?;
        data = json!(path);
    }
    if as_json {
        println!("{}", json!({"success":true,"data":data}));
    } else if let Some(s) = data.as_str() {
        println!("{s}");
    } else {
        println!("{data}");
    }
    passed()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn either_elapsed_clock_expires_the_share() {
        let ttl = Duration::from_secs(60);
        let mut expiry = Expiry::after(ttl);
        expiry.wall = SystemTime::UNIX_EPOCH;
        assert!(expiry.remaining().is_zero());
        expiry.wall = SystemTime::now() + ttl;
        expiry.monotonic = Instant::now();
        assert!(expiry.remaining().is_zero());
    }

    #[test]
    fn remote_browser_methods_cannot_reach_the_command_actor() {
        for method in [
            "Browser.close",
            "Target.getTargets",
            "Target.attachToTarget",
            "Target.createBrowserContext",
            "Target.createTarget",
            "Fetch.disable",
            "Page.navigate",
            "Storage.getCookies",
            "Runtime.evaluate",
            "Browser.setDownloadBehavior",
            "DOM.setFileInputFiles",
        ] {
            let req =
                json!({"id":1,"method":method,"params":{"targetId":"other","args":["snapshot"]}});
            let reply = dispatch(&req, "alpha", |_| panic!("forwarded {method}"));
            assert_eq!(reply["error"]["code"], -32601);
        }
        let forged = json!({"id":1,"method":"Vega.command","sessionId":"other","params":{"args":["snapshot"]}});
        assert!(
            dispatch(&forged, "alpha", |_| panic!("forwarded foreign session"))
                .get("error")
                .is_some()
        );
    }
}
