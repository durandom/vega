//! Offline project identity, separate from the local paths used for Git and bd.
//! Consumers migrate in the flag-day cutover; legacy lane keys stay unchanged
//! until then. See `docs/design/project-identity.md`.

use crate::Failure;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;
use std::io::{Read, Seek, SeekFrom};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

/// A normalized fetch remote, or an explicit host-scoped local identity.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ProjectId(String);

impl ProjectId {
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Reversible UTF-8 hex, including the identity kind. Use after a `v2`
    /// storage namespace; never feed an ID to a legacy lossy filename encoder.
    pub fn encoded(&self) -> String {
        encode_component(self.as_str())
    }
}

impl std::ops::Deref for ProjectId {
    type Target = str;
    fn deref(&self) -> &str {
        self.as_str()
    }
}

impl fmt::Display for ProjectId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Paths on this host only. Bare repositories use the bare root as checkout.
/// Linked worktrees share common_dir but retain their own checkout root.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct RepoLocation {
    pub common_dir: PathBuf,
    pub checkout: PathBuf,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Project {
    pub id: ProjectId,
    pub location: RepoLocation,
}

/// An explicitly local catalog. Populate from local paths, never registry
/// metadata from other hosts. Each checkout is retained; sync consumers can
/// group by common_dir to keep independent clones separate.
#[derive(Debug)]
pub struct LocationCatalog {
    host: String,
    locations: BTreeMap<ProjectId, Vec<RepoLocation>>,
}

impl LocationCatalog {
    /// Pass the registry's host identity, not a remote entry's host.
    pub fn new(host: &str) -> Result<Self, Failure> {
        validate_host(host)?;
        Ok(Self {
            host: host.into(),
            locations: BTreeMap::new(),
        })
    }

    pub fn resolve(&mut self, path: &Path) -> Result<Project, Failure> {
        let project = resolve(path, &self.host)?;
        if self.locations.iter().any(|(id, locations)| {
            id != &project.id
                && locations
                    .iter()
                    .any(|location| location.common_dir == project.location.common_dir)
        }) {
            return Err(failure("project_identity_changed", "A cataloged repository changed project identity", "quiesce writers and perform the explicit journaled rekey before rebuilding the catalog"));
        }
        let locations = self.locations.entry(project.id.clone()).or_default();
        locations.push(project.location.clone());
        locations.sort();
        locations.dedup();
        Ok(project)
    }

    pub fn locations(&self, id: &ProjectId) -> &[RepoLocation] {
        self.locations
            .get(id)
            .map(Vec::as_slice)
            .unwrap_or_default()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&ProjectId, &[RepoLocation])> {
        self.locations
            .iter()
            .map(|(id, locations)| (id, locations.as_slice()))
    }
}

/// Select paths from owner-local herdr metadata only. A workspace checkout
/// takes precedence over its common-dir location, then the pane's cwd.
pub fn metadata_path(workspace: Option<&serde_json::Value>, cwd: &str) -> PathBuf {
    let path = workspace
        .and_then(|w| {
            w["worktree"]["checkout_path"]
                .as_str()
                .filter(|s| !s.is_empty())
                .or_else(|| w["worktree"]["repo_key"].as_str().filter(|s| !s.is_empty()))
        })
        .unwrap_or(cwd);
    match (path.strip_prefix("~/"), std::env::var_os("HOME")) {
        (Some(rest), Some(home)) => Path::new(&home).join(rest),
        _ => path.into(),
    }
}

/// Return a diagnostic only when this local path's failure changes. Daemons
/// skip failed projects; retries should not flood logs. Clear on recovery.
pub fn diagnostic(scope: &str, path: &Path, error: Option<&Failure>) -> Option<String> {
    registered_diagnostic(scope, "", path, error)
}

/// [`diagnostic`] for one registered project ID at the path. A stale record
/// shares its path with the re-registered one, whose recovery must not clear
/// the stale record's failure and log it again on every pass.
pub fn registered_diagnostic(
    scope: &str,
    id: &str,
    path: &Path,
    error: Option<&Failure>,
) -> Option<String> {
    type Last = BTreeMap<(String, String, PathBuf), String>;
    static LAST: std::sync::OnceLock<std::sync::Mutex<Last>> = std::sync::OnceLock::new();
    let mut last = LAST
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let key = (scope.to_owned(), id.to_owned(), path.to_path_buf());
    let Some(error) = error else {
        last.remove(&key);
        return None;
    };
    let message = format!(
        "{}: {}: {}; {}",
        path.display(),
        error.code,
        error.message,
        error.hint
    );
    if last.get(&key) == Some(&message) {
        return None;
    }
    last.insert(key, message.clone());
    Some(message)
}

