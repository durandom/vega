//! vegad's lifecycle (docs/design/vegad.md, Lifecycle): `vega post relay
//! start|stop|status|install|uninstall`, the lock that keeps one relay per
//! host, and the in-place upgrade (the relay execs a replaced binary).
//!
//! herdr starts vegad (plugin vega.vegad, a `[[startup]]` hook running
//! `relay start`); vegad runs detached in its own session and outlives the
//! hook and the herdr server. The herdr-app vega mod runs the idempotent
//! starter on its host and, via `hc exec`, on connected hc machines. No login
//! job exists (vega-0h4c.42); `relay install` removes an old one.

use std::env;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::fs::MetadataExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use vega_herdr::{cell, collapse_home, Failure};

use crate::host;

const LABEL: &str = "vega.post-relay";
const UNIT: &str = "vega-post-relay.service";
const LOGIN_LABEL: &str = "vega.vegad-start";
const LOGIN_UNIT: &str = "vega-vegad-start.service";
const PLUGIN_ID: &str = "vega.vegad";
/// Env vegad keeps from the installing shell, if set there (`vegad.env`).
/// `DOLT_REMOTE_USER` is the beads sync's remote user (no secret; the password stays with the
/// dolt server).
const PINNED_ENV: [&str; 5] = [
    "VEGA_NATS_URL",
    "VEGA_NATS_NKEY",
    "VEGA_POST_HOST",
    "HERDR_SOCKET_PATH",
    "DOLT_REMOTE_USER",
];
/// Env of the starting pane or plugin command that vegad must not inherit.
const PANE_ENV: [&str; 4] = [
    "HERDR_PANE_ID",
    "HERDR_TAB_ID",
    "HERDR_WORKSPACE_ID",
    "HERDR_ENV",
];
fn terminal_env(key: &str) -> bool {
    PANE_ENV.contains(&key) || key.starts_with("HERDR_PLUGIN_") || key.starts_with("HC_")
}

/// The lock's fd, passed across exec by an upgrading vegad.
const LOCK_FD_ENV: &str = "VEGA_RELAY_LOCK_FD";
/// The log moves to `.log.1` above this size when vegad starts.
const LOG_MAX: u64 = 10 * 1024 * 1024;
/// A handoff older than this is from an earlier vegad: ignored.
const HANDOFF_MAX_AGE_MS: u64 = 60_000;

fn home() -> Result<PathBuf, Failure> {
    env::var("HOME")
        .map(PathBuf::from)
        .map_err(|_| Failure::new("no_home", "HOME is not set", "run from a login shell"))
}

pub fn state_dir() -> Result<PathBuf, Failure> {
    Ok(
        match env::var("XDG_STATE_HOME").ok().filter(|s| !s.is_empty()) {
            Some(s) => PathBuf::from(s),
            None => home()?.join(".local/state"),
        }
        .join("vega"),
    )
}

fn io_err(path: &Path, e: std::io::Error) -> Failure {
    Failure::new(
        "service_io",
        format!("{}: {e}", collapse_home(&path.display().to_string())),
        "check the directory permissions",
    )
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

// ---------------------------------------------------------------- lock

/// Held for the relay's lifetime: one relay per host on this machine. Two
/// relays would each type a pointer for the same mail.
pub struct RelayLock {
    file: File,
}

fn lock_path() -> Result<PathBuf, Failure> {
    Ok(state_dir()?.join(format!("post-relay-{}.lock", host())))
}

/// Takes the lock. `wait`: block until a running relay exits (a unit's
/// mode, so a manual relay is taken over without a restart loop); otherwise
/// fail. An upgrading vegad hands its lock over across exec instead.
pub fn lock(wait: bool) -> Result<RelayLock, Failure> {
    let path = lock_path()?;
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| io_err(dir, e))?;
    }
    if let Some(lock) = inherited_lock(&path) {
        return Ok(lock);
    }
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)
        .map_err(|e| io_err(&path, e))?;
    match file.try_lock() {
        Ok(()) => {}
        Err(fs::TryLockError::WouldBlock) => {
            let holder = holder_pid(&path).unwrap_or_else(|| "?".into());
            if !wait {
                return Err(Failure::new(
                    "relay_running",
                    format!("a relay for {} already runs (pid {holder})", host()),
                    "one relay per host; see `vega post relay status`",
                ));
            }
            crate::log(&format!(
                "relay: waiting for the lock (held by pid {holder})"
            ));
            file.lock().map_err(|e| io_err(&path, e))?;
        }
        Err(fs::TryLockError::Error(e)) => return Err(io_err(&path, e)),
    }
    file.set_len(0).map_err(|e| io_err(&path, e))?;
    file.seek(SeekFrom::Start(0))
        .map_err(|e| io_err(&path, e))?;
    writeln!(file, "{}", std::process::id()).map_err(|e| io_err(&path, e))?;
    Ok(RelayLock { file })
}

/// The lock an upgrading vegad kept open across exec (`reexec`): the flock
/// belongs to the open file description, so it was never released. Only an
/// fd that is our lock file counts.
fn inherited_lock(path: &Path) -> Option<RelayLock> {
    let fd: i32 = env::var(LOCK_FD_ENV).ok()?.parse().ok()?;
    env::remove_var(LOCK_FD_ENV);
    // SAFETY: fcntl on an int; fails harmlessly if the fd is not open.
    if unsafe { libc::fcntl(fd, libc::F_GETFD) } < 0 {
        return None;
    }
    // SAFETY: the fd is open and nothing else in this image owns it (exec
    // dropped the File that did). Forgotten below unless it is our lock.
    let file = unsafe { File::from_raw_fd(fd) };
    let same = match (file.metadata(), fs::metadata(path)) {
        (Ok(a), Ok(b)) => a.ino() == b.ino() && a.dev() == b.dev(),
        _ => false,
    };
    if !same {
        std::mem::forget(file); // not ours to close
        return None;
    }
    // SAFETY: fcntl on our own open fd.
    unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) };
    // Same open file description: this succeeds at once.
    file.try_lock().ok()?;
    Some(RelayLock { file })
}

impl RelayLock {
    /// Keeps the lock's fd open across exec; returns its number.
    fn keep_across_exec(&self) -> std::io::Result<i32> {
        let fd = self.file.as_raw_fd();
        // SAFETY: fcntl on our own open fd.
        if unsafe { libc::fcntl(fd, libc::F_SETFD, 0) } < 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(fd)
    }
}

fn mode_path() -> Result<PathBuf, Failure> {
    Ok(state_dir()?.join(format!("post-relay-{}.mode", host())))
}

/// The running relay records its delivery mode for `relay status`, with the
/// local time it started. Best effort: a failed write only loses the line.
pub fn write_mode(mode: &str) {
    let since = vega_herdr::local_time(None).unwrap_or_else(|| "?".into());
    if let Ok(path) = mode_path() {
        let _ = fs::write(path, format!("{mode}, since {since}\n"));
    }
}

