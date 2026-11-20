//! agent-browser 0.43's Unix daemon namespace and close protocol.
use std::env;
use std::fs;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::unix::fs::{FileTypeExt, MetadataExt};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::Duration;

use serde_json::Value;
use sha2::{Digest, Sha256};

pub(crate) fn configs() -> impl Iterator<Item = Value> {
    let defaults = [
        vega_herdr::platform::home().map(|home| home.join(".agent-browser/config.json")),
        Some(PathBuf::from("agent-browser.json")),
    ];
    let paths = env::var_os("AGENT_BROWSER_CONFIG")
        .map_or(defaults, |path| [Some(PathBuf::from(path)), None]);
    paths
        .into_iter()
        .flatten()
        .filter_map(|path| serde_json::from_slice(&fs::read(path).ok()?).ok())
}

fn socket_dir() -> PathBuf {
    let var = |key| env::var(key).ok().filter(|s| !s.is_empty());
    let base = var("AGENT_BROWSER_SOCKET_DIR")
        .map(PathBuf::from)
        .or_else(|| var("XDG_RUNTIME_DIR").map(|s| PathBuf::from(s).join("agent-browser")))
        .or_else(|| vega_herdr::platform::home().map(|home| home.join(".agent-browser")))
        .unwrap_or_else(|| env::temp_dir().join("agent-browser"));
    let namespace = var("AGENT_BROWSER_NAMESPACE").or_else(|| {
        configs()
            .filter_map(|config| config["namespace"].as_str().map(str::to_owned))
            .last()
    });
    let mut normalized = String::new();
    for c in namespace.unwrap_or_default().chars() {
        if c.is_alphanumeric() {
            normalized.extend(c.to_lowercase());
        } else if !normalized.is_empty() && !normalized.ends_with(['-', '_']) {
            normalized.push(if c == '_' { '_' } else { '-' });
        }
    }
    let namespace = normalized.trim_end_matches(['-', '_']);
    if namespace.is_empty() {
        base
    } else {
        base.join("namespaces").join(namespace).join("run")
    }
}

pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '_' | '-'))
}

pub fn has_binding(session: &str) -> bool {
    if !valid_name(session) {
        return false;
    }
    let dir = socket_dir();
    ["sock", "pid", "target"]
        .iter()
        .any(|ext| dir.join(format!("{session}.{ext}")).exists())
}

/// Upstream `session list` prunes stale files. Cleanup inventory only reads
/// the socket namespace so a dry run leaves those files intact.
pub fn socket_names() -> io::Result<Vec<String>> {
    let entries = match fs::read_dir(socket_dir()) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    let mut names = Vec::new();
    for entry in entries {
        let entry = entry?;
        if let Some(name) = entry
            .file_name()
            .to_str()
            .and_then(|s| s.strip_suffix(".sock"))
        {
            if valid_name(name) && entry.file_type()?.is_socket() {
                names.push(name.to_owned());
            }
        }
    }
    names.sort_unstable();
    Ok(names)
}

pub struct Daemon {
    path: PathBuf,
    identity: String,
}

impl Daemon {
    pub fn find(session: &str) -> io::Result<Self> {
        if !valid_name(session) {
            return Err(io::Error::other("invalid session name"));
        }
        let path = socket_dir().canonicalize()?.join(format!("{session}.sock"));
        let meta = fs::symlink_metadata(&path)?;
        if !meta.file_type().is_socket() {
            return Err(io::Error::other("session path is not a Unix socket"));
        }
        let identity = format!(
            "{:x}",
            Sha256::digest(format!(
                "{}:{}:{}:{}:{}",
                path.display(),
                meta.dev(),
                meta.ino(),
                meta.ctime(),
                meta.ctime_nsec()
            ))
        );
        Ok(Self { path, identity })
    }

    pub fn identity(&self) -> &str {
        &self.identity
    }

    /// Send close on the very connection verified before acting. The upstream
    /// CLI can instead start a replacement daemon if the old one disappears.
    pub fn close(&self, session: &str) -> io::Result<()> {
        let mut stream = UnixStream::connect(&self.path)?;
        if Self::find(session)?.identity != self.identity {
            return Err(io::Error::other("daemon identity changed"));
        }
        let timeout = Some(Duration::from_secs(10));
        stream.set_read_timeout(timeout)?;
        stream.set_write_timeout(timeout)?;
        stream.write_all(b"{\"id\":\"vega-disconnect\",\"action\":\"close\"}\n")?;
        let mut line = String::new();
        BufReader::new(stream.take(65536)).read_line(&mut line)?;
        let response: Value = serde_json::from_str(&line)?;
        if response["id"] != "vega-disconnect"
            || response["success"] != true
            || response["data"]["closed"] != true
        {
            return Err(io::Error::other("daemon did not confirm close"));
        }
        Ok(())
    }
}