/// A registered location (`what`) now resolves to `current`, another ID or
/// common dir: workers skip it until the operator re-registers the location
/// and removes the stale record (docs/design/project-identity.md).
pub fn identity_changed(what: &str, registered: &str, current: &Project) -> Failure {
    Failure::new(
        "project_identity_changed",
        format!(
            "{what} changed project identity: registered {registered}, now {} at {}",
            current.id,
            current.location.common_dir.display()
        ),
        "a remote change needs a rekey: stop writers, re-register under the new ID \
         (`vega uac claim`, `vega bd init` in the checkout), then remove the stale \
         records (docs/design/project-identity.md, Rekey after a remote change)",
    )
}

fn failure(code: &str, message: &str, hint: &str) -> Failure {
    Failure::new(code, message, hint)
}

/// Repository-selecting variables that would override `-C`; a caller inside a
/// hook or `git rebase --exec` has them set for its own repository.
const GIT_REPO_ENV: [&str; 5] = [
    "GIT_DIR",
    "GIT_COMMON_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_OBJECT_DIRECTORY",
];

fn git_command(path: &Path, args: &[&str]) -> Command {
    let mut command = Command::new("git");
    command
        .arg("-C")
        .arg(path)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("LC_ALL", "C");
    for key in GIT_REPO_ENV {
        command.env_remove(key);
    }
    command
}

// Resolver probes also run inside the bd shim. Keep the shim's metadata
// deadline and descendant cleanup when using the shared identity resolver.
fn output_within(cmd: &mut Command, timeout: Duration) -> std::io::Result<Option<Output>> {
    let mut stdout = tempfile::tempfile()?;
    let mut stderr = tempfile::tempfile()?;
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(stdout.try_clone()?)
        .stderr(stderr.try_clone()?)
        .process_group(0)
        .spawn()?;
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if start.elapsed() < timeout => std::thread::sleep(Duration::from_millis(10)),
            result => {
                // SAFETY: this child leads a new process group, never our own.
                unsafe { libc::kill(-(child.id() as i32), libc::SIGKILL) };
                let _ = child.kill();
                let _ = child.wait();
                return result.map(|_| None);
            }
        }
    };
    let read = |file: &mut std::fs::File| -> std::io::Result<Vec<u8>> {
        file.seek(SeekFrom::Start(0))?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        Ok(bytes)
    };
    Ok(Some(Output {
        status,
        stdout: read(&mut stdout)?,
        stderr: read(&mut stderr)?,
    }))
}

fn git(path: &Path, args: &[&str]) -> Result<Output, Failure> {
    let result =
        output_within(&mut git_command(path, args), Duration::from_secs(2)).map_err(|_| {
            failure(
                "project_git_io",
                "Could not run local Git",
                "check Git installation and repository permissions",
            )
        })?;
    result.ok_or_else(|| {
        failure(
            "project_git_timeout",
            "Local Git exceeded two seconds",
            "check the local repository and retry",
        )
    })
}

fn git_text(path: &Path, args: &[&str]) -> Result<String, Failure> {
    let output = git(path, args)?;
    if !output.status.success() {
        // Git stderr may quote config values or credential-bearing URLs.
        return Err(failure(
            "project_git_failed",
            "Local Git could not inspect the repository",
            "check repository layout and config permissions",
        ));
    }
    String::from_utf8(output.stdout)
        .map(|s| s.trim_end_matches('\n').to_owned())
        .map_err(|_| {
            failure(
                "project_git_encoding",
                "Git returned non-UTF-8 data",
                "use UTF-8 repository paths and configuration",
            )
        })
}

fn canonical(path: &Path) -> Result<PathBuf, Failure> {
    path.canonicalize().map_err(|_| {
        failure(
            "project_path_io",
            "Could not canonicalize a local repository path",
            "check that the repository or remote target exists and is accessible",
        )
    })
}

fn path_text(path: &Path) -> Result<&str, Failure> {
    path.to_str().ok_or_else(|| {
        failure(
            "project_path_encoding",
            "Repository path is not UTF-8",
            "use UTF-8 repository paths",
        )
    })
}

fn validate_host(host: &str) -> Result<(), Failure> {
    if host.is_empty()
        || host
            .chars()
            .any(|c| c.is_control() || matches!(c, ':' | '/' | '\\'))
    {
        return Err(failure(
            "project_host_invalid",
            "Invalid local registry host identity",
            "pass the local registry host identity",
        ));
    }
    Ok(())
}

/// Optional CLI discovery distinguishes an actual non-repository from Git or
/// config failures. Classify Git's stable C-locale diagnostic without exposing
/// its text (which can contain paths or configuration secrets).
pub fn resolve_optional(path: &Path, host: &str) -> Result<Option<Project>, Failure> {
    validate_host(host)?;
    let probe = git(path, &["rev-parse", "--git-common-dir"])?;
    if !probe.status.success() {
        if probe.stderr.starts_with(b"fatal: not a git repository") {
            return Ok(None);
        }
        return Err(failure(
            "project_git_failed",
            "Local Git could not inspect the repository",
            "check repository layout and config permissions",
        ));
    }
    resolve(path, host).map(Some)
}

