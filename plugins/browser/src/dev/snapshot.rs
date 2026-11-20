mod storage;

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, DirBuilder, File, Metadata, OpenOptions};
use std::io;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use url::Url;
use vega_herdr::{platform, project, Failure};

use super::{failure, passed, process, save, stale, usage, Browser, Cdp, Project, Registry};

pub(super) enum Action {
    Register(String),
    Save,
    Restore,
}

impl Action {
    pub(super) fn parse(args: &[String]) -> Result<Self, Failure> {
        let args: Vec<_> = args.iter().map(String::as_str).collect();
        match args.as_slice() {
            ["register", origin] => Ok(Self::Register(
                parse_origin(origin)?.origin().ascii_serialization(),
            )),
            ["save"] => Ok(Self::Save),
            ["restore"] => Ok(Self::Restore),
            _ => Err(usage("Use dev state register <origin>, save, or restore")),
        }
    }
}

fn parse_origin(raw: &str) -> Result<Url, Failure> {
    let url = Url::parse(raw)
        .map_err(|_| usage("Register an HTTP(S) origin such as http://localhost:3000"))?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || url.username() != ""
        || url.password().is_some()
        || url.path() != "/"
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(usage(
            "Use an HTTP(S) origin without credentials, path, query, or fragment",
        ));
    }
    Ok(url)
}

const FORMAT: u32 = 1;
const SCOPE: &str = "host-only-cookies+first-party-localStorage";

type LocalStorage = Vec<(String, String)>;

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Snapshot {
    version: u32,
    scope: String,
    project: project::ProjectId,
    origins: BTreeMap<String, LocalStorage>,
    cookies: Vec<Cookie>,
}

impl Snapshot {
    fn validate(&self, id: &project::ProjectId) -> Result<(), Failure> {
        if self.version != FORMAT
            || self.scope != SCOPE
            || &self.project != id
            || self.origins.is_empty()
        {
            return Err(failure(
                "Incompatible dev snapshot version, project, or scope",
            ));
        }
        for (origin, entries) in &self.origins {
            if parse_origin(origin)?.origin().ascii_serialization() != *origin {
                return Err(failure("Dev snapshot contains a noncanonical origin"));
            }
            let mut keys = BTreeSet::new();
            if entries.iter().any(|(key, _)| !keys.insert(key)) {
                return Err(failure("Dev snapshot contains duplicate localStorage keys"));
            }
        }
        let mut keys = BTreeSet::new();
        for cookie in &self.cookies {
            if !self.origins.contains_key(&cookie.origin)
                || !keys.insert((
                    parse_origin(&cookie.origin)?.host_str().map(str::to_owned),
                    &cookie.path,
                    &cookie.name,
                ))
            {
                return Err(failure(
                    "Dev snapshot contains an out-of-scope or duplicate cookie",
                ));
            }
            cookie.validate()?;
        }
        Ok(())
    }
}

#[derive(Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct Cookie {
    origin: String,
    name: String,
    value: String,
    path: String,
    secure: bool,
    http_only: bool,
    same_site: Option<String>,
    expires: Option<f64>,
    priority: String,
    source_scheme: String,
    source_port: i32,
}

impl Cookie {
    fn validate(&self) -> Result<(), Failure> {
        if !self.path.starts_with('/')
            || !matches!(
                self.same_site.as_deref(),
                None | Some("Strict" | "Lax" | "None")
            )
            || !matches!(self.priority.as_str(), "Low" | "Medium" | "High")
            || !matches!(
                self.source_scheme.as_str(),
                "Unset" | "Secure" | "NonSecure"
            )
            || !matches!(self.source_port, -1 | 1..=65535)
            || self
                .expires
                .is_some_and(|expires| !expires.is_finite() || expires <= 0.0)
        {
            return Err(failure(
                "Dev snapshot contains unsupported cookie attributes",
            ));
        }
        Ok(())
    }

    fn params(&self) -> Result<Value, Failure> {
        let mut cookie =
            serde_json::to_value(self).map_err(|_| failure("Could not encode dev cookie"))?;
        let fields = cookie.as_object_mut().expect("cookie object");
        fields.remove("origin");
        fields.retain(|_, value| !value.is_null());
        // An undotted domain in CDP preserves host-only scope. Supplying
        // the origin URL would force Secure for HTTPS and constrain sourcePort.
        fields.insert(
            "domain".into(),
            json!(parse_origin(&self.origin)?.host_str()),
        );
        Ok(cookie)
    }
}

