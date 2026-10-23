//! Secrets for vega tools and agents (docs/design/secrets.md).
//!
//! A chezmoi-managed manifest without values (`~/.config/vega/secrets.toml`),
//! one age ciphertext per secret (`~/.config/vega/secrets/<name>.age`) and one
//! age identity per host (`~/.config/vega/age/host.key`, never leaves the
//! host). Values are decrypted in process on demand and only ever held in a
//! [`Secret`], which does not print.

use std::collections::BTreeSet;
use std::fmt;
use std::fs;
use std::io::{Read, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::str::FromStr;
use std::time::{SystemTime, UNIX_EPOCH};

use age::secrecy::ExposeSecret;
use age::x25519;
use toml_edit::{Array, DocumentMut, Item, Table};
use zeroize::Zeroizing;

/// A structured failure (AXI 6). Never carries a value.
#[derive(Debug, Clone, PartialEq)]
pub struct Error {
    pub code: &'static str,
    pub message: String,
    pub hint: String,
}

impl Error {
    pub fn new(code: &'static str, message: impl Into<String>, hint: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            hint: hint.into(),
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {} ({})", self.code, self.message, self.hint)
    }
}

impl std::error::Error for Error {}

/// A decrypted value. `Debug` and `Display` print `***`; the memory is zeroed
/// on drop. Read it with [`Secret::expose`] and hand it on via env, never argv.
pub struct Secret(Zeroizing<String>);

impl Secret {
    pub fn new(value: String) -> Self {
        Self(Zeroizing::new(value))
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("***")
    }
}

impl fmt::Display for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("***")
    }
}

/// One `[secrets.<name>]` entry of the manifest.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Entry {
    pub name: String,
    pub env: String,
    pub tools: Vec<String>,
    pub hosts: Vec<String>,
    pub op: Option<String>,
    pub encrypted_to: Vec<String>,
    pub rotated: Option<String>,
}

/// Whether this host can use a secret, checked without decrypting.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Here {
    Ok,
    Missing,
    Undecryptable,
    NotForHost,
}

impl Here {
    pub fn label(self) -> &'static str {
        match self {
            Here::Ok => "ok",
            Here::Missing => "missing",
            Here::Undecryptable => "undecryptable",
            Here::NotForHost => "not_for_host",
        }
    }
}

/// The manifest, kept as a TOML document so edits keep comments and order.
#[derive(Debug, Clone, Default)]
pub struct Manifest {
    doc: DocumentMut,
}

fn strings(item: Option<&Item>) -> Vec<String> {
    item.and_then(Item::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

fn array(values: &[String]) -> Item {
    let mut a = Array::new();
    for v in values {
        a.push(v.as_str());
    }
    toml_edit::value(a)
}

impl FromStr for Manifest {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Error> {
        let doc = s.parse::<DocumentMut>().map_err(|e| {
            Error::new(
                "bad_manifest",
                format!("secrets.toml: {}", e.message()),
                "fix the manifest in the chezmoi source, then chezmoi apply",
            )
        })?;
        Ok(Self { doc })
    }
}

impl fmt::Display for Manifest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.doc)
    }
}

impl Manifest {
    pub fn admin(&self) -> Vec<String> {
        strings(self.doc.get("admin"))
    }

    pub fn set_admin(&mut self, hosts: &[String]) {
        self.doc["admin"] = array(hosts);
    }

    /// `(host, recipient)` from `[hosts.<host>]`, sorted by host.
    pub fn hosts(&self) -> Vec<(String, String)> {
        let Some(t) = self.doc.get("hosts").and_then(Item::as_table_like) else {
            return Vec::new();
        };
        let mut v: Vec<_> = t
            .iter()
            .filter_map(|(k, item)| {
                let r = item.as_table_like()?.get("recipient")?.as_str()?;
                Some((k.to_string(), r.to_string()))
            })
            .collect();
        v.sort();
        v
    }

    pub fn set_host(&mut self, host: &str, recipient: &str) {
        let hosts = implicit_table(&mut self.doc, "hosts");
        let mut t = Table::new();
        t["recipient"] = toml_edit::value(recipient);
        hosts[host] = Item::Table(t);
    }