/// Resolve locally with no network lookup. Configuration is read explicitly
/// from common_dir/config, including its includes but excluding worktree,
/// branch/upstream, global and command-line overrides.
pub fn resolve(path: &Path, host: &str) -> Result<Project, Failure> {
    validate_host(host)?;
    let common_dir = canonical(Path::new(&git_text(
        path,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )?))?;
    let bare = git_text(path, &["rev-parse", "--is-bare-repository"])? == "true";
    let checkout = if bare {
        common_dir.clone()
    } else if canonical(path)? == common_dir {
        match git_text(path, &["rev-parse", "--show-toplevel"]) {
            Ok(top) => canonical(Path::new(&top))?,
            Err(error) if error.code == "project_git_failed" => {
                // Ordinary .git directories have no toplevel when inspected
                // from inside. Validate their parent rather than assuming any
                // separate Git directory's parent is its checkout.
                let parent = common_dir
                    .parent()
                    .filter(|_| common_dir.file_name().is_some_and(|n| n == ".git"));
                if let Some(parent) = parent.filter(|parent| {
                    git_text(
                        parent,
                        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
                    )
                    .ok()
                    .and_then(|p| Path::new(&p).canonicalize().ok())
                    .as_ref()
                        == Some(&common_dir)
                }) {
                    parent.to_path_buf()
                } else {
                    return Err(failure(
                        "project_checkout_unknown",
                        "Separate Git directory does not identify its checkout",
                        "set core.worktree in the separate Git directory to the actual checkout path, or resolve from the checkout root",
                    ));
                }
            }
            Err(error) => return Err(error),
        }
    } else {
        canonical(Path::new(&git_text(
            path,
            &["rev-parse", "--show-toplevel"],
        )?))?
    };
    let config = common_dir.join("config");
    let output = git(
        &common_dir,
        &[
            "config",
            "--file",
            path_text(&config)?,
            "--includes",
            "--null",
            "--list",
        ],
    )?;
    if !output.status.success() {
        return Err(failure(
            "project_config_io",
            "Could not read common repository configuration",
            "repair the common repository config and its includes",
        ));
    }
    let config = String::from_utf8(output.stdout).map_err(|_| {
        failure(
            "project_config_encoding",
            "Common repository config is not UTF-8",
            "use UTF-8 remote configuration",
        )
    })?;
    let mut selected = None;
    let mut remotes: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for entry in config.split('\0').filter(|s| !s.is_empty()) {
        let (key, value) = entry.split_once('\n').unwrap_or((entry, ""));
        if key.eq_ignore_ascii_case("vega.projectremote") {
            selected = Some(value.to_owned());
        }
        // Section/key names are case insensitive; remote subsection names are not.
        if key
            .get(..7)
            .is_some_and(|s| s.eq_ignore_ascii_case("remote."))
        {
            if let Some((name, field)) = key[7..].rsplit_once('.') {
                let urls = remotes.entry(name.to_owned()).or_default();
                if field.eq_ignore_ascii_case("url") {
                    urls.push(value.to_owned());
                }
            }
        }
    }
    let remote = if let Some(name) = selected {
        Some(remotes.get(&name).ok_or_else(|| {
            failure(
                "project_remote_missing",
                "vega.projectRemote does not name a configured remote",
                "set vega.projectRemote in the common repository config to an existing remote",
            )
        })?)
    } else if let Some(urls) = remotes.get("origin") {
        Some(urls)
    } else if remotes.len() > 1 {
        return Err(failure(
            "project_remote_ambiguous",
            "Multiple remotes without origin",
            "set vega.projectRemote in the common repository config",
        ));
    } else {
        remotes.values().next()
    };
    let id = match remote {
        None => ProjectId(format!("local:{host}:{}", path_text(&common_dir)?)),
        Some(urls) => {
            if urls.len() != 1 {
                return Err(failure("project_remote_urls", "Selected remote must have exactly one fetch URL", "configure one remote.<name>.url in the common repository config; pushurl does not count"));
            }
            normalize_remote(&urls[0], host, &common_dir)?
        }
    };
    Ok(Project {
        id,
        location: RepoLocation {
            common_dir,
            checkout,
        },
    })
}

fn invalid_url() -> Failure {
    failure("project_remote_invalid", "Selected fetch URL is malformed or unsupported", "use an HTTP(S), SSH, git, SCP or local file remote without query, fragment, dot segments or encoded separators")
}