fn capture_cookies(cdp: &mut Cdp, project: &Project) -> Result<Vec<Cookie>, Failure> {
    let origins: Vec<_> = project
        .origins
        .iter()
        .map(|origin| parse_origin(origin))
        .collect::<Result<_, _>>()?;
    let result = cdp
        .call(
            "Storage.getCookies",
            json!({"browserContextId": project.context}),
        )
        .map_err(|_| failure("Could not read managed dev cookies"))?;
    let mut supported = Vec::new();
    for raw in result["cookies"]
        .as_array()
        .ok_or_else(|| failure("Chrome returned no dev cookie list"))?
    {
        let domain = raw["domain"]
            .as_str()
            .ok_or_else(|| failure("Chrome returned a cookie without a domain"))?;
        let Some(origin) = origins.iter().find(|origin| {
            origin
                .host_str()
                .is_some_and(|host| match domain.strip_prefix('.') {
                    Some(parent) => {
                        host == parent
                            || host
                                .strip_suffix(parent)
                                .is_some_and(|prefix| prefix.ends_with('.'))
                    }
                    None => host == domain,
                })
        }) else {
            continue;
        };
        let unsupported = || {
            failure("A cookie for a registered origin has unsupported domain, partition, source, or attribute metadata")
        };
        // Cookie visibility is host-scoped. CDP's sourcePort is metadata,
        // and Chrome may report 80 even for a cookie set on a loopback high port.
        let mut fields = raw.as_object().ok_or_else(unsupported)?.clone();
        if domain.starts_with('.') || fields.remove("sameParty").is_some_and(|v| v != false) {
            return Err(unsupported());
        }
        fields.remove("size");
        fields.remove("domain");
        match fields.remove("session").and_then(|v| v.as_bool()) {
            Some(true) if fields["expires"] == -1 => {
                fields.remove("expires");
            }
            Some(false) => {}
            _ => return Err(unsupported()),
        }
        fields.insert(
            "origin".into(),
            json!(origin.origin().ascii_serialization()),
        );
        let cookie: Cookie =
            serde_json::from_value(Value::Object(fields)).map_err(|_| unsupported())?;
        cookie.validate()?;
        supported.push(cookie);
    }
    Ok(supported)
}

fn owner_only(meta: &Metadata, mode: u32, directory: bool) -> Result<(), Failure> {
    // SAFETY: geteuid has no preconditions.
    if meta.uid() != unsafe { libc::geteuid() }
        || meta.mode() & 0o7777 != mode
        || if directory {
            !meta.is_dir()
        } else {
            !meta.is_file() || meta.nlink() != 1
        }
    {
        return Err(failure(
            "Dev state requires owner-only directories and regular owner-only files",
        ));
    }
    Ok(())
}

fn snapshot_path(id: &project::ProjectId) -> Result<PathBuf, Failure> {
    let base = platform::state_home()
        .ok_or_else(|| failure("Could not resolve the dev state directory"))?;
    let mut dir = base;
    DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&dir)
        .map_err(failure)?;
    trusted_parent(&dir)?;
    for component in ["vega", "browser", "dev-state"] {
        dir.push(component);
        match DirBuilder::new().mode(0o700).create(&dir) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(failure(error)),
        }
        let metadata = fs::symlink_metadata(&dir).map_err(failure)?;
        if component == "dev-state" {
            owner_only(&metadata, 0o700, true)?;
        } else {
            trusted_parent(&dir)?;
        }
    }
    Ok(dir.join(format!("{}.json", URL_SAFE_NO_PAD.encode(id.as_str()))))
}

fn trusted_parent(path: &Path) -> Result<(), Failure> {
    let metadata = fs::symlink_metadata(path).map_err(failure)?;
    // SAFETY: geteuid has no preconditions.
    if !metadata.is_dir()
        || metadata.mode() & 0o022 != 0
        || metadata.uid() != unsafe { libc::geteuid() }
    {
        return Err(failure(
            "Dev state parent directories must be owned by the current user and cannot be symlinks",
        ));
    }
    Ok(())
}