    pub fn entries(&self) -> Vec<Entry> {
        let Some(t) = self.doc.get("secrets").and_then(Item::as_table_like) else {
            return Vec::new();
        };
        t.iter()
            .filter_map(|(name, item)| {
                let s = item.as_table_like()?;
                let text = |k: &str| s.get(k).and_then(Item::as_str).map(str::to_owned);
                Some(Entry {
                    name: name.to_string(),
                    env: text("env").unwrap_or_default(),
                    tools: strings(s.get("for")),
                    hosts: strings(s.get("hosts")),
                    op: text("op"),
                    encrypted_to: strings(s.get("encrypted_to")),
                    rotated: text("rotated"),
                })
            })
            .collect()
    }

    pub fn entry(&self, name: &str) -> Option<Entry> {
        self.entries().into_iter().find(|e| e.name == name)
    }

    /// Writes an entry, keeping the order of an existing one.
    pub fn set_entry(&mut self, e: &Entry) {
        let secrets = implicit_table(&mut self.doc, "secrets");
        if !secrets.contains_key(&e.name) {
            secrets[e.name.as_str()] = Item::Table(Table::new());
        }
        let t = secrets[e.name.as_str()]
            .as_table_mut()
            .expect("secrets entries are tables");
        t["env"] = toml_edit::value(&e.env);
        t["for"] = array(&e.tools);
        t["hosts"] = array(&e.hosts);
        match &e.op {
            Some(op) => t["op"] = toml_edit::value(op),
            None => {
                t.remove("op");
            }
        }
        t["encrypted_to"] = array(&e.encrypted_to);
        match &e.rotated {
            Some(r) => t["rotated"] = toml_edit::value(r),
            None => {
                t.remove("rotated");
            }
        }
    }

    /// The hosts a secret is encrypted to: its hosts plus every admin host.
    pub fn recipients_of(&self, e: &Entry) -> Vec<String> {
        let set: BTreeSet<String> = e.hosts.iter().chain(&self.admin()).cloned().collect();
        set.into_iter().collect()
    }
}

fn implicit_table<'a>(doc: &'a mut DocumentMut, key: &str) -> &'a mut Table {
    if !doc.contains_table(key) {
        let mut t = Table::new();
        t.set_implicit(true);
        doc[key] = Item::Table(t);
    }
    doc[key].as_table_mut().expect("checked above")
}

/// Secret names become file names: lowercase letters, digits and hyphens.
pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && !name.starts_with('-')
}

/// An age X25519 public key (`age1...`).
pub fn valid_recipient(s: &str) -> bool {
    x25519::Recipient::from_str(s).is_ok()
}

/// `(env var, value)` pairs to inject, and the secrets skipped with why.
pub type EnvFor = (Vec<(String, Secret)>, Vec<Error>);

/// Where this host keeps the manifest, ciphertexts, its identity and the
/// audit log, and which host it is.
#[derive(Debug, Clone)]
pub struct Store {
    /// `~/.config/vega`
    pub config: PathBuf,
    /// `~/.local/state/vega/secrets`
    pub state: PathBuf,
    pub host: String,
}

impl Store {
    /// The store of this user on this host (XDG dirs, `hostname -s`).
    pub fn from_env() -> Result<Self, Error> {
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let xdg = |var: &str, rel: &str| {
            std::env::var_os(var)
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
                .or_else(|| home.as_ref().map(|h| h.join(rel)))
        };
        let config = xdg("XDG_CONFIG_HOME", ".config")
            .ok_or_else(|| Error::new("no_home", "HOME is not set", "set HOME"))?;
        let state = xdg("XDG_STATE_HOME", ".local/state")
            .ok_or_else(|| Error::new("no_home", "HOME is not set", "set HOME"))?;
        Ok(Self {
            config: config.join("vega"),
            state: state.join("vega").join("secrets"),
            host: hostname(),
        })
    }

    pub fn manifest_path(&self) -> PathBuf {
        self.config.join("secrets.toml")
    }

    pub fn secret_path(&self, name: &str) -> PathBuf {
        self.config.join("secrets").join(format!("{name}.age"))
    }

    pub fn key_path(&self) -> PathBuf {
        self.config.join("age").join("host.key")
    }

    pub fn audit_path(&self) -> PathBuf {
        self.state.join("audit.log")
    }