/// Normalize offline. Errors deliberately never include the input: it may
/// contain passwords, tokens or userinfo. Local targets must already exist.
pub fn normalize_remote(remote: &str, host: &str, common_dir: &Path) -> Result<ProjectId, Failure> {
    validate_host(host)?;
    if remote.is_empty()
        || remote.chars().any(|c| c.is_control())
        || remote.contains(['?', '#', '\\'])
    {
        return Err(invalid_url());
    }
    if remote.starts_with("file://") {
        let url = url::Url::parse(remote).map_err(|_| invalid_url())?;
        if !url.username().is_empty()
            || url.password().is_some()
            || url.port().is_some()
            || url.host_str().is_some_and(|h| h != "localhost")
        {
            return Err(invalid_url());
        }
        normalize_path(
            remote
                .split_once("://")
                .ok_or_else(invalid_url)?
                .1
                .split_once('/')
                .map(|(_, p)| p)
                .ok_or_else(invalid_url)?,
        )?;
        let path = url.to_file_path().map_err(|_| invalid_url())?;
        return local_remote(&path, host, common_dir);
    }
    if let Some((scheme, rest)) = remote.split_once("://") {
        let default_port = match scheme {
            "https" => 443,
            "http" => 80,
            "ssh" => 22,
            "git" => 9418,
            _ => return Err(invalid_url()),
        };
        if rest.chars().any(char::is_whitespace) {
            return Err(invalid_url());
        }
        let (authority, path) = rest.split_once('/').ok_or_else(invalid_url)?;
        if authority.is_empty() {
            return Err(invalid_url());
        }
        let path = normalize_path(path)?;
        let url = url::Url::parse(remote).map_err(|_| invalid_url())?;
        let hostname = url.host_str().ok_or_else(invalid_url)?;
        return network_id(hostname, url.port().filter(|p| *p != default_port), &path);
    }
    // A colon before any slash is SCP syntax. Bracketed IPv6 contains colons
    // of its own, so find the separator after its closing bracket.
    if !remote.starts_with(['/', '.']) {
        let separator = if let Some(end) = remote.find(']') {
            remote[end + 1..].find(':').map(|n| end + 1 + n)
        } else {
            remote.find(':')
        };
        if let Some(index) = separator.filter(|i| !remote[..*i].contains('/')) {
            if remote[..index].chars().any(char::is_whitespace) {
                return Err(invalid_url());
            }
            let authority = remote[..index].rsplit('@').next().ok_or_else(invalid_url)?;
            let raw_path = &remote[index + 1..];
            let path = normalize_path(raw_path.strip_prefix('/').unwrap_or(raw_path))?;
            let parsed =
                url::Url::parse(&format!("ssh://{authority}/")).map_err(|_| invalid_url())?;
            if parsed.port().is_some() {
                return Err(invalid_url());
            }
            return network_id(parsed.host_str().ok_or_else(invalid_url)?, None, &path);
        }
    }
    local_remote(Path::new(remote), host, common_dir)
}

fn local_remote(path: &Path, host: &str, common_dir: &Path) -> Result<ProjectId, Failure> {
    let target = canonical(&common_dir.join(path))?;
    Ok(ProjectId(format!("file:{host}:{}", path_text(&target)?)))
}

fn network_id(host: &str, port: Option<u16>, path: &str) -> Result<ProjectId, Failure> {
    if host.is_empty() || path.is_empty() {
        return Err(invalid_url());
    }
    let host = url::Host::parse(host)
        .map_err(|_| invalid_url())?
        .to_string()
        .to_ascii_lowercase();
    // GitHub paths are case-insensitive: lowercase before stripping the one
    // terminal `.git`, so `Repo.GIT` and `Repo.git` match but `Repo.git.git` stays.
    let path = if host == "github.com" {
        path.to_lowercase()
    } else {
        path.to_owned()
    };
    let path = path.strip_suffix(".git").unwrap_or(&path);
    if path.is_empty() || path.ends_with('/') {
        return Err(invalid_url());
    }
    // Lowercasing GitHub's path must not lowercase percent escape hex digits.
    let path = uppercase_escapes(path);
    Ok(ProjectId(format!(
        "{host}{}/{path}",
        port.map(|p| format!(":{p}")).unwrap_or_default()
    )))
}

fn uppercase_escapes(path: &str) -> String {
    let mut result = String::new();
    let mut chars = path.chars();
    while let Some(c) = chars.next() {
        result.push(c);
        if c == '%' {
            for _ in 0..2 {
                if let Some(c) = chars.next() {
                    result.push(c.to_ascii_uppercase());
                }
            }
        }
    }
    result
}