fn read_mode() -> Option<String> {
    let s = fs::read_to_string(mode_path().ok()?).ok()?;
    let s = s.trim();
    (!s.is_empty()).then(|| s.to_string())
}

fn broker_path() -> Result<PathBuf, Failure> {
    Ok(state_dir()?.join(format!("post-relay-{}.broker", host())))
}

/// The running relay records its broker state for `relay status` and
/// `vega_herdr::state::relay_local`: `connected` or `local (<why>)`.
pub fn write_broker(state: &str) {
    if let Ok(path) = broker_path() {
        let _ = fs::write(path, format!("{state}\n"));
    }
}

fn read_broker() -> Option<String> {
    let s = fs::read_to_string(broker_path().ok()?).ok()?;
    let s = s.trim();
    (!s.is_empty()).then(|| s.to_string())
}

/// Pid written by the lock holder; only meaningful while the lock is held.
fn holder_pid(path: &Path) -> Option<String> {
    let mut s = String::new();
    File::open(path).ok()?.read_to_string(&mut s).ok()?;
    let s = s.trim();
    (!s.is_empty()).then(|| s.to_string())
}

/// Some(pid) if a relay holds the lock right now.
fn lock_holder() -> Option<String> {
    let path = lock_path().ok()?;
    let file = OpenOptions::new().read(true).write(true).open(&path).ok()?;
    match file.try_lock() {
        Ok(()) => None, // nobody holds it; dropping the file releases ours
        Err(_) => holder_pid(&path).or_else(|| Some("?".into())),
    }
}

// ------------------------------------------------- running state, upgrade

/// What identifies an executable file: a rename (cargo install) changes the
/// inode, an in-place copy the size or mtime.
#[derive(Clone, Debug, PartialEq)]
pub struct ExeId(String);

fn exe_id(path: &Path) -> Option<ExeId> {
    let m = fs::metadata(path).ok()?;
    Some(ExeId(format!(
        "{}:{}:{}:{}.{}",
        m.dev(),
        m.ino(),
        m.len(),
        m.mtime(),
        m.mtime_nsec()
    )))
}

fn state_path() -> Result<PathBuf, Failure> {
    Ok(state_dir()?.join(format!("post-relay-{}.json", host())))
}

/// A replaced binary must stay unchanged this long before vegad execs it:
/// the install is done.
const SETTLE: Duration = Duration::from_secs(1);

/// Notices a replaced binary: due once the file differs from the one this
/// image started from and has not changed for `settle`.
pub struct Upgrade {
    exe: PathBuf,
    running: Option<ExeId>,
    seen: Option<ExeId>,
    seen_at: Instant,
    settle: Duration,
    tried: Option<ExeId>,
}

impl Upgrade {
    /// The path is taken now: on Linux `current_exe` of a replaced binary
    /// reads "<path> (deleted)".
    pub fn new() -> Self {
        let exe = env::current_exe().unwrap_or_default();
        Self::at(exe, SETTLE)
    }

    fn at(exe: PathBuf, settle: Duration) -> Self {
        let running = exe_id(&exe);
        Upgrade {
            seen: running.clone(),
            seen_at: Instant::now(),
            settle,
            exe,
            running,
            tried: None,
        }
    }

    pub fn exe(&self) -> &Path {
        &self.exe
    }

    /// Cheap (one stat); call as often as the loop turns.
    pub fn due(&mut self) -> bool {
        let now = exe_id(&self.exe);
        if now != self.seen {
            self.seen = now;
            self.seen_at = Instant::now();
            return false;
        }
        self.seen.is_some()
            && self.seen_at.elapsed() >= self.settle
            && self.seen != self.running
            && self.seen != self.tried
    }

    /// The exec failed: wait for the next change of the file.
    pub fn failed(&mut self) {
        self.tried = self.seen.clone();
    }

    /// Records the running image for `relay status` (best effort).
    pub fn write_state(&self) {
        let v = json!({
            "pid": std::process::id(),
            "version": env!("VEGA_VERSION"),
            "exe": self.exe.display().to_string(),
            "exe_id": self.running.as_ref().map(|i| i.0.clone()),
            "started_ms": now_ms(),
        });
        if let Ok(p) = state_path() {
            let _ = fs::write(p, format!("{v}\n"));
        }
    }
}

fn handoff_path() -> Result<PathBuf, Failure> {
    Ok(state_dir()?.join(format!("post-relay-{}.handoff.json", host())))
}

/// Saves the relay's state for the image it execs next.
pub fn save_handoff(state: Value) -> Result<(), String> {
    let v = json!({ "pid": std::process::id(), "at_ms": now_ms(), "state": state });
    let p = handoff_path().map_err(|f| f.message)?;
    fs::write(&p, format!("{v}\n")).map_err(|e| e.to_string())
}

/// The state the previous image of this process saved, once.
pub fn take_handoff() -> Option<Value> {
    let p = handoff_path().ok()?;
    let text = fs::read_to_string(&p).ok()?;
    let _ = fs::remove_file(&p);
    handoff_for(&text, std::process::id(), now_ms())
}

/// Only our own pid's (exec keeps the pid) and only a fresh one.
fn handoff_for(text: &str, pid: u32, now: u64) -> Option<Value> {
    let v: Value = serde_json::from_str(text).ok()?;
    let fresh = now.saturating_sub(v["at_ms"].as_u64()?) <= HANDOFF_MAX_AGE_MS;
    (v["pid"].as_u64()? == u64::from(pid) && fresh).then(|| v["state"].clone())
}

/// Execs `exe` with this process's arguments, the lock kept. Returns only
/// on failure, with the reason.
pub fn reexec(lock: &RelayLock, exe: &Path) -> String {
    let fd = match lock.keep_across_exec() {
        Ok(fd) => fd,
        Err(e) => return format!("lock fd: {e}"),
    };
    let err = Command::new(exe)
        .args(env::args_os().skip(1))
        .env(LOCK_FD_ENV, fd.to_string())
        .exec();
    // SAFETY: our own fd; restore close-on-exec after the failed exec.
    unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) };
    err.to_string()
}

/// The running relay's recorded state, if it is the lock holder's.
fn running_state(pid: &str) -> Option<Value> {
    let v: Value = serde_json::from_str(&fs::read_to_string(state_path().ok()?).ok()?).ok()?;
    (v["pid"].as_u64()?.to_string() == pid).then_some(v)
}

/// current: runs the file now at its path; stale: the file was replaced
/// (re-exec pending, or a relay from before self-exec); unknown: no record.
fn binary_state(state: Option<&Value>) -> &'static str {
    let Some(s) = state else { return "unknown" };
    let (Some(exe), Some(id)) = (s["exe"].as_str(), s["exe_id"].as_str()) else {
        return "unknown";
    };
    match exe_id(Path::new(exe)) {
        Some(now) if now.0 == id => "current",
        _ => "stale",
    }
}

// ------------------------------------------------------------- env file

fn env_file() -> Result<PathBuf, Failure> {
    Ok(state_dir()?.join("vegad.env"))
}