    /// The manifest; an absent file is an empty manifest.
    pub fn manifest(&self) -> Result<Manifest, Error> {
        match fs::read_to_string(self.manifest_path()) {
            Ok(s) => s.parse(),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Manifest::default()),
            Err(e) => Err(Error::new(
                "manifest_unreadable",
                format!("{}: {e}", self.manifest_path().display()),
                "check the file's permissions",
            )),
        }
    }

    pub fn save_manifest(&self, m: &Manifest) -> Result<(), Error> {
        write_atomic(&self.manifest_path(), m.to_string().as_bytes(), 0o644)
    }

    pub fn is_admin(&self, m: &Manifest) -> bool {
        m.admin().contains(&self.host)
    }

    /// This host's identity.
    pub fn identity(&self) -> Result<x25519::Identity, Error> {
        let path = self.key_path();
        let text = Zeroizing::new(fs::read_to_string(&path).map_err(|_| {
            Error::new(
                "no_identity",
                format!("no host identity at {}", path.display()),
                "vega secrets host init",
            )
        })?);
        text.lines()
            .map(str::trim)
            .find(|l| l.starts_with("AGE-SECRET-KEY-"))
            .and_then(|l| x25519::Identity::from_str(l).ok())
            .ok_or_else(|| {
                Error::new(
                    "bad_identity",
                    format!("{} holds no age X25519 identity", path.display()),
                    "restore it from a backup or run vega secrets host init on a fresh host",
                )
            })
    }

    /// Creates this host's identity unless it exists; returns its public key
    /// and whether it was created now.
    pub fn host_init(&self) -> Result<(String, bool), Error> {
        if self.key_path().exists() {
            return Ok((self.identity()?.to_public().to_string(), false));
        }
        let id = x25519::Identity::generate();
        let public = id.to_public().to_string();
        let body = Zeroizing::new(format!(
            "# vega secrets host identity for {}; never copy it to another host\n# public key: {public}\n{}\n",
            self.host,
            id.to_string().expose_secret()
        ));
        let dir = self.key_path().parent().expect("has parent").to_path_buf();
        fs::create_dir_all(&dir).map_err(|e| io_error(&dir, e))?;
        let _ = fs::set_permissions(&dir, fs::Permissions::from_mode(0o700));
        let mut f = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(self.key_path())
            .map_err(|e| io_error(&self.key_path(), e))?;
        f.write_all(body.as_bytes())
            .map_err(|e| io_error(&self.key_path(), e))?;
        Ok((public, true))
    }

    /// The state of a secret on this host, without decrypting.
    pub fn here(&self, m: &Manifest, e: &Entry) -> Here {
        if !m.recipients_of(e).contains(&self.host) {
            Here::NotForHost
        } else if !self.secret_path(&e.name).exists() {
            Here::Missing
        } else if !e.encrypted_to.contains(&self.host) {
            Here::Undecryptable
        } else {
            Here::Ok
        }
    }

    /// Encrypts `value` to the secret's recipients, writes the ciphertext and
    /// returns the recipients (for `encrypted_to`).
    pub fn encrypt(&self, m: &Manifest, e: &Entry, value: &Secret) -> Result<Vec<String>, Error> {
        let hosts = m.recipients_of(e);
        let known = m.hosts();
        let mut recipients = Vec::new();
        for h in &hosts {
            let r = known
                .iter()
                .find(|(k, _)| k == h)
                .map(|(_, r)| r)
                .ok_or_else(|| {
                    Error::new(
                        "unknown_host",
                        format!("host {h} has no [hosts.{h}] recipient in the manifest"),
                        format!("run vega secrets host init on {h}, then vega secrets host add {h} <age1...> here"),
                    )
                })?;
            let r = x25519::Recipient::from_str(r).map_err(|_| {
                Error::new(
                    "bad_recipient",
                    format!("[hosts.{h}] recipient is not an age X25519 public key"),
                    format!("vega secrets host add {h} <age1...>"),
                )
            })?;
            recipients.push(r);
        }
        if recipients.is_empty() {
            return Err(Error::new(
                "no_recipients",
                format!("{} has no hosts and the manifest no admin", e.name),
                "pass --hosts, or set admin in the manifest",
            ));
        }
        let encryptor =
            age::Encryptor::with_recipients(recipients.iter().map(|r| r as &dyn age::Recipient))
                .map_err(|e| Error::new("encrypt_failed", e.to_string(), "report this as a bug"))?;
        let mut out = Vec::new();
        let mut w = encryptor
            .wrap_output(&mut out)
            .map_err(|e| Error::new("encrypt_failed", e.to_string(), "report this as a bug"))?;
        w.write_all(value.expose().as_bytes())
            .and_then(|_| w.finish().map(|_| ()))
            .map_err(|e| Error::new("encrypt_failed", e.to_string(), "report this as a bug"))?;
        write_atomic(&self.secret_path(&e.name), &out, 0o600)?;
        Ok(hosts)
    }

    /// Decrypts a secret with this host's identity. No scope check: callers
    /// outside this crate use [`Store::get`] or [`Store::env_for`].
    pub fn decrypt(&self, name: &str) -> Result<Secret, Error> {
        let path = self.secret_path(name);
        let data = fs::read(&path).map_err(|_| {
            Error::new(
                "missing",
                format!("{name}: no ciphertext at {}", path.display()),
                format!("on the admin host: vega secrets set {name}; here: chezmoi update"),
            )
        })?;
        let id = self.identity()?;
        let undecryptable = || {
            Error::new(
                "undecryptable",
                format!("{name} is not encrypted to {}", self.host),
                "on the admin host: vega secrets rekey, commit, push; here: chezmoi update",
            )
        };
        let d = age::Decryptor::new(&data[..]).map_err(|_| undecryptable())?;
        let mut r = d
            .decrypt(std::iter::once(&id as &dyn age::Identity))
            .map_err(|_| undecryptable())?;
        let mut buf = Zeroizing::new(Vec::new());
        r.read_to_end(&mut buf).map_err(|_| undecryptable())?;
        let s = String::from_utf8(buf.to_vec()).map_err(|_| {
            Error::new(
                "not_utf8",
                format!("{name} is not UTF-8 text"),
                format!("vega secrets set {name}"),
            )
        })?;
        Ok(Secret::new(s))
    }

    /// The scoped lookup for vega plugins: `get("titan-dolt", "bd")`.
    pub fn get(&self, name: &str, tool: &str) -> Result<Secret, Error> {
        let m = self.manifest()?;
        let e = m.entry(name).ok_or_else(|| {
            Error::new(
                "unknown_secret",
                format!("no secret {name} in the manifest"),
                format!("ask the Slayer to run vega secrets set {name} on the admin host"),
            )
        })?;
        if !e.tools.iter().any(|t| t == tool) {
            return Err(Error::new(
                "not_for_tool",
                format!("{name} is not for {tool}"),
                format!("ask the Slayer to add {tool} to the secret's `for` (vega secrets set {name} --for ...)"),
            ));
        }
        if self.here(&m, &e) == Here::NotForHost {
            return Err(Error::new(
                "not_for_host",
                format!("{name} is not for host {}", self.host),
                format!("ask the Slayer to add {} to the secret's hosts", self.host),
            ));
        }
        let s = self.decrypt(name)?;
        self.audit(name, tool);
        Ok(s)
    }

    /// `(env var, value)` of every secret for `tool` this host can decrypt,
    /// plus one error per secret for `tool` it cannot. `not_for_tool` if no
    /// secret lists the tool.
    pub fn env_for(&self, tool: &str) -> Result<EnvFor, Error> {
        let m = self.manifest()?;
        let mine: Vec<Entry> = m
            .entries()
            .into_iter()
            .filter(|e| e.tools.iter().any(|t| t == tool))
            .collect();
        if mine.is_empty() {
            return Err(Error::new(
                "not_for_tool",
                format!("no secret is for {tool}"),
                "vega secrets   # lists each secret's tools",
            ));
        }
        let (mut env, mut skipped) = (Vec::new(), Vec::new());
        for e in mine {
            match self.get(&e.name, tool) {
                Ok(s) => env.push((e.env.clone(), s)),
                Err(err) => skipped.push(err),
            }
        }
        Ok((env, skipped))
    }

    /// Appends `time,secret,tool,caller` to the audit log; best effort.
    pub fn audit(&self, name: &str, tool: &str) {
        let caller = ["CLAUDE_CODE_SESSION_ID", "HERDR_PANE_ID"]
            .iter()
            .find_map(|v| std::env::var(v).ok().filter(|s| !s.is_empty()))
            .unwrap_or_else(|| format!("pid {}", std::process::id()));
        let line = format!("{},{name},{tool},{caller}\n", now_rfc3339());
        let _ = fs::create_dir_all(&self.state);
        if let Ok(mut f) = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .open(self.audit_path())
        {
            let _ = f.write_all(line.as_bytes());
        }
    }
}

