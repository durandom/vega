//! Dedicated VPN profile; Chromium mechanics remain in the shared launcher.
use super::*;
mod trust;
use std::fs::{File, OpenOptions};
use std::net::{TcpListener, TcpStream};
use std::os::unix::fs::PermissionsExt;

pub(super) struct Vpn {
    host: String,
    socks: u16,
    port: u16,
    profile: PathBuf,
    state: PathBuf,
    ca_sha256: String,
}

fn fail(message: impl Into<String>) -> Failure {
    Failure::new(
        "vpn",
        message,
        "vega browser vpn status; see vega browser vpn --help",
    )
}

impl Vpn {
    fn load(active: bool) -> Result<Self, Failure> {
        let home = PathBuf::from(env::var_os("HOME").unwrap_or_default());
        let state = resolved(
            &env::var_os("XDG_STATE_HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| home.join(".local/state"))
                .join("vega/browser/vpn"),
        )?;
        let config = env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".config"))
            .join("vega/browser-vpn.json");
        let config = if active && state.join("active.json").is_file() {
            state.join("active.json")
        } else {
            config
        };
        let value: serde_json::Value = match fs::read_to_string(config) {
            Ok(s) => {
                serde_json::from_str(&s).map_err(|e| fail(format!("invalid VPN config: {e}")))?
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => serde_json::json!({}),
            Err(e) => return Err(fail(e.to_string())),
        };
        let obj = value
            .as_object()
            .ok_or_else(|| fail("VPN config must be an object"))?;
        for key in obj.keys() {
            if !["host", "socks_port", "cdp_port", "profile", "ca_sha256"].contains(&key.as_str()) {
                return Err(fail(format!("unknown VPN config field: {key}")));
            }
        }
        let string = |key: &str, default: String| -> Result<String, Failure> {
            match obj.get(key) {
                None => Ok(default),
                Some(v) => v
                    .as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| fail(format!("{key} must be a string"))),
            }
        };
        let port = |key: &str, default: u16| -> Result<u16, Failure> {
            match obj.get(key) {
                None => Ok(default),
                Some(v) => v
                    .as_u64()
                    .and_then(|p| u16::try_from(p).ok())
                    .filter(|p| *p > 1024)
                    .ok_or_else(|| fail(format!("{key} must be a port above 1024"))),
            }
        };
        let mut v = Self {
            host: string("host", "puck".into())?,
            socks: port("socks_port", 1080)?,
            port: port("cdp_port", 9224)?,
            profile: PathBuf::from(string(
                "profile",
                home.join(".local/share/vega-chromium-vpn")
                    .display()
                    .to_string(),
            )?),
            state,
            ca_sha256: string("ca_sha256", trust::DEFAULT_SHA256.into())?.to_ascii_lowercase(),
        };
        if v.ca_sha256.len() != 64 || !v.ca_sha256.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(fail("ca_sha256 must contain exactly 64 hexadecimal digits"));
        }
        if v.host.is_empty()
            || v.host.starts_with('-')
            || !v
                .host
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_.@".contains(&b))
        {
            return Err(fail("invalid SSH host alias"));
        }
        v.profile = resolved(&v.profile)?;
        v.state = resolved(&v.state)?;
        if v.state.to_string_lossy().chars().any(char::is_whitespace) {
            return Err(fail(
                "VPN state path must not contain whitespace (process ownership uses argv tokens)",
            ));
        }
        let g = global();
        if v.port == v.socks || [v.port, v.socks].iter().any(|p| *p == 9223 || *p == g.port) {
            return Err(fail(
                "VPN ports must differ from each other and the global CDP port",
            ));
        }
        // Dedicated names and no symlink components avoid aliases of existing user profiles.
        if !v.profile.is_absolute()
            || !v
                .profile
                .file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with("vega-chromium-vpn"))
            || v.profile
                .components()
                .any(|c| matches!(c, std::path::Component::ParentDir))
            || v.profile.to_string_lossy().chars().any(char::is_whitespace)
            || v.profile.starts_with(resolved(&g.profile)?)
            || v.profile
                .starts_with(resolved(&home.join(".local/share/vega-chromium"))?)
        {
            return Err(fail(
                "profile must be an absolute, non-symlink dedicated vega-chromium-vpn* directory",
            ));
        }
        Ok(v)
    }

    pub(super) fn browser(&self) -> Chromium {
        Chromium {
            port: self.port,
            profile: self.profile.clone(),
            chromium: chromium_override().unwrap_or_else(|| {
                let home = PathBuf::from(env::var_os("HOME").unwrap_or_default());
                vega_browser::chrome::binary(&home)
            }),
            extra_args: vec![
                format!("--proxy-server=socks5://127.0.0.1:{}", self.socks),
                "--proxy-bypass-list=<-loopback>".into(),
                "--disable-quic".into(),
                "--force-webrtc-ip-handling-policy=disable_non_proxied_udp".into(),
            ],
        }
    }

    fn socket(&self) -> PathBuf {
        self.state.join("ssh.sock")
    }

    pub(super) fn lock(&self) -> Result<File, Failure> {
        fs::create_dir_all(&self.state).map_err(|e| fail(e.to_string()))?;
        fs::set_permissions(&self.state, fs::Permissions::from_mode(0o700))
            .map_err(|e| fail(e.to_string()))?;
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(self.state.join("lock"))
            .map_err(|e| fail(e.to_string()))?;
        file.try_lock()
            .map_err(|_| fail("another VPN lifecycle command is running"))?;
        Ok(file)
    }

    fn supervisor(&self) -> Option<u32> {
        let pid: u32 = fs::read_to_string(self.state.join("autossh.pid"))
            .ok()?
            .trim()
            .parse()
            .ok()?;
        let socket = self.socket().display().to_string();
        procs::scan()
            .iter()
            .find(|p| {
                p.pid == pid
                    && p.cmd
                        .split_whitespace()
                        .next()
                        .is_some_and(|s| s.ends_with("autossh"))
                    && p.cmd.split_whitespace().any(|a| a == socket)
            })
            .map(|p| p.pid)
    }

    fn control(&self, op: &str) -> bool {
        Command::new("ssh")
            .args(["-S"])
            .arg(self.socket())
            .args(["-O", op, "-o", "BatchMode=yes", &self.host])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    }

    fn browser_pids(&self) -> Vec<u32> {
        let g = self.browser();
        procs::scan()
            .into_iter()
            .filter(|p| {
                procs::ours(
                    std::slice::from_ref(p),
                    self.port,
                    &self.profile.display().to_string(),
                ) && p.cmd.split_whitespace().any(|a| a == g.extra_args[0])
            })
            .map(|p| p.pid)
            .collect()
    }

    pub(super) fn require_running(&self) -> Result<(), Failure> {
        if self.supervisor().is_none()
            || !self.control("check")
            || !listening(self.socks)
            || self.browser_pids().is_empty()
            || cdp::probe(self.port) != Probe::Cdp
        {
            return Err(fail(
                "VPN profile is not running; run vega browser vpn start",
            ));
        }
        Ok(())
    }

    fn settings(&self) -> serde_json::Value {
        serde_json::json!({"host":self.host,"cdp_port":self.port,"socks_port":self.socks,"profile":self.profile,"ca_sha256":self.ca_sha256})
    }

    fn start(&self) -> Result<(), Failure> {
        if let Ok(saved) = fs::read_to_string(self.state.join("active.json")) {
            if serde_json::from_str::<serde_json::Value>(&saved)
                .ok()
                .as_ref()
                != Some(&self.settings())
            {
                return Err(fail(
                    "VPN configuration changed; run vpn stop before starting again",
                ));
            }
        }
        if self.supervisor().is_some() {
            self.require_running()?;
            return self.trust();
        }
        for port in [self.port, self.socks] {
            TcpListener::bind(("127.0.0.1", port)).map_err(|_| {
                fail(format!(
                    "port {port} is already occupied; refusing to attach"
                ))
            })?;
        }
        if !procs::with_profile(&procs::scan(), &self.profile.display().to_string()).is_empty() {
            return Err(fail(
                "VPN profile already has a browser; stop it before starting",
            ));
        }
        let browser = self.browser();
        if !browser.chromium.is_file() {
            return Err(Failure::new(
                "missing_chromium",
                format!("VPN browser is missing at {}", browser.chromium.display()),
                "run vega browser install, or correct CDP_CHROMIUM",
            ));
        }
        self.trust()?;
        let _ = fs::remove_file(self.socket());
        let mut child = Command::new("nohup")
            .args(["autossh", "-M", "0", "-N", "-T", "-a", "-D"])
            .arg(format!("127.0.0.1:{}", self.socks))
            .args([
                "-o",
                "BatchMode=yes",
                "-o",
                "ExitOnForwardFailure=yes",
                "-o",
                "ConnectTimeout=10",
                "-o",
                "ServerAliveInterval=30",
                "-o",
                "ServerAliveCountMax=3",
                "-o",
                "ControlMaster=yes",
                "-o",
                "ControlPersist=no",
                "-S",
            ])
            .arg(self.socket())
            .arg(&self.host)
            .env("AUTOSSH_GATETIME", "30")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| fail(e.to_string()))?;
        if let Err(e) = fs::write(self.state.join("autossh.pid"), child.id().to_string())
            .and_then(|_| fs::write(self.state.join("active.json"), self.settings().to_string()))
        {
            let _ = child.kill();
            let _ = child.wait();
            return Err(fail(e.to_string()));
        }
        let result = (|| {
            for _ in 0..60 {
                if child.try_wait().map_err(|e| fail(e.to_string()))?.is_some() {
                    return Err(fail(
                        "autossh exited; check SSH BatchMode access and autossh installation",
                    ));
                }
                if listening(self.socks) && self.control("check") {
                    return ensure_chromium(&browser, None).map(|_| ());
                }
                thread::sleep(Duration::from_millis(200));
            }
            Err(fail("SSH SOCKS startup timed out"))
        })();
        if let Err(original) = &result {
            if let Err(cleanup) = self.stop() {
                return Err(fail(format!(
                    "{}; cleanup also failed: {}",
                    original.message, cleanup.message
                )));
            }
        }
        result
    }

    fn stop(&self) -> Result<(), Failure> {
        for pid in self.browser_pids() {
            signal(pid)?;
        }
        if let Some(pid) = self.supervisor() {
            signal(pid)?;
        }
        // Also terminates a surviving master if the supervisor already died.
        if self.socket().exists() {
            self.control("exit");
        }
        for _ in 0..30 {
            if self.browser_pids().is_empty()
                && self.supervisor().is_none()
                && !self.control("check")
            {
                let _ = fs::remove_file(self.state.join("autossh.pid"));
                let _ = fs::remove_file(self.state.join("active.json"));
                return Ok(());
            }
            thread::sleep(Duration::from_millis(100));
        }
        Err(fail("VPN processes have not stopped; state retained"))
    }

    fn status(&self) -> serde_json::Value {
        let supervisor = self.supervisor().is_some();
        let tunnel = supervisor && self.control("check") && listening(self.socks);
        let endpoint = if self.browser_pids().is_empty() {
            None
        } else {
            ureq::AgentBuilder::new()
                .try_proxy_from_env(false)
                .redirects(0)
                .timeout(Duration::from_secs(2))
                .build()
                .get(&format!("http://127.0.0.1:{}/json/version", self.port))
                .call()
                .ok()
                .filter(|r| r.status() == 200)
        };
        let browser = endpoint.is_some();
        let browser_version = endpoint
            .and_then(|r| r.into_json::<serde_json::Value>().ok())
            .and_then(|v| v["Browser"].as_str().map(str::to_owned));
        let corp = if tunnel {
            Command::new("curl")
                .args([
                    "--silent",
                    "--show-error",
                    // Reachability only, no data sent: the corp host uses an internal CA.
                    "--insecure",
                    "--noproxy",
                    "",
                    "--socks5-hostname",
                ])
                .arg(format!("127.0.0.1:{}", self.socks))
                .args([
                    "--connect-timeout",
                    "5",
                    "--max-time",
                    "8",
                    "--output",
                    "/dev/null",
                    "--write-out",
                    "%{http_code}",
                    "https://gitlab.cee.redhat.com",
                ])
                .output()
                .ok()
                .filter(|o| o.status.success())
                .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
        } else {
            None
        };
        serde_json::json!({"profile":"vpn", "host":self.host, "cdp_port":self.port, "socks_port":self.socks,
            "ca_sha256":self.ca_sha256, "ca_trusted":self.ca_trusted().ok(),
            "supervisor":supervisor, "tunnel":tunnel, "browser":browser, "browser_version":browser_version, "corp_http":corp,
            "hint": if corp.is_none() { "Corp host unavailable; check VPN on puck (no automatic probe retries)" } else { "" }})
    }
}