#[cfg(test)]
fn parse_env(text: &str) -> Vec<(String, String)> {
    vega_herdr::vegad_env::parse(text)
        .into_iter()
        .filter(|(k, _)| PINNED_ENV.contains(&k.as_str()))
        .collect()
}

fn read_env_file() -> Vec<(String, String)> {
    vega_herdr::vegad_env::read()
        .into_iter()
        .filter(|(k, _)| PINNED_ENV.contains(&k.as_str()))
        .collect()
}

/// Applies `vegad.env` to this process, so `relay start|stop|status` find
/// the same lock (VEGA_POST_HOST) as the vegad they start.
pub fn load_env_file() {
    let shell_host = env::var("VEGA_POST_HOST").is_ok_and(|h| !h.is_empty());
    for (k, v) in read_env_file() {
        if k == "VEGA_POST_HOST" && shell_host {
            continue;
        }
        env::set_var(k, v);
    }
}

// ------------------------------------------------------------- start

fn log_path() -> Result<PathBuf, Failure> {
    Ok(state_dir()?.join(format!("post-relay-{}.log", host())))
}

/// Directory of the first `name` on PATH, skipping vega's agent-only shims.
fn dir_of(name: &str) -> Option<String> {
    let path = env::var("PATH").ok()?;
    env::split_paths(&path)
        .filter(|d| !is_shims(d))
        .find(|d| d.join(name).is_file())
        .map(|d| d.display().to_string())
}

fn is_shims(d: &Path) -> bool {
    d.to_string_lossy().contains("/vega/shims")
}

/// vegad's PATH: its own directory, herdr's, vega's, then the starter's
/// PATH without the agent shims, then the system directories.
fn daemon_path(exe: &Path, inherited: &str, herdr_bin: Option<&str>) -> String {
    daemon_path_with(exe, inherited, herdr_bin, [dir_of("herdr"), dir_of("vega")])
}

// Keep host discovery at the boundary so PATH assembly can be tested in isolation.
fn daemon_path_with(
    exe: &Path,
    inherited: &str,
    herdr_bin: Option<&str>,
    discovered: [Option<String>; 2],
) -> String {
    let [herdr_dir, vega_dir] = discovered;
    let mut dirs: Vec<String> = Vec::new();
    let first = [
        exe.parent().map(|p| p.display().to_string()),
        herdr_bin
            .and_then(|b| Path::new(b).parent())
            .map(|p| p.display().to_string()),
        herdr_dir,
        vega_dir,
    ];
    let rest = env::split_paths(inherited)
        .filter(|d| !is_shims(d) && !d.as_os_str().is_empty())
        .map(|d| d.display().to_string());
    let system = ["/usr/bin", "/bin", "/usr/sbin", "/sbin"].map(String::from);
    for d in first.into_iter().flatten().chain(rest).chain(system) {
        if !dirs.contains(&d) {
            dirs.push(d);
        }
    }
    dirs.join(":")
}