fn normalize_path(raw: &str) -> Result<String, Failure> {
    let mut normalized = String::new();
    let mut chars = raw.chars();
    while let Some(c) = chars.next() {
        if c != '%' {
            normalized.push(c);
            continue;
        }
        let a = chars
            .next()
            .and_then(|c| c.to_digit(16))
            .ok_or_else(invalid_url)?;
        let b = chars
            .next()
            .and_then(|c| c.to_digit(16))
            .ok_or_else(invalid_url)?;
        let byte = (a * 16 + b) as u8;
        if matches!(byte, b'/' | b'\\') {
            return Err(invalid_url());
        }
        if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) {
            normalized.push(char::from(byte));
        } else {
            normalized.push_str(&format!("%{byte:02X}"));
        }
    }
    let path = normalized.trim_end_matches('/');
    if path.is_empty() || path.split('/').any(|part| part == "." || part == "..") {
        return Err(invalid_url());
    }
    Ok(path.to_owned())
}

pub fn encode_component(value: &str) -> String {
    value
        .as_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

pub fn decode_component(encoded: &str) -> Result<String, Failure> {
    let invalid = || {
        failure(
            "project_key_invalid",
            "Invalid UTF-8 hex storage component",
            "use the v2 encoded project ID or location component",
        )
    };
    if !encoded.len().is_multiple_of(2) {
        return Err(invalid());
    }
    let bytes = encoded
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let a = char::from(pair[0]).to_digit(16).ok_or_else(invalid)?;
            let b = char::from(pair[1]).to_digit(16).ok_or_else(invalid)?;
            Ok((a * 16 + b) as u8)
        })
        .collect::<Result<Vec<_>, Failure>>()?;
    String::from_utf8(bytes).map_err(|_| invalid())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_probes_kill_descendants_and_ignore_inherited_pipes() {
        let tmp = tempfile::tempdir().unwrap();
        let marker = tmp.path().join("survived");
        let mut command = Command::new("/bin/sh");
        command
            .args(["-c", "(sleep 0.3; echo survived > \"$1\") & wait", "probe"])
            .arg(&marker);
        assert!(output_within(&mut command, Duration::from_millis(50))
            .unwrap()
            .is_none());
        std::thread::sleep(Duration::from_millis(400));
        assert!(!marker.exists());
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "sleep 0.3 & echo ready"]);
        let start = Instant::now();
        let output = output_within(&mut command, Duration::from_secs(1))
            .unwrap()
            .unwrap();
        assert!(output.status.success());
        assert!(start.elapsed() < Duration::from_millis(250));
        assert_eq!(output.stdout, b"ready\n");
    }

    use tempfile::TempDir;

    fn run(path: &Path, args: &[&str]) {
        let out = Command::new("git")
            .arg("-C")
            .arg(path)
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    fn repo(root: &Path, name: &str, bare: bool) -> PathBuf {
        let dir = root.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        if bare {
            run(&dir, &["init", "--bare", "-q"]);
        } else {
            run(&dir, &["init", "-q"]);
        }
        dir
    }

    fn config(path: &Path, key: &str, value: &str) {
        run(path, &["config", "--local", key, value]);
    }

    fn remote(path: &Path, name: &str, url: &str) {
        run(path, &["remote", "add", name, url]);
    }

    #[test]
    fn normalization_table() {
        let cases = [
            (
                "git@github.com:durandom/orrery.git",
                "github.com/durandom/orrery",
            ),
            (
                "ssh://git@github.com/durandom/orrery.git",
                "github.com/durandom/orrery",
            ),
            (
                "https://github.com/Durandom/Orrery.git/",
                "github.com/durandom/orrery",
            ),
            ("https://github.com/Org/Repo.GIT", "github.com/org/repo"),
            (
                "https://github.com/Org/Repo.git.git",
                "github.com/org/repo.git",
            ),
            (
                "https://github.com/Org/Repo.GIT.git",
                "github.com/org/repo.git",
            ),
            (
                "https://git.example/Team/Repo.GIT",
                "git.example/Team/Repo.GIT",
            ),
            ("https://git.example/Team/Repo.git", "git.example/Team/Repo"),
            ("git@git.example:/Team/Repo.git", "git.example/Team/Repo"),
            ("https://git.example/Team//Repo", "git.example/Team//Repo"),
            (
                "https://secret:password@GIT.EXAMPLE:443/Team/Repo.git",
                "git.example/Team/Repo",
            ),
            ("http://GIT.EXAMPLE:80/Team/Repo", "git.example/Team/Repo"),
            (
                "ssh://git@GIT.EXAMPLE:22/Team/Repo",
                "git.example/Team/Repo",
            ),
            ("git://GIT.EXAMPLE:9418/Team/Repo", "git.example/Team/Repo"),
            (
                "ssh://git@GIT.EXAMPLE:2222/a/b/Repo.git",
                "git.example:2222/a/b/Repo",
            ),
            ("https://git.example:444/Repo", "git.example:444/Repo"),
            ("http://git.example:81/Repo", "git.example:81/Repo"),
            ("git://git.example:123/Repo", "git.example:123/Repo"),
            (
                "https://git.example/%54eam/R%65po%7e%2b",
                "git.example/Team/Repo~%2B",
            ),
            ("https://github.com/TEAM/Repo%2b", "github.com/team/repo%2B"),
            (
                "ssh://git@MÜNICH.example/Repo",
                "xn--mnich-kva.example/Repo",
            ),
            (
                "https://[2001:db8::1]:443/Team/Repo",
                "[2001:db8::1]/Team/Repo",
            ),
            (
                "ssh://git@[2001:db8::1]:2222/Repo",
                "[2001:db8::1]:2222/Repo",
            ),
            ("git@[2001:db8::1]:Repo.git", "[2001:db8::1]/Repo"),
            ("git.example:Team/Repo.git.git", "git.example/Team/Repo.git"),
        ];
        for (input, expected) in cases {
            assert_eq!(
                normalize_remote(input, "host", Path::new("/"))
                    .unwrap()
                    .as_str(),
                expected,
                "{input}"
            );
        }
    }

    #[test]
    fn rejected_urls_and_errors_never_expose_credentials() {
        for input in [
            "",
            "https://secret:password@git.example",
            "https://git.example/",
            "https://git.example/repo?token=secret",
            "ssh://git.example/repo#fragment",
            "ftp://git.example/repo",
            "https://git.example/a/../repo",
            "https://git.example/./repo",
            "https://git.example/a/%2e%2e/repo",
            "https://git.example/a%2fb",
            "git.example:a%5Cb",
            "https://git.example/Repo%",
            "https://git.example/Repo%GG",
            "https://git.example/.git",
            "ssh:///repo",
            "ssh://[broken/repo",
            "ssh://%GG/repo",
        ] {
            let err = normalize_remote(input, "host", Path::new("/")).unwrap_err();
            assert_eq!(err.code, "project_remote_invalid", "{input}");
            assert!(!format!("{err:?}").contains("secret"));
            assert!(!format!("{err:?}").contains("password"));
        }
    }

    #[test]
    fn local_targets_are_canonical_and_host_scoped() {
        let tmp = TempDir::new().unwrap();
        let common = repo(tmp.path(), "clone", false).join(".git");
        let target = repo(tmp.path(), "Target.git", true).canonicalize().unwrap();
        for url in [
            "../../Target.git".to_owned(),
            target.to_str().unwrap().to_owned(),
            url::Url::from_file_path(&target).unwrap().to_string(),
        ] {
            let a = normalize_remote(&url, "a", &common).unwrap();
            let b = normalize_remote(&url, "b", &common).unwrap();
            assert_eq!(a.as_str(), format!("file:a:{}", target.display()));
            assert_ne!(a, b);
        }
        assert!(normalize_remote("file://remote-host/repo", "a", &common).is_err());
        assert!(normalize_remote("../../missing", "a", &common).is_err());
        assert!(normalize_remote("file:///a/%2e%2e/b", "a", &common).is_err());
    }

    #[test]
    fn git_ignores_the_callers_repository_env() {
        let command = git_command(Path::new("/repo"), &["config", "--list"]);
        let removed: Vec<_> = command
            .get_envs()
            .filter(|(_, value)| value.is_none())
            .map(|(key, _)| key.to_str().unwrap().to_owned())
            .collect();
        for key in GIT_REPO_ENV {
            assert!(removed.iter().any(|k| k == key), "{key} not cleared");
        }
    }

    #[test]
    fn selection_forks_ambiguity_and_fetch_urls() {
        let tmp = TempDir::new().unwrap();
        let path = repo(tmp.path(), "clone", false);
        let local = resolve(&path, "host").unwrap();
        assert_eq!(
            local.id.as_str(),
            format!("local:host:{}", local.location.common_dir.display())
        );
        assert_ne!(local.id, resolve(&path, "other-host").unwrap().id);
        remote(&path, "upstream", "https://github.com/parent/repo.git");
        let parent = resolve(&path, "host").unwrap().id;
        remote(&path, "mirror", "git@github.com:parent/repo.git");
        assert_eq!(
            resolve(&path, "host").unwrap_err().code,
            "project_remote_ambiguous"
        );
        config(&path, "vega.projectRemote", "upstream");
        assert_eq!(resolve(&path, "host").unwrap().id, parent);
        remote(&path, "origin", "https://github.com/fork/repo.git");
        run(&path, &["config", "--unset", "vega.projectRemote"]);
        config(&path, "remote.pushDefault", "upstream");
        config(&path, "branch.main.remote", "upstream");
        let fork = resolve(&path, "host").unwrap().id;
        assert_ne!(fork, parent);
        config(
            &path,
            "remote.origin.pushurl",
            "https://github.com/parent/repo.git",
        );
        assert_eq!(resolve(&path, "host").unwrap().id, fork);
        config(&path, "vega.projectRemote", "mirror");
        assert_eq!(resolve(&path, "host").unwrap().id, parent);
        config(&path, "vega.projectRemote", "missing");
        assert_eq!(
            resolve(&path, "host").unwrap_err().code,
            "project_remote_missing"
        );
        config(&path, "vega.projectRemote", "origin");
        run(
            &path,
            &[
                "config",
                "--local",
                "--add",
                "remote.origin.url",
                "https://github.com/second/repo",
            ],
        );
        assert_eq!(
            resolve(&path, "host").unwrap_err().code,
            "project_remote_urls"
        );
        run(&path, &["config", "--unset-all", "remote.origin.url"]);
        assert_eq!(
            resolve(&path, "host").unwrap_err().code,
            "project_remote_urls"
        );
        config(
            &path,
            "remote.origin.url",
            "ftp://secret:password@example/repo",
        );
        assert_eq!(
            resolve(&path, "host").unwrap_err().code,
            "project_remote_invalid"
        );
    }

    #[test]
    fn separate_git_directory_requires_checkout_or_core_worktree() {
        let tmp = TempDir::new().unwrap();
        let checkout = tmp.path().join("Documents/brain");
        let gitdir = tmp.path().join("brain.git");
        std::fs::create_dir_all(checkout.parent().unwrap()).unwrap();
        run(
            tmp.path(),
            &[
                "init",
                "-q",
                "--separate-git-dir",
                gitdir.to_str().unwrap(),
                checkout.to_str().unwrap(),
            ],
        );
        let from_checkout = resolve(&checkout, "host").unwrap();
        let error = resolve(&gitdir, "host").unwrap_err();
        assert_eq!(error.code, "project_checkout_unknown");
        assert!(error.hint.contains("core.worktree"));
        config(&gitdir, "core.worktree", checkout.to_str().unwrap());
        assert_eq!(resolve(&gitdir, "host").unwrap(), from_checkout);
    }

    #[test]
    fn git_layouts_common_config_and_local_catalog() {
        let tmp = TempDir::new().unwrap();
        let plain = repo(tmp.path(), "plain", false);
        run(
            &plain,
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.invalid",
                "commit",
                "--allow-empty",
                "-qm",
                "initial",
            ],
        );
        remote(&plain, "origin", "git@github.com:Team/Repo.git");
        let linked = tmp.path().join("linked");
        run(
            &plain,
            &["worktree", "add", "-qb", "linked", linked.to_str().unwrap()],
        );
        config(&plain, "extensions.worktreeConfig", "true");
        run(
            &linked,
            &[
                "config",
                "--worktree",
                "remote.origin.url",
                "https://github.com/wrong/repo",
            ],
        );
        run(
            &linked,
            &["config", "--worktree", "vega.projectRemote", "missing"],
        );
        let bare = repo(tmp.path(), "group/.bare", true);
        remote(&bare, "origin", "https://github.com/team/repo.git");
        let sibling = tmp.path().join("group/main");
        run(&bare, &["fetch", "-q", plain.to_str().unwrap(), "HEAD"]);
        run(
            &bare,
            &[
                "worktree",
                "add",
                "-qb",
                "main",
                sibling.to_str().unwrap(),
                "FETCH_HEAD",
            ],
        );
        let standalone = repo(tmp.path(), "standalone.git", true);
        remote(
            &standalone,
            "another-name",
            "ssh://git@github.com/team/repo",
        );
        let mut catalog = LocationCatalog::new("host").unwrap();
        let a = catalog.resolve(&plain).unwrap();
        let b = catalog.resolve(&linked).unwrap();
        assert_eq!(a.id, b.id);
        assert_eq!(a.id, resolve(&plain, "another-host").unwrap().id);
        assert_eq!(a.location.common_dir, b.location.common_dir);
        assert_ne!(a.location.checkout, b.location.checkout);
        for dir in [&bare, &sibling, &standalone] {
            assert_eq!(catalog.resolve(dir).unwrap().id, a.id);
        }
        catalog.resolve(&plain.join(".git")).unwrap();
        assert_eq!(catalog.locations(&a.id).len(), 5);
        assert_eq!(catalog.iter().count(), 1);
        assert_eq!(
            resolve(&standalone, "host").unwrap().location.checkout,
            standalone.canonicalize().unwrap()
        );
        config(
            &plain,
            "remote.origin.url",
            "https://github.com/changed/repo",
        );
        assert_eq!(
            catalog.resolve(&plain).unwrap_err().code,
            "project_identity_changed"
        );
        assert_eq!(catalog.locations(&a.id).len(), 5);
        assert_eq!(
            resolve(tmp.path(), "host").unwrap_err().code,
            "project_git_failed"
        );
    }

    #[test]
    fn malformed_common_config_is_an_error() {
        let tmp = TempDir::new().unwrap();
        let path = repo(tmp.path(), "clone", false);
        std::fs::write(path.join(".git/config"), "[broken\nsecret=password").unwrap();
        let err = resolve(&path, "host").unwrap_err();
        assert!(resolve_optional(&path, "host").is_err());
        assert_eq!(resolve_optional(tmp.path(), "host").unwrap(), None);
        assert!(!format!("{err:?}").contains("password"));
        assert!(err.code.starts_with("project_"));
    }

    #[test]
    fn git_io_failures_are_bounded_and_structured() {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(mode) = std::env::var("VEGA_PROJECT_TEST_MODE") {
            let start = std::time::Instant::now();
            let err = git_text(Path::new("/"), &["config", "--list"]).unwrap_err();
            assert_eq!(
                err.code,
                match mode.as_str() {
                    "hang" => "project_git_timeout",
                    "encoding" => "project_git_encoding",
                    "missing" => "project_git_io",
                    _ => "project_git_failed",
                }
            );
            assert!(start.elapsed() < Duration::from_secs(5));
            assert!(!format!("{err:?}").contains("secret-password"));
            return;
        }
        let tmp = TempDir::new().unwrap();
        let bin = tmp.path().join("git");
        std::fs::write(
            &bin,
            r#"#!/bin/sh
[ "$1" = --vega-test-warmup ] && exit 0
[ "$GIT_TERMINAL_PROMPT" = 0 ] && [ "$GIT_OPTIONAL_LOCKS" = 0 ] || exit 70
case "$VEGA_PROJECT_TEST_MODE" in
 hang) exec /bin/sleep 60 ;;
 encoding) printf '\377' ;;
 *) echo secret-password >&2; exit 1 ;;