fn resolved(path: &std::path::Path) -> Result<PathBuf, Failure> {
    if !path.is_absolute()
        || path
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return Err(fail("VPN paths must be absolute without parent traversal"));
    }
    for parent in path.ancestors() {
        if parent.exists() {
            return parent
                .canonicalize()
                .map(|p| {
                    let rest = path.strip_prefix(parent).expect("ancestor");
                    if rest.as_os_str().is_empty() {
                        p
                    } else {
                        p.join(rest)
                    }
                })
                .map_err(|e| fail(e.to_string()));
        }
    }
    Err(fail("cannot resolve VPN path"))
}

fn signal(pid: u32) -> Result<(), Failure> {
    Command::new("kill")
        .arg(pid.to_string())
        .output()
        .map_err(|e| fail(e.to_string()))?
        .status
        .success()
        .then_some(())
        .ok_or_else(|| fail(format!("could not stop PID {pid}")))
}

fn listening(port: u16) -> bool {
    TcpStream::connect_timeout(&([127, 0, 0, 1], port).into(), Duration::from_millis(200)).is_ok()
}

pub(super) fn select(args: &[String]) -> Result<(Option<Vpn>, Vec<String>), Failure> {
    let mut result = Vec::new();
    let mut selected = None;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        let value = if arg == "--profile" {
            Some(iter.next().map(String::as_str).unwrap_or(""))
        } else {
            arg.strip_prefix("--profile=")
        };
        if let Some(value) = value {
            if selected.is_some() || !matches!(value, "global" | "vpn") {
                return Err(Failure::new(
                    "usage",
                    "ab profile must be global or vpn (specified once)",
                    "omit --profile for the global Chromium",
                ));
            }
            selected = Some(value);
        } else {
            result.push(arg.clone());
        }
    }
    Ok((
        if selected == Some("vpn") {
            Some(Vpn::load(false)?)
        } else {
            None
        },
        result,
    ))
}