/// Spawns `exe relay` detached: own session, stdin from /dev/null, output
/// appended to the log. herdr reads a hook's pipes to EOF, so vegad must not
/// keep them.
fn spawn_detached(exe: &Path) -> Result<std::process::Child, Failure> {
    let log = log_path()?;
    if let Some(d) = log.parent() {
        fs::create_dir_all(d).map_err(|e| io_err(d, e))?;
    }
    if fs::metadata(&log).is_ok_and(|m| m.len() > LOG_MAX) {
        let _ = fs::rename(&log, log.with_extension("log.1"));
    }
    let out = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log)
        .map_err(|e| io_err(&log, e))?;
    let err = out.try_clone().map_err(|e| io_err(&log, e))?;
    let mut cmd = Command::new(exe);
    cmd.arg("relay")
        .stdin(Stdio::null())
        .stdout(out)
        .stderr(err)
        .env(
            "PATH",
            daemon_path(
                exe,
                &env::var("PATH").unwrap_or_default(),
                env::var("HERDR_BIN_PATH").ok().as_deref(),
            ),
        );
    for (k, _) in env::vars().filter(|(k, _)| terminal_env(k)) {
        cmd.env_remove(k);
    }
    for (k, v) in read_env_file() {
        if k != "VEGA_POST_HOST" {
            cmd.env(k, v);
        }
    }
    // The child must take the same host lock as its starter, including a
    // shell override that differs from the value pinned at installation.
    cmd.env("VEGA_POST_HOST", host());
    // SAFETY: setsid is async-signal-safe.
    unsafe {
        cmd.pre_exec(|| {
            if libc::setsid() < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    cmd.spawn().map_err(|e| {
        Failure::new(
            "start_failed",
            format!("{}: {e}", collapse_home(&exe.display().to_string())),
            "check the binary; `vega post relay status`",
        )
    })
}

/// `relay start`: starts vegad unless one runs. Idempotent; herdr's startup
/// hook and the inbox call it.
pub fn start() -> Result<(), Failure> {
    load_env_file();
    if let Some(pid) = lock_holder() {
        println!("relay: running (pid {pid})");
        return Ok(());
    }
    if let Some(u) = legacy_unit() {
        // Not migrated yet: the unit keeps owning the relay.
        println!(
            "relay: {}",
            if kickstart(u) {
                "started (legacy unit; `vega post relay install` migrates)"
            } else {
                "start failed (legacy unit); see `vega post relay status`"
            }
        );
        return Ok(());
    }
    let exe = env::current_exe()
        .map_err(|e| Failure::new("no_exe", e.to_string(), "run the installed vega-post"))?;
    start_exe(&exe)
}

fn start_exe(exe: &Path) -> Result<(), Failure> {
    let mut child = spawn_detached(exe)?;
    let until = Instant::now() + Duration::from_secs(3);
    while Instant::now() < until {
        if let Some(pid) = lock_holder() {
            println!("relay: started (pid {pid})");
            println!("log: {}", collapse_home(&log_path()?.display().to_string()));
            return Ok(());
        }
        if let Ok(Some(status)) = child.try_wait() {
            return Err(Failure::new(
                "start_failed",
                format!("vegad exited at start ({status})"),
                "see the log: `vega post relay status`",
            ));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Err(Failure::new(
        "start_slow",
        format!("vegad (pid {}) holds no lock after 3 s", child.id()),
        "see `vega post relay status`",
    ))
}

/// `relay stop`: SIGTERM to the lock holder, then wait for the lock.
pub fn stop() -> Result<(), Failure> {
    load_env_file();
    let Some(pid) = lock_holder() else {
        println!("relay: not running");
        return Ok(());
    };
    terminate(&pid)?;
    println!("relay: stopped (pid {pid})");
    if legacy_unit().is_some() {
        println!("help: a legacy unit restarts it; `vega post relay install` migrates");
    }
    Ok(())
}

fn terminate(pid: &str) -> Result<(), Failure> {
    let n: i32 = pid.parse().map_err(|_| {
        Failure::new(
            "no_pid",
            format!("the lock names no pid ({pid})"),
            "see `vega post relay status`",
        )
    })?;
    // SAFETY: kill with a pid from our own lock file.
    if unsafe { libc::kill(n, libc::SIGTERM) } < 0 {
        return Err(Failure::new(
            "stop_failed",
            format!("kill {n}: {}", std::io::Error::last_os_error()),
            "see `vega post relay status`",
        ));
    }
    wait_lock_free(Duration::from_secs(5));
    Ok(())
}

fn wait_lock_free(within: Duration) -> bool {
    let until = Instant::now() + within;
    while Instant::now() < until {
        if lock_holder().is_none() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    false
}

// ------------------------------------------------- legacy units, install

#[derive(Clone, Copy, PartialEq)]
enum Manager {
    Launchd,
    Systemd,
}

fn manager() -> Option<Manager> {
    match env::consts::OS {
        "macos" => Some(Manager::Launchd),
        "linux" => Some(Manager::Systemd),
        _ => None,
    }
}

fn unit_path(m: Manager) -> Result<PathBuf, Failure> {
    Ok(match m {
        Manager::Launchd => home()?.join(format!("Library/LaunchAgents/{LABEL}.plist")),
        Manager::Systemd => {
            let config = match env::var("XDG_CONFIG_HOME").ok().filter(|s| !s.is_empty()) {
                Some(c) => PathBuf::from(c),
                None => home()?.join(".config"),
            };
            config.join("systemd/user").join(UNIT)
        }
    })
}

/// The launchd / systemd unit from before vega-lhc.54, if still installed.
fn legacy_unit() -> Option<Manager> {
    let m = manager()?;
    unit_path(m).ok()?.exists().then_some(m)
}

fn run(cmd: &str, args: &[&str]) -> (bool, String) {
    match Command::new(cmd).args(args).output() {
        Ok(o) => (
            o.status.success(),
            format!(
                "{}{}",
                String::from_utf8_lossy(&o.stdout),
                String::from_utf8_lossy(&o.stderr)
            )
            .trim()
            .to_string(),
        ),
        Err(e) => (false, e.to_string()),
    }
}

fn uid() -> String {
    run("id", &["-u"]).1
}

fn kickstart(m: Manager) -> bool {
    match m {
        Manager::Launchd => {
            let domain = format!("gui/{}", uid());
            run("launchctl", &["kickstart", &format!("{domain}/{LABEL}")]).0
                || unit_path(m).is_ok_and(|u| {
                    run(
                        "launchctl",
                        &["bootstrap", &domain, &u.display().to_string()],
                    )
                    .0
                })
        }
        Manager::Systemd => run("systemctl", &["--user", "start", UNIT]).0,
    }
}

/// Stops and removes a legacy unit (its relay exits with it). Returns the
/// removed path.
fn remove_legacy_unit() -> Result<Option<PathBuf>, Failure> {
    let Some(m) = legacy_unit() else {
        return Ok(None);
    };
    let unit = unit_path(m)?;
    match m {
        Manager::Launchd => {
            let _ = run("launchctl", &["bootout", &format!("gui/{}/{LABEL}", uid())]);
        }
        Manager::Systemd => {
            let _ = run("systemctl", &["--user", "disable", "--now", UNIT]);
        }
    }
    fs::remove_file(&unit).map_err(|e| io_err(&unit, e))?;
    if m == Manager::Systemd {
        let _ = run("systemctl", &["--user", "daemon-reload"]);
    }
    Ok(Some(unit))
}

/// Name of the toolbox container we run in, from `/run/.containerenv`.
/// None on the host and in non-toolbox containers.
fn toolbox_name() -> Option<String> {
    if !Path::new("/run/.toolboxenv").exists() {
        return None;
    }
    parse_container_name(&fs::read_to_string("/run/.containerenv").ok()?)
}

fn parse_container_name(containerenv: &str) -> Option<String> {
    containerenv
        .lines()
        .find_map(|l| l.strip_prefix("name=\""))
        .and_then(|v| v.strip_suffix('"'))
        .filter(|n| !n.is_empty())
        .map(String::from)
}

/// The installed binary (the one updates replace) and the env to pin.
struct Plan {
    exe: PathBuf,
    /// Toolbox container to run the relay in; herdr's hook runs on the host.
    container: Option<String>,
    env: Vec<(String, String)>,
}

fn plan() -> Result<Plan, Failure> {
    let exe = env::current_exe()
        .and_then(|p| p.canonicalize())
        .map_err(|e| Failure::new("no_exe", e.to_string(), "run the installed vega-post"))?;
    // herdr's hook must run the binary that updates replace in place: the
    // `vega-post` on PATH, never a build output or a copy elsewhere.
    let on_path =
        dir_of("vega-post").and_then(|d| Path::new(&d).join("vega-post").canonicalize().ok());
    if on_path.as_deref() != Some(exe.as_path()) {
        return Err(Failure::new(
            "not_installed_binary",
            format!(
                "{} is not the vega-post on PATH ({})",
                collapse_home(&exe.display().to_string()),
                on_path
                    .map(|p| collapse_home(&p.display().to_string()))
                    .unwrap_or_else(|| "none".into())
            ),
            "install vega first (`mise run install`), then run `vega post relay install`",
        ));
    }
    let mut env: Vec<(String, String)> = PINNED_ENV
        .iter()
        .filter_map(|k| {
            env::var(k)
                .ok()
                .filter(|v| !v.is_empty())
                .map(|v| (k.to_string(), v))
        })
        .collect();
    // Always pin the resolved identity, including an existing vegad.env pin.
    env.retain(|(k, _)| k != "VEGA_POST_HOST");
    env.push(("VEGA_POST_HOST".into(), host()));
    Ok(Plan {
        exe,
        container: toolbox_name(),
        env,
    })
}

/// The startup hook's manifest: `relay start` with this binary's absolute
/// path, so herdr's PATH does not matter. Built in a toolbox, the binary needs
/// the toolbox glibc, not the host's: the hook (host side) enters the container.
fn manifest(exe: &str, container: Option<&str>) -> String {
    let command = match container {
        Some(c) => format!("\"/usr/bin/toolbox\", \"run\", \"-c\", \"{c}\", \"{exe}\""),
        None => format!("\"{exe}\""),
    };
    format!(
        "# Generated by `vega post relay install`: starts vegad with herdr.\nid = \"{PLUGIN_ID}\"\nname = \"vegad\"\nversion = \"0.2.0\"\nmin_herdr_version = \"0.9.0\"\ndescription = \"Starts vega's per-machine daemon (vegad) when herdr starts\"\nplatforms = [\"linux\", \"macos\"]\n\n[[startup]]\ncommand = [{command}, \"relay\", \"start\"]\n"
    )
}

fn plugin_dir() -> Result<PathBuf, Failure> {
    let data = env::var("XDG_DATA_HOME")
        .ok()
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .unwrap_or(home()?.join(".local/share"));
    Ok(data.join("vega/herdr-plugin"))
}

/// Writes plugin vega.vegad and links it once. herdr re-reads a linked
/// manifest from disk, so a rewrite needs no new link.
fn link_herdr_plugin(exe: &str, container: Option<&str>) -> String {
    if dir_of("herdr").is_none() {
        return "skipped (herdr not installed)".into();
    }
    let Ok(dir) = plugin_dir() else {
        return "skipped (no HOME)".into();
    };
    if fs::create_dir_all(&dir).is_err()
        || fs::write(dir.join("herdr-plugin.toml"), manifest(exe, container)).is_err()
    {
        return format!(
            "skipped (cannot write {})",
            collapse_home(&dir.display().to_string())
        );
    }
    let (ok, list) = run("herdr", &["plugin", "list"]);
    if !ok {
        return "skipped (herdr not reachable)".into();
    }
    if list.contains(PLUGIN_ID) {
        return format!("{PLUGIN_ID} linked (manifest updated)");
    }
    match run("herdr", &["plugin", "link", &dir.display().to_string()]) {
        (true, _) => format!("{PLUGIN_ID} linked (starts vegad with herdr)"),
        (false, out) => format!("link failed: {}", out.lines().next().unwrap_or("")),
    }
}

fn write_env_file(env: &[(String, String)]) -> Result<PathBuf, Failure> {
    let p = env_file()?;
    if let Some(d) = p.parent() {
        fs::create_dir_all(d).map_err(|e| io_err(d, e))?;
    }
    let mut text = "# Written by `vega post relay install`: env vegad starts with.\n".to_string();
    for (k, v) in env {
        text.push_str(&format!("{k}={v}\n"));
    }
    fs::write(&p, text).map_err(|e| io_err(&p, e))?;
    Ok(p)
}

// vega-0h4c.25 installed a login job that ran `relay start`. It is gone (vega-0h4c.42): the
// herdr-app vega mod starts vegad on this host and on connected hc machines. These helpers only
// find and remove a job left by an earlier `relay install`.
fn login_path(m: Manager) -> Result<PathBuf, Failure> {
    let old = unit_path(m)?;
    Ok(old.with_file_name(match m {
        Manager::Launchd => format!("{LOGIN_LABEL}.plist"),
        Manager::Systemd => LOGIN_UNIT.into(),
    }))
}

/// Removes the old login job; returns its path if there was one.
fn remove_login() -> Result<Option<PathBuf>, Failure> {
    let Some(m) = manager() else {
        return Ok(None);
    };
    remove_login_for(m)
}

fn remove_login_for(m: Manager) -> Result<Option<PathBuf>, Failure> {
    let path = login_path(m)?;
    if !path.exists() {
        return Ok(None);
    }
    let result = match m {
        Manager::Launchd => {
            let service = format!("gui/{}/{LOGIN_LABEL}", uid());
            if run("launchctl", &["print", &service]).0 {
                run("launchctl", &["bootout", &service])
            } else {
                (true, String::new())
            }
        }
        Manager::Systemd => run("systemctl", &["--user", "disable", "--now", LOGIN_UNIT]),
    };
    if !result.0 {
        return Err(Failure::new(
            "login_service",
            result.1,
            "retry vega post relay uninstall in a logged-in user session",
        ));
    }
    fs::remove_file(&path).map_err(|e| io_err(&path, e))?;
    if m == Manager::Systemd {
        let _ = run("systemctl", &["--user", "daemon-reload"]);
    }
    Ok(Some(path))
}

/// What `relay install` says about the old login job: the line, and whether it is a warning.
fn login_removal_report(r: Result<Option<PathBuf>, Failure>) -> (Option<String>, bool) {
    match r {
        Ok(Some(job)) => (
            Some(format!(
                "login_service: {} (removed; vegad starts from the herdr-app vega mod)",
                collapse_home(&job.display().to_string())
            )),
            false,
        ),
        Ok(None) => (None, false),
        Err(e) => (
            Some(format!(
                "warning: old login job not removed: {} ({})",
                e.message, e.hint
            )),
            true,
        ),
    }
}

/// Verification is advisory: a first host or an offline/local installation must
/// still be installable. Do it before changing service files or starting vegad.
fn install_host_warning() -> Result<Option<String>, Failure> {
    if crate::url().is_empty() {
        return Ok(None);
    }
    let resolved = host();
    // An inbox-only host cannot read the registry by design: nothing to verify.
    if vega_herdr::state::is_inbox_only(&resolved) {
        return Ok(None);
    }
    let known = crate::runtime()?.block_on(async {
        tokio::time::timeout(crate::reach::WAIT, async {
            let client = crate::connect("relay-install", None)
                .await
                .map_err(|e| e.message)?;
            vega_registry::get_one(&client, &vega_registry::beat_key(&resolved)).await
        })
        .await
    });
    Ok(host_warning(&resolved, matches!(known, Ok(Ok(Some(_))))))
}

fn host_warning(resolved: &str, known: bool) -> Option<String> {
    (!known).then(|| {
        format!(
            "relay host {resolved} is not verified in the registry (unknown or unavailable); \
         if the hostname differs from the registry host, run \
         `VEGA_POST_HOST=<registry-host> vega post relay install` before starting it; \
         check `vega post relay status`"
        )
    })
}

/// `relay install`: migrates from a launchd / systemd unit, pins the env,
/// links herdr's startup hook and starts vegad. Idempotent.
pub fn install() -> Result<(), Failure> {
    let p = plan()?;
    if let Some(warning) = install_host_warning()? {
        eprintln!("warning: {warning}");
    }
    if let Some(unit) = remove_legacy_unit()? {
        println!(
            "legacy_unit: {} (stopped, removed)",
            collapse_home(&unit.display().to_string())
        );
        wait_lock_free(Duration::from_secs(5));
    }
    let envf = write_env_file(&p.env)?;
    load_env_file();
    let exe = p.exe.display().to_string();
    println!("exe: {}", collapse_home(&exe));
    if let Some(c) = &p.container {
        println!("container: {c} (herdr's hook runs the relay via `toolbox run`)");
    }
    println!("env: {}", collapse_home(&envf.display().to_string()));
    for (k, v) in &p.env {
        println!("  {k}={}", cell(v));
    }
    println!(
        "herdr_plugin: {}",
        link_herdr_plugin(&exe, p.container.as_deref())
    );
    match lock_holder() {
        Some(pid) => println!("relay: running (pid {pid})"),
        None => start_exe(&p.exe)?,
    }
    // Migration: a login job from before vega-0h4c.42 is removed; failing to do so only warns.
    match login_removal_report(remove_login()) {
        (line, false) => line.iter().for_each(|l| println!("{l}")),
        (line, true) => line.iter().for_each(|l| eprintln!("{l}")),
    }
    println!("help: vega post relay status");
    Ok(())
}

/// `relay uninstall`: stops vegad, unlinks the startup hook, removes a
/// legacy unit and the env file.
pub fn uninstall() -> Result<(), Failure> {
    load_env_file();
    remove_login()?;
    if let Some(unit) = remove_legacy_unit()? {
        println!(
            "legacy_unit: {} (removed)",
            collapse_home(&unit.display().to_string())
        );
    }
    match lock_holder() {
        Some(pid) => {
            terminate(&pid)?;
            println!("relay: stopped (pid {pid})");
        }
        None => println!("relay: not running"),
    }
    let (ok, list) = run("herdr", &["plugin", "list"]);
    let plugin = if !ok {
        "skipped (herdr not reachable)".to_string()
    } else if !list.contains(PLUGIN_ID) {
        "not linked".to_string()
    } else {
        match run("herdr", &["plugin", "unlink", PLUGIN_ID]) {
            (true, _) => format!("{PLUGIN_ID} unlinked"),
            (false, out) => format!("unlink failed: {}", out.lines().next().unwrap_or("")),
        }
    };
    println!("herdr_plugin: {plugin}");
    if let Ok(p) = env_file() {
        if fs::remove_file(&p).is_ok() {
            println!("env: {} (removed)", collapse_home(&p.display().to_string()));
        }
    }
    Ok(())
}

// ------------------------------------------------------------- status

/// What `relay status` knows, also as `--json` (the inbox reads it).
fn status_value() -> Result<Value, Failure> {
    let holder = lock_holder();
    let state = holder.as_deref().and_then(running_state);
    let legacy = legacy_unit()
        .and_then(|m| unit_path(m).ok())
        .map(|p| collapse_home(&p.display().to_string()));
    Ok(json!({
        "host": host(),
        "running": holder.is_some(),
        "pid": holder,
        "version": state.as_ref().and_then(|s| s["version"].as_str().map(String::from)),
        "binary": if holder.is_some() { binary_state(state.as_ref()) } else { "-" },
        "exe": state.as_ref().and_then(|s| s["exe"].as_str().map(collapse_home)),
        "since": state.as_ref()
            .and_then(|s| s["started_ms"].as_u64())
            .and_then(|ms| vega_herdr::local_time(Some((ms / 1000) as i64))),
        "mode": holder.as_ref().and_then(|_| read_mode()),
        "broker": holder.as_ref().and_then(|_| read_broker()),
        "legacy_unit": legacy,
        "login_service": manager().and_then(|m| login_path(m).ok()).filter(|p| p.is_file()).map(|p| collapse_home(&p.display().to_string())),
        "log": collapse_home(&log_path()?.display().to_string()),
    }))
}

/// The `relay status` line for an old login job; none once it is gone.
fn login_service_line(v: &Value) -> Option<String> {
    v["login_service"].as_str().map(|job| {
        format!("login_service: {job} (old login job; `vega post relay install` removes it)")
    })
}

pub fn status(as_json: bool) -> Result<(), Failure> {
    load_env_file();
    let v = status_value()?;
    if as_json {
        println!("{v}");
        return Ok(());
    }
    let s = |k: &str| v[k].as_str().unwrap_or("-").to_string();
    println!("host: {}", s("host"));
    if v["running"].as_bool() == Some(true) {
        println!("vegad: running, pid {}, since {}", s("pid"), s("since"));
        println!("version: {}, binary {}", s("version"), s("binary"));
        println!("exe: {}", s("exe"));
        println!("mode: {}", s("mode"));
        let broker = s("broker");
        println!("broker: {broker}");
        if broker.starts_with("local") {
            println!(
                "off: {} (vegad probes for a broker every minute)",
                crate::LOCAL_OFF
            );
        }
    } else {
        println!("vegad: stopped");
    }
    if let Some(u) = v["legacy_unit"].as_str() {
        println!("legacy_unit: {u} (installed; `vega post relay install` migrates)");
    }
    if let Some(line) = login_service_line(&v) {
        println!("{line}");
    }
    let log = log_path()?;
    let tail = fs::read_to_string(&log)
        .map(|t| {
            let lines: Vec<&str> = t.lines().collect();
            lines[lines.len().saturating_sub(5)..].join("\n")
        })
        .unwrap_or_default();
    if tail.trim().is_empty() {
        println!("log: {} (empty)", s("log"));
    } else {
        println!("log[{}]: {}", tail.lines().count(), s("log"));
        for l in tail.lines() {
            println!("  {l}");
        }
    }
    if v["running"].as_bool() != Some(true) {
        println!("help: vega post relay start");
    } else if s("binary") == "stale" {
        println!("help: the binary was replaced; vegad re-execs within seconds (a relay from before self-exec: `vega post relay install`)");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_identity_checks_registry_and_preserves_wsl_pin() {
        use std::net::TcpListener;
        use std::os::unix::fs::PermissionsExt;
        if env::var_os("VEGA_INSTALL_HOST_TEST").is_none() {
            let d = tempfile::tempdir().unwrap();
            let bin = d.path().join("bin");
            fs::create_dir(&bin).unwrap();
            fs::write(bin.join("hostname"), "#!/bin/sh\necho io-wsl\n").unwrap();
            fs::set_permissions(bin.join("hostname"), fs::Permissions::from_mode(0o755)).unwrap();
            std::os::unix::fs::symlink(env::current_exe().unwrap(), bin.join("vega-post")).unwrap();
            let port = TcpListener::bind("127.0.0.1:0")
                .unwrap()
                .local_addr()
                .unwrap()
                .port();
            let mut server = Command::new("nats-server")
                .args(["-js", "-a", "127.0.0.1", "-p", &port.to_string(), "-sd"])
                .arg(d.path().join("nats"))
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap();
            let status = Command::new(env::current_exe().unwrap())
                .args([
                    "--exact",
                    "service::tests::install_identity_checks_registry_and_preserves_wsl_pin",
                ])
                .env_clear()
                .env("VEGA_INSTALL_HOST_TEST", "1")
                .env("VEGA_NATS_URL", format!("nats://127.0.0.1:{port}"))
                .env("HOME", d.path())
                .env("PATH", &bin)
                .status()
                .unwrap();
            let _ = server.kill();
            let _ = server.wait();
            assert!(status.success());
            return;
        }
        // The real hostname resolver and planner run against a WSL-style fake.
        assert_eq!(host(), "io-wsl");
        let rt = crate::runtime().unwrap();
        rt.block_on(async {
            let until = Instant::now() + Duration::from_secs(5);
            let client = loop {
                if let Ok(c) = async_nats::connect(crate::url()).await {
                    break c;
                }
                assert!(Instant::now() < until);
                tokio::time::sleep(Duration::from_millis(20)).await;
            };
            let kv = async_nats::jetstream::new(client)
                .create_key_value(async_nats::jetstream::kv::Config {
                    bucket: vega_registry::BUCKET.into(),
                    ..Default::default()
                })
                .await
                .unwrap();
            kv.put(
                vega_registry::beat_key("io"),
                json!({"schema":2,"at":now_ms()/1000}).to_string().into(),
            )
            .await
            .unwrap();
        });
        assert!(install_host_warning().unwrap().unwrap().contains("io-wsl"));
        let initial = plan().unwrap();
        assert!(initial
            .env
            .contains(&("VEGA_POST_HOST".into(), "io-wsl".into())));
        env::set_var("VEGA_POST_HOST", "io");
        assert!(install_host_warning().unwrap().is_none());
        write_env_file(&plan().unwrap().env).unwrap();
        env::remove_var("VEGA_POST_HOST");
        assert_eq!(host(), "io");
        let pinned = plan().unwrap();
        assert!(pinned.env.contains(&("VEGA_POST_HOST".into(), "io".into())));
        assert!(install_host_warning().unwrap().is_none());
        env::set_var("VEGA_POST_HOST", "replacement");
        assert!(plan()
            .unwrap()
            .env
            .contains(&("VEGA_POST_HOST".into(), "replacement".into())));
        env::set_var("VEGA_NATS_URL", "nats://127.0.0.1:1");
        assert!(install_host_warning().unwrap().is_some());
        // An inbox-only host cannot read the registry: no false warning.
        env::set_var("VEGA_POST_INBOX_ONLY", "replacement");
        assert!(install_host_warning().unwrap().is_none());
        env::remove_var("VEGA_POST_INBOX_ONLY");
    }

    #[test]
    fn install_warns_for_unverified_host_identity() {
        let warning = host_warning("io-wsl", false).unwrap();
        assert!(warning.contains("io-wsl"));
        assert!(warning.contains("VEGA_POST_HOST=<registry-host> vega post relay install"));
        assert!(host_warning("io", true).is_none());
    }

    /// `relay install` writes no login job and removes one from vega-0h4c.25. The fake managers
    /// run in a subprocess so HOME and PATH stay private to the test.
    #[test]
    fn old_login_jobs_are_removed_and_never_written() {
        use std::os::unix::fs::PermissionsExt;
        if env::var_os("VEGA_LOGIN_TEST").is_none() {
            let d = tempfile::tempdir().unwrap();
            let bin = d.path().join("bin");
            fs::create_dir(&bin).unwrap();
            for (name, body) in [
                ("id", "echo 123"),
                (
                    "launchctl",
                    r#"echo "$*" >> "$HOME/calls"
case "$1" in print) test -f "$HOME/loaded" ;; bootout) test -z "$FAIL_BOOTOUT" && /bin/rm -f "$HOME/loaded" ;; esac"#,
                ),
                ("systemctl", r#"echo "$*" >> "$HOME/calls""#),
            ] {
                let p = bin.join(name);
                fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
                fs::set_permissions(p, fs::Permissions::from_mode(0o755)).unwrap();
            }
            std::os::unix::fs::symlink(env::current_exe().unwrap(), bin.join("vega-post")).unwrap();
            let status = Command::new(env::current_exe().unwrap())
                .args([
                    "--exact",
                    "service::tests::old_login_jobs_are_removed_and_never_written",
                ])
                .env("VEGA_LOGIN_TEST", "1")
                .env("HOME", d.path())
                .env("XDG_DATA_HOME", d.path().join("data"))
                .env("XDG_CONFIG_HOME", d.path().join("config"))
                .env("XDG_STATE_HOME", d.path().join("state"))
                .env("PATH", &bin)
                .status()
                .unwrap();
            assert!(status.success());
            return;
        }
        let calls = || fs::read_to_string(home().unwrap().join("calls")).unwrap_or_default();
        // relay status names an old job of this OS's manager with the removal hint, and drops
        // the line once the job is gone.
        let ours = login_path(manager().expect("launchd or systemd")).unwrap();
        fs::create_dir_all(ours.parent().unwrap()).unwrap();
        fs::write(&ours, "old job").unwrap();
        let line = login_service_line(&status_value().unwrap()).expect("old job reported");
        assert!(line.starts_with("login_service: "), "{line}");
        assert!(
            line.ends_with("(old login job; `vega post relay install` removes it)"),
            "{line}"
        );
        fs::remove_file(&ours).unwrap();
        assert_eq!(login_service_line(&status_value().unwrap()), None);
        let mac = login_path(Manager::Launchd).unwrap();
        fs::create_dir_all(mac.parent().unwrap()).unwrap();
        fs::write(&mac, "old job").unwrap();
        fs::write(home().unwrap().join("loaded"), "").unwrap();
        assert_eq!(
            remove_login_for(Manager::Launchd).unwrap(),
            Some(mac.clone())
        );
        assert!(!mac.exists());
        assert!(calls().contains("bootout gui/123/vega.vegad-start"));
        assert_eq!(
            remove_login_for(Manager::Launchd).unwrap(),
            None,
            "idempotent"
        );
        let linux = login_path(Manager::Systemd).unwrap();
        fs::create_dir_all(linux.parent().unwrap()).unwrap();
        fs::write(&linux, "old unit").unwrap();
        assert_eq!(
            remove_login_for(Manager::Systemd).unwrap(),
            Some(linux.clone())
        );
        assert!(!linux.exists());
        assert!(calls().contains("--user disable --now vega-vegad-start.service"));
        // A job that cannot be unloaded only warns during install and stays for repair.
        fs::write(&mac, "old job").unwrap();
        fs::write(home().unwrap().join("loaded"), "").unwrap();
        // This subprocess is the only reader/writer of its environment.
        unsafe {
            env::set_var("FAIL_BOOTOUT", "yes");
        }
        let (line, warning) = login_removal_report(remove_login_for(Manager::Launchd));
        assert!(
            warning
                && line
                    .unwrap()
                    .starts_with("warning: old login job not removed")
        );
        assert!(mac.exists());
        let (line, _) = login_removal_report(Err(Failure::new("x", "boom", "run: fix it")));
        assert!(line.unwrap().ends_with("boom (run: fix it)"));
        let (line, warning) = login_removal_report(Ok(Some(mac.clone())));
        assert!(!warning && line.unwrap().contains("removed"));
        unsafe {
            env::remove_var("FAIL_BOOTOUT");
        }
        fs::remove_file(&mac).unwrap();
        // Real planning and hook selection still work on a herdr-less host, and write no job.
        let planned = plan().expect("planning must not require herdr");
        assert!(planned.env.contains(&("VEGA_POST_HOST".into(), host())));
        assert!(!mac.exists() && !linux.exists());
    }

    fn scratch(name: &str) -> PathBuf {
        let d = env::temp_dir().join(format!("vega-service-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn daemon_drops_terminal_context_but_keeps_tui_socket() {
        for key in [
            "HC_TERMINAL_ID",
            "HC_TERMINAL_SOCKET",
            "HC_SCOPE",
            "HC_SESSION",
            "HC_FUTURE",
            "HERDR_PANE_ID",
            "HERDR_PLUGIN_CONTEXT_JSON",
        ] {
            assert!(terminal_env(key), "{key}");
        }
        for key in ["PATH", "HERDR_SOCKET_PATH", "VEGA_POST_HOST"] {
            assert!(!terminal_env(key), "{key}");
        }
    }

    #[test]
    fn detached_relay_inherits_the_starters_resolved_host() {
        const TEST: &str = "VEGA_DETACHED_HOST_TEST";
        if env::var_os(TEST).is_none() {
            let out = Command::new(env::current_exe().unwrap())
                .args([
                    "--exact",
                    "service::tests::detached_relay_inherits_the_starters_resolved_host",
                    "--nocapture",
                ])
                .env(TEST, "1")
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{}{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        env::set_var("HOME", temp.path());
        env::remove_var("XDG_STATE_HOME");
        let state = state_dir().unwrap();
        fs::create_dir_all(&state).unwrap();
        fs::write(state.join("vegad.env"), "VEGA_POST_HOST=pinned-host\n").unwrap();
        let exe = temp.path().join("fake-relay");
        fs::write(
            &exe,
            "#!/bin/sh\nprintf '%s' \"$VEGA_POST_HOST\" > \"$HOME/child-host\"\n",
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&exe, fs::Permissions::from_mode(0o755)).unwrap();
        for (shell, expected) in [
            (None, "pinned-host"),
            (Some(""), "pinned-host"),
            (Some("shell-host"), "shell-host"),
        ] {
            match shell {
                Some(value) => env::set_var("VEGA_POST_HOST", value),
                None => env::remove_var("VEGA_POST_HOST"),
            }
            load_env_file();
            assert_eq!(host(), expected);
            assert!(spawn_detached(&exe).unwrap().wait().unwrap().success());
            assert_eq!(
                fs::read_to_string(temp.path().join("child-host")).unwrap(),
                expected
            );
            assert!(state.join(format!("post-relay-{expected}.log")).exists());
        }
    }

    #[test]
    fn upgrade_is_due_once_a_replaced_binary_is_stable() {
        let d = scratch("upgrade");
        let exe = d.join("vega-post");
        fs::write(&exe, "old").unwrap();
        let mut u = Upgrade::at(exe.clone(), Duration::ZERO);
        assert!(!u.due(), "unchanged");
        // Replace by rename, as cargo install does.
        let tmp = d.join("tmp");
        fs::write(&tmp, "new binary").unwrap();
        fs::rename(&tmp, &exe).unwrap();
        assert!(!u.due(), "just changed: maybe still being written");
        assert!(u.due(), "unchanged since");
        let mut slow = Upgrade::at(exe.clone(), Duration::from_secs(3600));
        fs::write(&tmp, "slow").unwrap();
        fs::rename(&tmp, &exe).unwrap();
        assert!(!slow.due() && !slow.due(), "not settled yet");
        fs::write(&tmp, "new binary").unwrap();
        fs::rename(&tmp, &exe).unwrap();
        u.due();
        assert!(u.due());
        u.failed();
        assert!(!u.due(), "a failed exec waits for the next change");
        fs::write(&tmp, "newer binary!").unwrap();
        fs::rename(&tmp, &exe).unwrap();
        assert!(!u.due());
        assert!(u.due(), "the next change counts again");
        fs::remove_file(&exe).unwrap();
        assert!(!u.due(), "a missing file is never due");
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn handoff_only_for_the_same_pid_and_fresh() {
        let v = json!({"pid": 42, "at_ms": 1_000, "state": {"x": 1}}).to_string();
        assert_eq!(handoff_for(&v, 42, 2_000), Some(json!({"x": 1})));
        assert_eq!(handoff_for(&v, 43, 2_000), None, "another process");
        assert_eq!(handoff_for(&v, 42, 1_000 + 61_000), None, "too old");
        assert_eq!(handoff_for("garbage", 42, 2_000), None);
    }

    #[test]
    fn env_file_keeps_only_pinned_keys() {
        let t = "# c\nVEGA_NATS_URL=nats://a:4222\nPATH=/evil\nHERDR_SOCKET_PATH=/s/herdr.sock\nbad line\n";
        assert_eq!(
            parse_env(t),
            vec![
                ("VEGA_NATS_URL".into(), "nats://a:4222".into()),
                ("HERDR_SOCKET_PATH".into(), "/s/herdr.sock".into()),
            ]
        );
    }

    #[test]
    fn daemon_path_drops_shims_and_dedupes() {
        let p = daemon_path_with(
            Path::new("/home/u/.local/bin/vega-post"),
            "/home/u/.local/share/vega/shims:/opt/x:/home/u/.local/bin:/usr/bin",
            Some("/opt/herdr/bin/herdr"),
            [None, Some("/home/u/.local/bin".into())],
        );
        assert!(p.starts_with("/home/u/.local/bin:/opt/herdr/bin:"), "{p}");
        assert!(!p.contains("shims"), "{p}");
        assert_eq!(p.matches("/home/u/.local/bin").count(), 1, "{p}");
        assert!(p.contains("/opt/x") && p.ends_with("/sbin"), "{p}");
    }

    #[test]
    fn daemon_path_keeps_mise_shims_but_drops_agent_shims() {
        let p = daemon_path_with(
            Path::new("/home/u/.local/bin/vega-post"),
            "/home/u/.local/share/vega/shims:/opt/mise/shims:/usr/bin",
            None,
            [Some("/opt/mise/shims".into()), None],
        );
        assert_eq!(
            p,
            "/home/u/.local/bin:/opt/mise/shims:/usr/bin:/bin:/usr/sbin:/sbin"
        );
    }

    #[test]
    fn manifest_starts_the_relay_with_an_absolute_path() {
        let m = manifest("/home/u/.local/bin/vega-post", None);
        assert!(m.contains("id = \"vega.vegad\""));
        assert!(m.contains(
            "[[startup]]\ncommand = [\"/home/u/.local/bin/vega-post\", \"relay\", \"start\"]"
        ));
    }

    #[test]
    fn container_name_parsed_from_containerenv() {
        let env = "engine=\"podman-5\"\nname=\"fedora-toolbox-44\"\nid=\"abc\"\n";
        assert_eq!(
            parse_container_name(env).as_deref(),
            Some("fedora-toolbox-44")
        );
        assert_eq!(parse_container_name("engine=\"x\"\n"), None);
    }

    #[test]
    fn manifest_runs_the_relay_in_the_toolbox_container() {
        let m = manifest("/home/u/.local/bin/vega-post", Some("tb"));
        assert!(m.contains(
            "command = [\"/usr/bin/toolbox\", \"run\", \"-c\", \"tb\", \"/home/u/.local/bin/vega-post\", \"relay\", \"start\"]"
        ));
    }

    #[test]
    fn binary_state_compares_the_file_now() {
        let d = scratch("binary");
        let exe = d.join("vega-post");
        fs::write(&exe, "a").unwrap();
        let id = exe_id(&exe).unwrap().0;
        let s = json!({"exe": exe.display().to_string(), "exe_id": id});
        assert_eq!(binary_state(Some(&s)), "current");
        let tmp = d.join("tmp");
        fs::write(&tmp, "bb").unwrap();
        fs::rename(&tmp, &exe).unwrap();
        assert_eq!(binary_state(Some(&s)), "stale");
        assert_eq!(binary_state(None), "unknown");
        let _ = fs::remove_dir_all(&d);
    }
}