fn existing_file(path: &Path) -> Result<(), Failure> {
    match fs::symlink_metadata(path) {
        Ok(meta) => owner_only(&meta, 0o600, false),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(failure(error)),
    }
}

fn write_snapshot(path: &Path, snapshot: &Snapshot) -> Result<(), Failure> {
    existing_file(path)?;
    let dir = path.parent().expect("snapshot directory");
    let file = tempfile::NamedTempFile::new_in(dir).map_err(failure)?;
    owner_only(&file.as_file().metadata().map_err(failure)?, 0o600, false)?;
    serde_json::to_writer(file.as_file(), snapshot)
        .map_err(|_| failure("Could not write dev snapshot"))?;
    file.as_file().sync_all().map_err(failure)?;
    existing_file(path)?;
    file.persist(path).map_err(|error| failure(error.error))?;
    File::open(dir)
        .and_then(|dir| dir.sync_all())
        .map_err(failure)
}

fn read_snapshot(path: &Path) -> Result<Snapshot, Failure> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .map_err(|_| failure("Could not open the project dev snapshot"))?;
    owner_only(&file.metadata().map_err(failure)?, 0o600, false)?;
    serde_json::from_reader(file).map_err(|_| failure("Invalid dev snapshot"))
}

fn running(data: &Registry, profile: &Path) -> Result<Option<Browser>, Failure> {
    let Some(browser) = &data.browser else {
        return Ok(None);
    };
    if !process::running(browser, profile)? {
        return Ok(None);
    }
    let executable = crate::install::chrome_binary(&platform::home().unwrap_or_default())
        .and_then(|path| path.canonicalize().ok());
    if executable.as_ref() != Some(&browser.executable) {
        return Err(stale(
            "The managed process is not the installed Chrome for Testing",
        ));
    }
    Ok(Some(browser.clone()))
}

fn contexts(cdp: &mut Cdp) -> Result<Vec<Value>, Failure> {
    cdp.call("Target.getBrowserContexts", json!({}))?["browserContextIds"]
        .as_array()
        .cloned()
        .ok_or_else(|| failure("Chrome returned no managed context list"))
}

fn verify_context(data: &Registry, id: &project::ProjectId, cdp: &mut Cdp) -> Result<(), Failure> {
    let project = data
        .projects
        .get(id)
        .ok_or_else(|| stale("Open a named dev session for this project first"))?;
    if project.context.is_empty()
        || !contexts(cdp)?
            .iter()
            .any(|context| context == &project.context)
        || data
            .projects
            .iter()
            .any(|(other, value)| other != id && value.context == project.context)
    {
        return Err(stale(
            "The registered project context is not an exclusive managed dev context",
        ));
    }
    Ok(())
}

pub(super) fn command(
    action: Action,
    profile: &Path,
    repo: &project::Project,
    data: &mut Registry,
) -> Result<ExitCode, Failure> {
    let browser = running(data, profile)?;
    if matches!(action, Action::Restore) {
        let snapshot = read_snapshot(&snapshot_path(&repo.id)?)?;
        snapshot.validate(&repo.id)?;
        restore(profile, repo, data, browser, snapshot)?;
        println!("state: restored");
        return passed();
    }

    let browser =
        browser.ok_or_else(|| stale("Open a named dev session for this project first"))?;
    let mut cdp = Cdp::connect(&browser)?;
    verify_context(data, &repo.id, &mut cdp)?;
    let project = data
        .projects
        .get_mut(&repo.id)
        .expect("verified project context");
    if let Action::Register(origin) = action {
        project.origins.insert(origin);
        save(profile, data)?;
        println!("state: origin registered");
        return passed();
    }

    if project.origins.is_empty() {
        return Err(usage(
            "Register a project origin with dev state register <origin> before saving",
        ));
    }
    let path = snapshot_path(&repo.id)?;
    existing_file(&path)?;
    let cookies = capture_cookies(&mut cdp, project)?;
    let mut origins = BTreeMap::new();
    for origin in &project.origins {
        origins.insert(
            origin.clone(),
            storage::access(&mut cdp, project, origin, None)?,
        );
    }
    let snapshot = Snapshot {
        version: FORMAT,
        scope: SCOPE.into(),
        project: repo.id.clone(),
        origins,
        cookies,
    };
    snapshot.validate(&repo.id)?;
    write_snapshot(&path, &snapshot)?;
    println!("state: saved\norigins: {}", snapshot.origins.len());
    passed()
}