esac
"#,
        )
        .unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        // Keep macOS first-exec assessment outside the production Git budget.
        assert!(Command::new(&bin)
            .arg("--vega-test-warmup")
            .status()
            .unwrap()
            .success());
        for mode in ["fail", "hang", "encoding", "missing"] {
            let output = Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "project::tests::git_io_failures_are_bounded_and_structured",
                    "--nocapture",
                ])
                .env(
                    "PATH",
                    if mode == "missing" {
                        tmp.path().join("missing")
                    } else {
                        tmp.path().to_path_buf()
                    },
                )
                .env("VEGA_PROJECT_TEST_MODE", mode)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{mode}: {}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }

    #[test]
    fn reversible_storage_components_do_not_collide() {
        let values = [
            "github.com/a/b",
            "github.com/a_b",
            "github.com/a:b",
            "local:a:/x",
            "file:a:/x",
            "file:b:/x",
            "git.example/团队/répo",
            "/x/.bare",
            "/x/_bare",
        ];
        let mut keys = std::collections::BTreeSet::new();
        for value in values {
            let encoded = encode_component(value);
            assert!(keys.insert(encoded.clone()));
            assert_eq!(decode_component(&encoded).unwrap(), value);
        }
        for bad in ["f", "xz", "ff", "é"] {
            assert!(decode_component(bad).is_err());
        }
        let id = normalize_remote("https://git.example/团队/répo", "host", Path::new("/")).unwrap();
        assert_eq!(decode_component(&id.encoded()).unwrap(), id.as_str());
    }
    #[test]
    fn diagnostics_recover_independently_for_each_consumer() {
        let path = Path::new("/diagnostic-scope-fixture");
        let error = failure("project_git_failed", "unavailable", "repair configuration");
        assert!(diagnostic("bd", path, Some(&error)).is_some());
        assert!(diagnostic("bd", path, Some(&error)).is_none());
        assert!(diagnostic("layout", path, None).is_none());
        assert!(diagnostic("bd", path, Some(&error)).is_none());
        diagnostic("bd", path, None);
        assert!(diagnostic("bd", path, Some(&error)).is_some());
    }

    #[test]
    fn a_stale_record_beside_a_healthy_one_logs_once() {
        let path = Path::new("/diagnostic-stale-fixture");
        let error = failure("project_identity_changed", "changed", "rekey it");
        let mut logged = Vec::new();
        for _ in 0..3 {
            assert!(registered_diagnostic("git", "new", path, None).is_none());
            logged.extend(registered_diagnostic("git", "old", path, Some(&error)));
        }
        assert_eq!(logged.len(), 1, "{logged:?}");
        assert!(logged[0].ends_with("changed; rekey it"), "{logged:?}");
    }
}