pub(super) fn command(args: &[String]) -> Result<ExitCode, Failure> {
    let (op, json) = match args {
        [op] if ["start", "stop", "status", "trust"].contains(&op.as_str()) => (op.as_str(), false),
        [op, flag]
            if ["start", "stop", "status", "trust"].contains(&op.as_str()) && flag == "--json" =>
        {
            (op.as_str(), true)
        }
        _ => {
            return Err(Failure::new(
                "usage",
                "expected vpn start|stop|status|trust [--json]",
                "vega browser vpn --help",
            ))
        }
    };
    let v = Vpn::load(!["start", "trust"].contains(&op))?;
    let _lock = v.lock()?;
    match op {
        "start" => v.start()?,
        "stop" => v.stop()?,
        "trust" => v.trust()?,
        _ => (),
    }
    let value = if op == "status" {
        v.status()
    } else {
        serde_json::json!({"profile":"vpn", "state":match op { "start" => "started", "trust" => "trusted", _ => "stopped" }})
    };
    if json {
        println!("{value}");
    } else {
        for (key, value) in value.as_object().expect("object") {
            println!(
                "{key}: {}",
                cell(
                    value
                        .as_str()
                        .map(str::to_owned)
                        .unwrap_or_else(|| value.to_string())
                        .as_str()
                )
            );
        }
    }
    Ok(ExitCode::SUCCESS)
}