fn restore(
    profile: &Path,
    repo: &project::Project,
    data: &mut Registry,
    browser: Option<Browser>,
    snapshot: Snapshot,
) -> Result<(), Failure> {
    let browser = match browser {
        Some(browser) => browser,
        None => {
            let browser = process::launch(profile)?;
            data.browser = Some(browser.clone());
            data.projects.clear();
            data.pending_restore = None;
            save(profile, data)?;
            browser
        }
    };
    let mut cdp = Cdp::connect(&browser)?;
    if data.projects.contains_key(&repo.id) {
        let project = &data.projects[&repo.id];
        if project.context.is_empty() {
            return Err(stale("Invalid recorded dev context"));
        }
        if contexts(&mut cdp)?.iter().any(|id| id == &project.context) {
            return Err(failure(
                "Close the project's live dev context before restoring",
            ));
        }
    }
    if data
        .sessions
        .values()
        .any(|session| session.project == repo.id && session.generation == browser.path)
    {
        return Err(stale(
            "Close the project's existing dev sessions before restoring",
        ));
    }
    if let Some(pending) = &data.pending_restore {
        if pending.is_empty()
            || data
                .projects
                .values()
                .any(|project| &project.context == pending)
        {
            return Err(stale("Invalid staged dev context"));
        }
        if contexts(&mut cdp)?.iter().any(|id| id == pending) {
            dispose(&mut cdp, pending)?;
        }
    }
    let context = cdp.string("Target.createBrowserContext", json!({}), "browserContextId")?;
    if context.is_empty()
        || data
            .projects
            .values()
            .any(|project| project.context == context)
    {
        return Err(failure("Could not identify a fresh managed dev context"));
    }
    let project = Project {
        label: repo
            .location
            .common_dir
            .parent()
            .and_then(Path::file_name)
            .and_then(|name| name.to_str())
            .unwrap_or(repo.id.as_str())
            .to_owned(),
        context: context.clone(),
        window: 0,
        origins: snapshot.origins.keys().cloned().collect(),
    };
    let result = (|| {
        if !contexts(&mut cdp)?.iter().any(|id| id == &context) {
            return Err(failure("Could not verify the new Chrome context"));
        }
        // Commit a cleanup record before sending any snapshot values. If the
        // command dies, the next restore can dispose this unpublished context.
        data.pending_restore = Some(context.clone());
        save(profile, data)?;
        let expected: Vec<_> = snapshot
            .cookies
            .iter()
            .filter(|cookie| {
                cookie
                    .expires
                    .is_none_or(|expires| expires > vega_shim::unix_now() as f64)
            })
            .collect();
        let cookies = expected
            .iter()
            .map(|cookie| cookie.params())
            .collect::<Result<Vec<_>, _>>()?;
        cdp.call(
            "Storage.setCookies",
            json!({"browserContextId": context, "cookies": cookies}),
        )
        .map_err(|_| failure("Could not replay managed dev cookies"))?;
        let replayed = capture_cookies(&mut cdp, &project)?;
        if replayed.len() != expected.len()
            || replayed.iter().any(|cookie| !expected.contains(&cookie))
        {
            return Err(failure(
                "Chrome did not retain all supported dev cookie attributes",
            ));
        }
        for (origin, entries) in &snapshot.origins {
            storage::access(&mut cdp, &project, origin, Some(entries))?;
        }
        data.projects.insert(repo.id.clone(), project);
        data.pending_restore = None;
        save(profile, data)
    })();
    if result.is_err() {
        // A timed-out page command can leave the socket's read side unusable.
        if Cdp::connect(&browser)
            .and_then(|mut socket| dispose(&mut socket, &context))
            .is_err()
        {
            return Err(failure(
                "Dev replay failed and the staged context could not be disposed",
            ));
        }
    }
    result
}

fn dispose(cdp: &mut Cdp, context: &str) -> Result<(), Failure> {
    cdp.call(
        "Target.disposeBrowserContext",
        json!({"browserContextId": context}),
    )?;
    Ok(())
}