fn io_error(path: &Path, e: std::io::Error) -> Error {
    Error::new(
        "io",
        format!("{}: {e}", path.display()),
        "check the directory and its permissions",
    )
}

/// Writes via a temp file in the same dir and a rename.
fn write_atomic(path: &Path, data: &[u8], mode: u32) -> Result<(), Error> {
    let dir = path.parent().expect("has parent");
    fs::create_dir_all(dir).map_err(|e| io_error(dir, e))?;
    let tmp = dir.join(format!(
        ".{}.tmp{}",
        path.file_name().unwrap_or_default().to_string_lossy(),
        std::process::id()
    ));
    let res = (|| {
        let mut f = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(mode)
            .open(&tmp)?;
        f.write_all(data)?;
        f.sync_all()?;
        fs::rename(&tmp, path)
    })();
    res.map_err(|e| {
        let _ = fs::remove_file(&tmp);
        io_error(path, e)
    })
}

/// `hostname -s`, lowercase (as `vega uac claim` and the bd shim use it).
pub fn hostname() -> String {
    Command::new("hostname")
        .arg("-s")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_lowercase())
        .filter(|h| !h.is_empty())
        .unwrap_or_else(|| "unknown".into())
}

fn civil(unix: i64) -> (i64, i64, i64, i64, i64, i64) {
    let (days, secs) = (unix.div_euclid(86400), unix.rem_euclid(86400));
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    (y, m, d, secs / 3600, secs % 3600 / 60, secs % 60)
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Today in UTC, `YYYY-MM-DD` (the manifest's `rotated`).
pub fn today() -> String {
    let (y, m, d, ..) = civil(unix_now());
    format!("{y:04}-{m:02}-{d:02}")
}

fn now_rfc3339() -> String {
    let (y, m, d, h, mi, s) = civil(unix_now());
    format!("{y:04}-{m:02}-{d:02}T{h:02}:{mi:02}:{s:02}Z")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store(dir: &Path, host: &str) -> Store {
        Store {
            config: dir.join("config"),
            state: dir.join("state"),
            host: host.into(),
        }
    }

    /// Two hosts sharing one config dir stand in for chezmoi: each has its
    /// own identity file.
    fn setup(dir: &Path) -> (Store, Store, Manifest) {
        let p = store(dir, "proteus");
        let (pk, created) = p.host_init().unwrap();
        assert!(created);
        let g = Store {
            config: dir.join("g"),
            ..store(dir, "ganymede")
        };
        let (gk, _) = g.host_init().unwrap();
        let mut m = Manifest::default();
        m.set_admin(&["proteus".into()]);
        m.set_host("proteus", &pk);
        m.set_host("ganymede", &gk);
        (p, g, m)
    }

    fn entry(name: &str, tools: &[&str], hosts: &[&str]) -> Entry {
        Entry {
            name: name.into(),
            env: "AA_KEY".into(),
            tools: tools.iter().map(|s| s.to_string()).collect(),
            hosts: hosts.iter().map(|s| s.to_string()).collect(),
            ..Entry::default()
        }
    }

    #[test]
    fn secret_never_prints_its_value() {
        let s = Secret::new("hunter2".into());
        assert_eq!(format!("{s} {s:?}"), "*** ***");
        assert_eq!(s.expose(), "hunter2");
    }

    #[test]
    fn host_key_is_private_and_kept() {
        let dir = tempfile::tempdir().unwrap();
        let p = store(dir.path(), "proteus");
        let (k1, created) = p.host_init().unwrap();
        assert!(created && k1.starts_with("age1"));
        let mode = fs::metadata(p.key_path()).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        let (k2, created) = p.host_init().unwrap();
        assert!(!created);
        assert_eq!(k1, k2);
    }

    #[test]
    fn round_trip_with_scopes_and_audit() {
        let dir = tempfile::tempdir().unwrap();
        let (p, _, mut m) = setup(dir.path());
        let mut e = entry("aa", &["lanista"], &["proteus"]);
        e.encrypted_to = p.encrypt(&m, &e, &Secret::new("v1".into())).unwrap();
        m.set_entry(&e);
        p.save_manifest(&m).unwrap();

        assert_eq!(p.get("aa", "lanista").unwrap().expose(), "v1");
        assert_eq!(p.get("aa", "bd").unwrap_err().code, "not_for_tool");
        assert_eq!(p.get("nope", "lanista").unwrap_err().code, "unknown_secret");
        let (env, skipped) = p.env_for("lanista").unwrap();
        assert_eq!(env.len(), 1);
        assert_eq!(env[0].0, "AA_KEY");
        assert!(skipped.is_empty());
        assert_eq!(p.env_for("gh").unwrap_err().code, "not_for_tool");

        let log = fs::read_to_string(p.audit_path()).unwrap();
        assert!(log.lines().all(|l| l.contains(",aa,lanista,")));
        assert!(!log.contains("v1"));
        let mode = fs::metadata(p.secret_path("aa"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn hosts_see_only_their_secrets() {
        let dir = tempfile::tempdir().unwrap();
        let (p, g, mut m) = setup(dir.path());
        // proteus only (admin); ganymede is not a recipient.
        let mut e = entry("aa", &["lanista"], &[]);
        e.encrypted_to = p.encrypt(&m, &e, &Secret::new("v1".into())).unwrap();
        assert_eq!(e.encrypted_to, ["proteus"]);
        m.set_entry(&e);
        assert_eq!(g.here(&m, &e), Here::NotForHost);
        assert_eq!(p.here(&m, &e), Here::Ok);

        // ganymede added to hosts but not yet rekeyed: undecryptable there.
        e.hosts = vec!["ganymede".into()];
        m.set_entry(&e);
        fs::create_dir_all(g.config.join("secrets")).unwrap();
        fs::copy(p.secret_path("aa"), g.secret_path("aa")).unwrap();
        assert_eq!(g.here(&m, &e), Here::Undecryptable);
        assert_eq!(g.decrypt("aa").unwrap_err().code, "undecryptable");

        // rekey: admin decrypts and re-encrypts to both.
        let v = p.decrypt("aa").unwrap();
        e.encrypted_to = p.encrypt(&m, &e, &v).unwrap();
        assert_eq!(e.encrypted_to, ["ganymede", "proteus"]);
        fs::copy(p.secret_path("aa"), g.secret_path("aa")).unwrap();
        assert_eq!(g.decrypt("aa").unwrap().expose(), "v1");
    }

    #[test]
    fn encrypt_needs_known_hosts() {
        let dir = tempfile::tempdir().unwrap();
        let (p, _, m) = setup(dir.path());
        let e = entry("aa", &["lanista"], &["titan"]);
        let err = p.encrypt(&m, &e, &Secret::new("v".into())).unwrap_err();
        assert_eq!(err.code, "unknown_host");
        assert!(!p.secret_path("aa").exists());
    }

    #[test]
    fn manifest_edits_keep_comments_and_round_trip() {
        let src = "# vega secrets\nadmin = [\"proteus\"]\n\n[hosts.proteus]\nrecipient = \"age1x\"\n\n[secrets.aa]\n# Artificial Analysis\nenv = \"AA\"\nfor = [\"lanista\"]\nhosts = [\"proteus\"]\nop = \"op://Private/AA/credential\"\n";
        let mut m: Manifest = src.parse().unwrap();
        assert_eq!(m.admin(), ["proteus"]);
        assert_eq!(m.hosts(), [("proteus".into(), "age1x".into())]);
        let mut e = m.entry("aa").unwrap();
        assert_eq!(e.op.as_deref(), Some("op://Private/AA/credential"));
        e.rotated = Some("2026-10-11".into());
        e.encrypted_to = vec!["proteus".into()];
        m.set_entry(&e);
        m.set_entry(&entry("titan-dolt", &["bd"], &["proteus"]));
        let out = m.to_string();
        assert!(out.starts_with("# vega secrets\n"));
        assert!(out.contains("# Artificial Analysis"));
        assert!(out.contains("[secrets.titan-dolt]"));
        let back: Manifest = out.parse().unwrap();
        assert_eq!(back.entry("aa").unwrap(), e);
        assert_eq!(back.entries().len(), 2);
    }

    #[test]
    fn names_are_file_safe() {
        assert!(valid_name("titan-dolt"));
        assert!(!valid_name("../x"));
        assert!(!valid_name("A"));
        assert!(!valid_name(""));
    }

    #[test]
    fn civil_dates() {
        assert_eq!(civil(0), (1970, 1, 1, 0, 0, 0));
        assert_eq!(civil(1_791_712_800), (2026, 10, 11, 10, 0, 0));
    }
}
