//! Folder trust on spawn (Slayer decisions vega-nic.8 and vega-lhc.55).
//!
//! Trust is per agent kind: Claude keeps `projects[<path>].hasTrustDialogAccepted`
//! in `~/.claude.json`, codex keeps `[projects."<path>"] trust_level` in
//! `~/.codex/config.toml`. Before a claude or codex agent starts, vega gives
//! it trust for its folder when the repo is already trusted, by that kind or
//! by the other one. The repo is the folder's checkout, the repo root (the
//! parent of the git common dir) or the main checkout (on `main`/`master`).
//! vega never creates trust that no kind has, and keeps codex's explicit
//! `untrusted` entries.
//!
//! How each agent looks it up (Claude Code 2.1.296, codex 0.162):
//! - Claude walks from the cwd up to the git toplevel, never above it, so a
//!   trusted parent of the repo does not count. vega writes the toplevel.
//! - Codex looks at the repo root when it has a `.git`: worktrees of a normal
//!   clone inherit it. A bare repo without that file (`repo/.bare`, worktrees
//!   beside it) makes codex ask per worktree: vega writes the worktree and
//!   copies the main checkout's hook approvals; codex re-checks their hashes,
//!   so changed hooks still ask. `-c projects...` overrides do not reach the
//!   dialog.
//! - A `git clone --shared` (vega gate's review clones) is its own repo; vega
//!   treats it as a checkout of the repo its `objects/info/alternates` names:
//!   that repo's trust counts, and its hook approvals are copied.
//!
//! Both files are rewritten by running agents. Writes re-read under a lock:
//! Claude's own `~/.claude.json.lock` (a proper-lockfile directory), and a
//! vega lock next to codex's config; Claude's file is replaced atomically.

use std::env;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread::sleep;
use std::time::{Duration, Instant, SystemTime};

use serde_json::{json, Value};

/// The toolbox launcher a host provisions at `~/.local/bin/hc-toolbox-shell` (see
/// docs/design/hc-migration.md, "Toolbox launch of local hc spawns"), when this host has one
/// that is executable. The host's glibc can be older than the vega shims need (vega-alc), so
/// local agents must start inside the toolbox. Shared by spawn, resume and gate reviewers.
pub fn toolbox_launcher() -> Option<String> {
    toolbox_launcher_in(Path::new(&env::var("HOME").ok()?))
}

#[cfg(unix)]
fn toolbox_launcher_in(home: &Path) -> Option<String> {
    use std::os::unix::fs::PermissionsExt;
    let path = home.join(".local/bin/hc-toolbox-shell");
    let meta = fs::metadata(&path).ok()?;
    (meta.is_file() && meta.permissions().mode() & 0o111 != 0)
        .then(|| path.to_string_lossy().into_owned())
}

/// Toolboxes are a Linux host feature; Windows never starts agents through one.
#[cfg(not(unix))]
fn toolbox_launcher_in(_home: &Path) -> Option<String> {
    None
}

/// `toolbox run` passes only a fixed env allowlist (the launcher re-injects `HC_*`), so the
/// env goes in through `env K=V ...` inside the toolbox, between the launcher and the agent.
/// An empty value stays `K=`, so it still overrides a value the toolbox would inherit.
pub fn toolbox_argv<'a>(
    launcher: &str,
    env: impl IntoIterator<Item = (&'a str, &'a str)>,
    agent: Vec<String>,
) -> Vec<String> {
    let mut out = vec![launcher.to_string(), "env".to_string()];
    out.extend(env.into_iter().map(|(k, v)| format!("{k}={v}")));
    out.extend(agent);
    out
}

/// Agent args added by `spawn`, per herdr kind. Spawned agents run without
/// approval prompts; codex also needs `--no-daemon`, because its shared daemon
/// keeps the `HERDR_*` env of the pane it first started in, and skips its
/// startup update check, whose "Update available" screen blocks the start.
pub fn default_args(kind: &str) -> &'static [&'static str] {
    match kind {
        "codex" => &[
            "--no-daemon",
            "--dangerously-bypass-approvals-and-sandbox",
            "-c",
            "check_for_update_on_startup=false",
        ],
        "claude" => &["--dangerously-skip-permissions"],
        "agy" => &["--dangerously-skip-permissions", "--sandbox"],
        "pi" => &["--approve"],
        _ => &[],
    }
}

/// The argv words that hand an agent its first prompt. claude, codex and pi
/// take it as a positional; agy rejects positionals ("Prompts are read only
/// from -p/--print, -i/--prompt-interactive, or stdin") and needs `-i`, which
/// runs the prompt and keeps the session interactive.
pub fn prompt_args(kind: &str, prompt: &str) -> Vec<String> {
    match kind {
        "agy" => vec!["-i".into(), prompt.into()],
        _ => vec![prompt.into()],
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Claude,
    Codex,
}

impl Kind {
    fn name(self) -> &'static str {
        match self {
            Kind::Claude => "claude",
            Kind::Codex => "codex",
        }
    }
}

/// The git paths whose trust counts for a folder.
struct Repo {
    dir: String,
    top: String,
    root: String,
    root_has_git: bool,
    main: Option<String>,
    /// The checkout a `git clone --shared` borrows its objects from.
    shared: Option<String>,
}

impl Repo {
    fn of(dir: &str) -> Option<Repo> {
        let dir = real(dir);
        let top = real(&git(&dir, &["rev-parse", "--show-toplevel"])?);
        let common = git(
            &dir,
            &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        )?;
        let root = Path::new(&common).parent()?;
        let alternates = Path::new(&common).join("objects/info/alternates");
        Some(Repo {
            shared: fs::read_to_string(alternates)
                .ok()
                .and_then(|a| shared_checkout(&a))
                .map(|c| real(&c)),
            root_has_git: root.join(".git").exists(),
            root: real(root.to_str()?),
            main: main_worktree(&dir).map(|m| real(&m)),
            dir,
            top,
        })
    }

    /// Paths whose trust, by any kind, vouches for the repo.
    fn sources(&self) -> Vec<&str> {
        let mut out: Vec<&str> = Vec::new();
        for p in [
            Some(&self.dir),
            Some(&self.top),
            Some(&self.root),
            self.main.as_ref(),
            self.shared.as_ref(),
        ]
        .into_iter()
        .flatten()
        {
            if !out.contains(&p.as_str()) {
                out.push(p);
            }
        }
        out
    }

    /// The entry codex looks up for this folder.
    fn codex_key(&self) -> &str {
        if self.root_has_git {
            &self.root
        } else {
            &self.top
        }
    }

    /// The checkout whose hook approvals `target` gets: the main checkout,
    /// else the repo a shared clone borrows from.
    fn hooks_from(&self, target: &str) -> Option<&str> {
        [self.main.as_deref(), self.shared.as_deref()]
            .into_iter()
            .flatten()
            .find(|c| *c != target)
    }
}

/// The checkout behind the first alternates line `<checkout>/.git/objects`.
/// A bare origin (no checkout of its own) gives none.
fn shared_checkout(alternates: &str) -> Option<String> {
    let line = alternates
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.starts_with('#'))?;
    line.trim_end_matches('/')
        .strip_suffix("/.git/objects")
        .map(str::to_string)
}

/// Gives a `kind` agent about to start in `dir` the trust its repo already
/// has. Returns the spawn output line, if vega wrote trust or failed to.
pub fn apply(kind: &str, dir: &str) -> Option<String> {
    let kind = match kind {
        "claude" => Kind::Claude,
        "codex" => Kind::Codex,
        _ => return None,
    };
    let repo = Repo::of(dir)?;
    let claude = read_claude(&claude_file());
    let codex = fs::read_to_string(codex_file()).unwrap_or_default();
    let target = match kind {
        Kind::Claude if claude_walk(&claude, &repo.dir, &repo.top) => return None,
        Kind::Claude => repo.top.clone(),
        Kind::Codex => {
            let key = repo.codex_key();
            if trusted(&codex, key) || trusted(&codex, &repo.dir) || has_project(&codex, key) {
                return None;
            }
            key.to_string()
        }
    };
    let (from, source) = source(kind, &repo, &claude, &codex)?;
    let label = kind.name();
    let written = match kind {
        Kind::Claude => trust_claude(&claude_file(), &target).map(|()| String::new()),
        Kind::Codex => trust_codex(&codex_file(), &target, repo.hooks_from(&target)),
    };
    Some(match written {
        Ok(copied) => format!(
            "{label} trust: {} trusted, as {} trusts {}{copied}",
            short(&target),
            from.name(),
            short(source)
        ),
        Err(e) => format!(
            "{label} trust: not written for {} ({e}); {label} may ask",
            short(&target)
        ),
    })
}

/// The first kind and path that trust the repo: the agent's own kind first.
fn source<'r>(kind: Kind, repo: &'r Repo, claude: &Value, codex: &str) -> Option<(Kind, &'r str)> {
    let other = match kind {
        Kind::Claude => Kind::Codex,
        Kind::Codex => Kind::Claude,
    };
    [kind, other].into_iter().find_map(|k| {
        repo.sources()
            .into_iter()
            .find(|p| match k {
                Kind::Claude => claude_trusts(claude, p),
                Kind::Codex => trusted(codex, p),
            })
            .map(|p| (k, p))
    })
}

// --- Claude ---------------------------------------------------------------

/// `$CLAUDE_CONFIG_DIR/.claude.json`, by default `~/.claude.json`.
fn claude_file() -> PathBuf {
    env::var("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| crate::platform::home().unwrap_or_default())
        .join(".claude.json")
}

fn read_claude(file: &Path) -> Value {
    fs::read_to_string(file)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or(Value::Null)
}

/// Claude's Windows project keys use disk-canonical case and forward slashes
/// (observed with native Claude Code 2.1.296 on io-win). `real` resolves the
/// directory, removing verbatim prefixes and non-root trailing separators.
/// Keep this separate from the native paths used for git and Codex.
fn claude_key(path: &str) -> String {
    #[cfg(windows)]
    return real(path).replace('\\', "/");
    #[cfg(not(windows))]
    path.to_string()
}

fn claude_trusts(config: &Value, path: &str) -> bool {
    // Do not canonicalize stored keys: Claude does an exact key lookup, so
    // an old native-backslash entry cannot vouch for trust or pass read-back.
    config["projects"][claude_key(path)]["hasTrustDialogAccepted"] == json!(true)
}

/// Claude's own check: `dir` or a parent of it up to the git toplevel.
fn claude_walk(config: &Value, dir: &str, top: &str) -> bool {
    Path::new(dir)
        .ancestors()
        .take_while(|p| p.starts_with(top))
        .any(|p| p.to_str().is_some_and(|p| claude_trusts(config, p)))
}

/// Sets `projects[path].hasTrustDialogAccepted` and nothing else, under
/// Claude's lock, on a fresh read; then reads the file back to check it.
fn trust_claude(file: &Path, path: &str) -> Result<(), String> {
    {
        let _lock = Lock::take(&with_suffix(file, ".lock")).map_err(|e| e.to_string())?;
        let text = fs::read_to_string(file).map_err(|e| e.to_string())?;
        let mut config: Value =
            serde_json::from_str(&text).map_err(|e| format!("unreadable: {e}"))?;
        if !set_trust(&mut config, path) {
            return Err("unexpected shape".to_string());
        }
        let tmp = with_suffix(file, &format!(".tmp.vega.{}", std::process::id()));
        let out = serde_json::to_string_pretty(&config).map_err(|e| e.to_string())?;
        write_new(&tmp, out.as_bytes())
            .and_then(|()| crate::platform::replace(&tmp, file))
            .map_err(|e| {
                let _ = fs::remove_file(&tmp);
                e.to_string()
            })?;
    }
    if claude_trusts(&read_claude(file), path) {
        Ok(())
    } else {
        Err("lost on read-back".to_string())
    }
}

/// Marks `path` trusted, creating a minimal project entry. False if the
/// config is not shaped as Claude writes it.
fn set_trust(config: &mut Value, path: &str) -> bool {
    let Some(root) = config.as_object_mut() else {
        return false;
    };
    let Some(projects) = root
        .entry("projects")
        .or_insert_with(|| json!({}))
        .as_object_mut()
    else {
        return false;
    };
    match projects
        .entry(claude_key(path))
        .or_insert_with(|| json!({}))
        .as_object_mut()
    {
        Some(p) => {
            p.insert("hasTrustDialogAccepted".to_string(), json!(true));
            true
        }
        None => false,
    }
}

fn write_new(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    let mut f = crate::platform::private_options(&mut options).open(path)?;
    f.write_all(bytes)?;
    f.sync_all()
}

// --- Codex ----------------------------------------------------------------

/// `$CODEX_HOME/config.toml`, by default `~/.codex/config.toml`.
fn codex_file() -> PathBuf {
    env::var("CODEX_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| crate::platform::home().unwrap_or_default().join(".codex"))
        .join("config.toml")
}

/// Appends `path`'s trust and the hook approvals of checkout `main`, unless
/// a parallel spawn already wrote an entry. Returns the hooks part of the line.
fn trust_codex(file: &Path, path: &str, main: Option<&str>) -> Result<String, String> {
    let _lock = Lock::take(&with_suffix(file, ".vega.lock")).map_err(|e| e.to_string())?;
    let config = fs::read_to_string(file).unwrap_or_default();
    if has_project(&config, path) {
        return Ok(String::new());
    }
    let main = main.filter(|m| *m != path);
    let hooks = main.map(|m| hook_approvals(&config, m)).unwrap_or_default();
    let mut f = OpenOptions::new()
        .append(true)
        .create(true)
        .open(file)
        .map_err(|e| e.to_string())?;
    f.write_all(entries(path, &hooks).as_bytes())
        .map_err(|e| e.to_string())?;
    Ok(match (main, hooks.len()) {
        (Some(m), n) if n > 0 => format!(", {n} hook approvals copied from {}", short(m)),
        _ => String::new(),
    })
}

fn project_header(path: &str) -> String {
    format!("[projects.{}]", json!(path))
}

/// Whether `[projects."<path>"]` says `trust_level = "trusted"`.
fn trusted(config: &str, path: &str) -> bool {
    #[cfg(windows)]
    return windows_project(config, path).is_some_and(|p| {
        p.get("trust_level").and_then(toml_edit::Item::as_str) == Some("trusted")
    });
    #[cfg(not(windows))]
    {
        let header = project_header(path);
        let mut lines = config.lines().map(str::trim);
        lines.any(|l| l == header)
            && lines
                .take_while(|l| !l.starts_with('['))
                .any(|l| l.replace(' ', "") == "trust_level=\"trusted\"")
    }
}

/// Any entry for `path`, trusted or not: an explicit choice vega keeps.
#[cfg(windows)]
fn windows_project(config: &str, path: &str) -> Option<toml_edit::Table> {
    let doc: toml_edit::DocumentMut = config.parse().ok()?;
    let project = doc
        .get("projects")?
        .as_table()?
        .iter()
        .find_map(|(key, value)| {
            (real(key) == real(path))
                .then(|| value.as_table().cloned())
                .flatten()
        });
    project
}

fn has_project(config: &str, path: &str) -> bool {
    #[cfg(windows)]
    return windows_project(config, path).is_some();
    #[cfg(not(windows))]
    {
        let header = project_header(path);
        config.lines().any(|l| l.trim() == header)
    }
}

/// `(event:i:j, trusted_hash)` approved for `<checkout>/.codex/hooks.json`.
fn hook_approvals(config: &str, checkout: &str) -> Vec<(String, String)> {
    #[cfg(windows)]
    {
        let Ok(doc) = config.parse::<toml_edit::DocumentMut>() else {
            return Vec::new();
        };
        let Some(state) = doc
            .get("hooks")
            .and_then(|h| h.get("state"))
            .and_then(toml_edit::Item::as_table)
        else {
            return Vec::new();
        };
        let hooks = Path::new(checkout).join(".codex/hooks.json");
        state
            .iter()
            .filter_map(|(key, value)| {
                // Split from the right: the path itself contains a drive colon.
                let mut parts = key.rsplitn(4, ':');
                let j = parts.next()?;
                let i = parts.next()?;
                let event = parts.next()?;
                let path = parts.next()?;
                (real(path) == real(hooks.to_str()?))
                    .then(|| {
                        value
                            .get("trusted_hash")?
                            .as_str()
                            .map(|hash| (format!("{event}:{i}:{j}"), hash.to_owned()))
                    })
                    .flatten()
            })
            .collect()
    }
    #[cfg(not(windows))]
    let prefix = format!("[hooks.state.\"{checkout}/.codex/hooks.json:");
    #[cfg(not(windows))]
    {
        let mut out = Vec::new();
        let mut key: Option<String> = None;
        for line in config.lines().map(str::trim) {
            if let Some(rest) = line.strip_prefix(&prefix) {
                key = rest.strip_suffix("\"]").map(str::to_string);
            } else if line.starts_with('[') {
                key = None;
            } else if let (Some(k), Some(v)) = (&key, line.strip_prefix("trusted_hash")) {
                let hash = v.trim().trim_start_matches('=').trim().trim_matches('"');
                out.push((k.clone(), hash.to_string()));
                key = None;
            }
        }
        out
    }
}

/// The config.toml lines that trust `path` and approve its hooks.
fn entries(path: &str, hooks: &[(String, String)]) -> String {
    let mut s = format!("\n{}\ntrust_level = \"trusted\"\n", project_header(path));
    for (key, hash) in hooks {
        let hook = format!("{path}/.codex/hooks.json:{key}");
        s += &format!(
            "\n[hooks.state.{}]\ntrusted_hash = {}\n",
            json!(hook),
            json!(hash)
        );
    }
    s
}

// --- Shared ---------------------------------------------------------------

/// A lock directory, as proper-lockfile (Claude) makes it: `mkdir` is atomic.
/// A lock older than proper-lockfile's 10 s stale time is taken over.
struct Lock(PathBuf);

impl Lock {
    fn take(path: &Path) -> io::Result<Lock> {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match fs::create_dir(path) {
                Ok(()) => return Ok(Lock(path.to_path_buf())),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                    if stale(path) {
                        let _ = fs::remove_dir(path);
                        continue;
                    }
                    if Instant::now() > deadline {
                        return Err(io::Error::new(
                            io::ErrorKind::TimedOut,
                            format!("{} is held", short(&path.display().to_string())),
                        ));
                    }
                    sleep(Duration::from_millis(50));
                }
                Err(e) => return Err(e),
            }
        }
    }
}

impl Drop for Lock {
    fn drop(&mut self) {
        let _ = fs::remove_dir(&self.0);
    }
}

fn stale(path: &Path) -> bool {
    fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| SystemTime::now().duration_since(t).ok())
        .is_some_and(|age| age > Duration::from_secs(10))
}

fn with_suffix(file: &Path, suffix: &str) -> PathBuf {
    let mut s = file.as_os_str().to_owned();
    s.push(suffix);
    PathBuf::from(s)
}

/// The path as the agents key it: symlinks resolved (`/tmp` is `/private/tmp`).
fn real(path: &str) -> String {
    // Verbatim Win32 paths require backslashes even when a config writer
    // appended a forward-slash hook suffix. Normalize before opening the path.
    #[cfg(windows)]
    let native = path.replace('/', "\\");
    #[cfg(windows)]
    let canonical = dunce::canonicalize(&native);
    #[cfg(not(windows))]
    let canonical = fs::canonicalize(path);
    let path = canonical
        .ok()
        .and_then(|p| p.to_str().map(str::to_string))
        .unwrap_or_else(|| path.to_string());
    #[cfg(windows)]
    let path = path.replace('/', "\\");
    path
}

/// The worktree on the repo's `main` or `master` branch.
fn main_worktree(dir: &str) -> Option<String> {
    let list = git(dir, &["worktree", "list", "--porcelain"])?;
    main_in(&list)
}

fn main_in(porcelain: &str) -> Option<String> {
    porcelain.split("\n\n").find_map(|block| {
        let path = block.lines().find_map(|l| l.strip_prefix("worktree "))?;
        let branch = block.lines().find_map(|l| l.strip_prefix("branch "))?;
        matches!(branch, "refs/heads/main" | "refs/heads/master").then(|| path.to_string())
    })
}

fn short(path: &str) -> String {
    crate::collapse_home(path)
}

fn git(dir: &str, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .stderr(Stdio::null())
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    #[test]
    fn toolbox_launcher_needs_an_executable_file() {
        use std::os::unix::fs::PermissionsExt;
        let home = tempfile::tempdir().unwrap();
        assert_eq!(super::toolbox_launcher_in(home.path()), None);
        let bin = home.path().join(".local/bin");
        std::fs::create_dir_all(&bin).unwrap();
        let f = bin.join("hc-toolbox-shell");
        std::fs::write(&f, "#!/bin/sh\n").unwrap();
        assert_eq!(super::toolbox_launcher_in(home.path()), None);
        std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(
            super::toolbox_launcher_in(home.path()),
            Some(f.to_string_lossy().into_owned())
        );
    }

    #[test]
    fn toolbox_argv_prefixes_launcher_and_carries_env() {
        let argv = super::toolbox_argv(
            "/h/hc-toolbox-shell",
            [("A", "b c"), ("HERDR_PANE_ID", "")],
            vec!["claude".into(), "--x".into()],
        );
        assert_eq!(
            argv,
            [
                "/h/hc-toolbox-shell",
                "env",
                "A=b c",
                "HERDR_PANE_ID=",
                "claude",
                "--x"
            ]
        );
    }

    use super::*;

    const CONFIG: &str = include_str!("../testdata/codex-config.toml");

    #[test]
    fn prompt_args_per_kind() {
        let p = "Read /x/prompt.md and carry out the gate review.";
        assert_eq!(prompt_args("agy", p), vec!["-i", p]);
        for kind in ["claude", "codex", "pi", "unknown"] {
            assert_eq!(prompt_args(kind, p), vec![p], "{kind}");
        }
    }

    fn repo(dir: &str, top: &str, root: &str, root_has_git: bool, main: Option<&str>) -> Repo {
        Repo {
            dir: dir.to_string(),
            top: top.to_string(),
            root: root.to_string(),
            root_has_git,
            main: main.map(str::to_string),
            shared: None,
        }
    }

    #[test]
    fn trust_entries_escape_toml_keys_and_values() {
        let path = r#"C:\Users\a user\repo"#;
        let text = entries(path, &[("stop:0:0".into(), "sha256:abc".into())]);
        let doc: toml_edit::DocumentMut = text.parse().unwrap();
        assert_eq!(
            doc["projects"][path]["trust_level"].as_str(),
            Some("trusted")
        );
        assert_eq!(
            doc["hooks"]["state"][&format!("{path}/.codex/hooks.json:stop:0:0")]["trusted_hash"]
                .as_str(),
            Some("sha256:abc")
        );
    }

    #[test]
    #[cfg(windows)]
    fn trust_matches_native_plain_forward_slash_and_verbatim_paths() {
        let dir = tempfile::tempdir().unwrap();
        let plain = dunce::canonicalize(dir.path()).unwrap();
        let verbatim = fs::canonicalize(dir.path()).unwrap();
        let plain = plain.to_str().unwrap();
        let verbatim = verbatim.to_str().unwrap();
        fs::create_dir_all(dir.path().join(".codex")).unwrap();
        fs::write(dir.path().join(".codex/hooks.json"), "{}").unwrap();
        let hooks = vec![("stop:0:0".into(), "sha256:abc".into())];
        for spelling in [
            plain.to_owned(),
            plain.replace('\\', "/"),
            verbatim.to_owned(),
        ] {
            let text = entries(&spelling, &hooks);
            assert!(trusted(&text, plain), "{spelling}");
            assert!(has_project(&text, verbatim), "{spelling}");
            assert_eq!(hook_approvals(&text, plain), hooks, "{spelling}");
        }
        let text = entries(plain, &[]).replace("\"trusted\"", "\"untrusted\"");
        assert!(has_project(&text, verbatim));
        assert!(!trusted(&text, verbatim));
    }

    #[test]
    #[cfg(windows)]
    fn claude_windows_keys_match_native_claude_not_native_rust_paths() {
        let dir = tempfile::tempdir().unwrap();
        let checkout = dir.path().join("rally-ng-maps.win-native");
        fs::create_dir(&checkout).unwrap();
        let plain = dunce::canonicalize(&checkout).unwrap();
        let plain = plain.to_str().unwrap();
        let verbatim = fs::canonicalize(&checkout).unwrap();
        let expected = plain.replace('\\', "/");
        let mut lower_drive = plain.to_string();
        lower_drive.replace_range(..1, &plain[..1].to_ascii_lowercase());
        for spelling in [
            plain.to_string(),
            expected.clone(),
            verbatim.to_str().unwrap().to_string(),
            format!("{plain}\\"),
            format!("{expected}/"),
            lower_drive,
        ] {
            let mut config = json!({"projects": {}});
            assert!(set_trust(&mut config, &spelling));
            // Assert what Claude reads, independently of vega's lookup helper.
            assert_eq!(config["projects"].as_object().unwrap().len(), 1);
            assert_eq!(
                config["projects"][&expected]["hasTrustDialogAccepted"],
                true
            );
            assert!(claude_trusts(&config, &spelling));
        }
    }

    #[test]
    #[cfg(windows)]
    fn claude_windows_repairs_old_backslash_trust_without_losing_project_data() {
        let dir = tempfile::tempdir().unwrap();
        let plain = dunce::canonicalize(dir.path()).unwrap();
        let plain = plain.to_str().unwrap();
        let key = plain.replace('\\', "/");
        let file = dir.path().join(".claude.json");
        let mut config = json!({"other": 7, "projects": {
            plain: {"hasTrustDialogAccepted": true, "lastCost": 2},
            &key: {"hasTrustDialogAccepted": false, "mcpServers": {"keep": {}}}
        }});
        assert!(!claude_trusts(&config, plain));
        assert!(!claude_walk(&config, plain, plain));
        fs::write(&file, serde_json::to_vec(&config).unwrap()).unwrap();
        trust_claude(&file, plain).unwrap();
        config["projects"][&key]["hasTrustDialogAccepted"] = json!(true);
        assert_eq!(read_claude(&file), config);
        assert!(claude_walk(&read_claude(&file), plain, plain));
    }

    #[test]
    fn trusted_reads_only_the_projects_own_section() {
        assert!(trusted(CONFIG, "/src/sk"));
        assert!(!trusted(CONFIG, "/src/other"));
        assert!(!trusted(CONFIG, "/src/none"));
        assert!(!trusted(CONFIG, "/src"));
        assert!(has_project(CONFIG, "/src/other"));
        assert!(!has_project(CONFIG, "/src/sk/new"));
    }

    #[test]
    fn hook_approvals_come_from_the_main_checkout_only() {
        assert_eq!(
            hook_approvals(CONFIG, "/src/sk/main"),
            vec![
                ("stop:0:0".to_string(), "sha256:aa".to_string()),
                ("session_start:1:0".to_string(), "sha256:bb".to_string()),
            ]
        );
        assert!(hook_approvals(CONFIG, "/src/sk").is_empty());
    }

    #[test]
    fn entries_trust_the_worktree_and_its_hooks() {
        let hooks = vec![("stop:0:0".to_string(), "sha256:aa".to_string())];
        assert_eq!(
            entries("/src/sk/new", &hooks),
            "\n[projects.\"/src/sk/new\"]\ntrust_level = \"trusted\"\n\n[hooks.state.\"/src/sk/new/.codex/hooks.json:stop:0:0\"]\ntrusted_hash = \"sha256:aa\"\n"
        );
    }

    #[test]
    fn main_in_finds_the_main_branch_worktree() {
        let list = "worktree /src/sk/.bare\nbare\n\nworktree /src/sk/feat\nHEAD 1\nbranch refs/heads/feat/x\n\nworktree /src/sk/main\nHEAD 2\nbranch refs/heads/main\n";
        assert_eq!(main_in(list).as_deref(), Some("/src/sk/main"));
        assert_eq!(main_in("worktree /a\nbare\n"), None);
    }

    #[test]
    fn claude_walk_stops_at_the_git_toplevel() {
        let config = json!({"projects": {
            "/src": {"hasTrustDialogAccepted": true},
            "/src/app": {"hasTrustDialogAccepted": false},
            "/src/lib": {"hasTrustDialogAccepted": true},
        }});
        assert!(!claude_walk(&config, "/src/app/sub", "/src/app"));
        assert!(claude_walk(&config, "/src/lib/sub", "/src/lib"));
        assert!(claude_walk(&config, "/src/lib", "/src/lib"));
    }

    #[test]
    fn source_prefers_the_own_kind_and_needs_some_trust() {
        let claude = json!({"projects": {"/src/sk": {"hasTrustDialogAccepted": true}}});
        let r = repo(
            "/src/sk/feat",
            "/src/sk/feat",
            "/src/sk",
            false,
            Some("/src/sk/main"),
        );
        let s = source(Kind::Codex, &r, &claude, CONFIG).map(|(k, p)| (k.name(), p));
        assert_eq!(s, Some(("codex", "/src/sk")));
        let s = source(Kind::Claude, &r, &claude, "").map(|(k, p)| (k.name(), p));
        assert_eq!(s, Some(("claude", "/src/sk")));
        let s = source(Kind::Claude, &r, &Value::Null, CONFIG).map(|(k, p)| (k.name(), p));
        assert_eq!(s, Some(("codex", "/src/sk")));
        let none = repo("/x", "/x", "/x", true, None);
        assert!(source(Kind::Claude, &none, &claude, CONFIG).is_none());
        // An untrusted codex entry vouches for nothing.
        let other = repo("/src/other", "/src/other", "/src/other", true, None);
        assert!(source(Kind::Claude, &other, &Value::Null, CONFIG).is_none());
    }

    #[test]
    fn codex_key_is_the_root_only_when_it_has_a_git() {
        assert_eq!(repo("/a/w", "/a/w", "/a", true, None).codex_key(), "/a");
        assert_eq!(repo("/a/w", "/a/w", "/a", false, None).codex_key(), "/a/w");
    }

    #[test]
    fn shared_checkout_reads_the_alternates_repo() {
        assert_eq!(
            shared_checkout("# x\n/src/sk/.git/objects\n/y/.git/objects\n").as_deref(),
            Some("/src/sk")
        );
        assert_eq!(shared_checkout("/src/sk/.bare/objects\n"), None);
        assert_eq!(shared_checkout(""), None);
    }

    #[test]
    fn a_shared_clone_takes_trust_and_hooks_from_its_origin() {
        let dir = env::temp_dir().join(format!("vega-trust-shared-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let (origin, clone) = (dir.join("origin"), dir.join("clone"));
        fs::create_dir_all(&origin).unwrap();
        let run = |args: &[&str]| {
            let ok = Command::new("git")
                .args(args)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .unwrap()
                .success();
            assert!(ok, "git {args:?}");
        };
        let o = origin.to_str().unwrap();
        run(&["-C", o, "init", "-q", "-b", "main"]);
        run(&[
            "-C",
            o,
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "x",
        ]);
        run(&["clone", "-q", "--shared", o, clone.to_str().unwrap()]);
        run(&["-C", clone.to_str().unwrap(), "checkout", "-q", "--detach"]);
        let r = Repo::of(clone.to_str().unwrap()).unwrap();
        let (o, c) = (real(o), real(clone.to_str().unwrap()));
        assert_eq!(r.shared.as_deref(), Some(o.as_str()));
        assert_eq!(r.codex_key(), c);
        assert_eq!(r.hooks_from(&c), Some(o.as_str()));
        let config = format!("[projects.{}]\ntrust_level = \"trusted\"\n", json!(o));
        let s = source(Kind::Codex, &r, &Value::Null, &config).map(|(k, p)| (k.name(), p));
        assert_eq!(s, Some(("codex", o.as_str())));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn hooks_from_skips_the_target_itself() {
        let mut r = repo("/c", "/c", "/c", true, Some("/c"));
        assert_eq!(r.hooks_from("/c"), None);
        r.shared = Some("/src/sk".to_string());
        assert_eq!(r.hooks_from("/c"), Some("/src/sk"));
        r.main = Some("/m".to_string());
        assert_eq!(r.hooks_from("/c"), Some("/m"));
    }

    #[test]
    fn set_trust_touches_only_the_flag() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a");
        let b = dir.path().join("b");
        fs::create_dir(&a).unwrap();
        fs::create_dir(&b).unwrap();
        let a = claude_key(a.to_str().unwrap());
        let b = claude_key(b.to_str().unwrap());
        let mut c =
            json!({"x": 1, "projects": {&a: {"lastCost": 2, "hasTrustDialogAccepted": false}}});
        assert!(set_trust(&mut c, &a));
        assert!(set_trust(&mut c, &b));
        assert_eq!(
            c,
            json!({"x": 1, "projects": {
                &a: {"lastCost": 2, "hasTrustDialogAccepted": true},
                &b: {"hasTrustDialogAccepted": true},
            }})
        );
        assert!(set_trust(&mut json!({}), "/a"));
        assert!(!set_trust(&mut json!([]), "/a"));
        assert!(!set_trust(&mut json!({"projects": []}), "/a"));
    }

    #[test]
    fn trust_claude_writes_atomically_and_keeps_the_rest() {
        let dir = env::temp_dir().join(format!("vega-trust-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join(".claude.json");
        fs::write(&file, r#"{"b": 1, "a": {"z": 1, "y": 2}, "projects": {}}"#).unwrap();
        trust_claude(&file, "/r").unwrap();
        let v = read_claude(&file);
        assert!(claude_trusts(&v, "/r"));
        // Key order survives (preserve_order).
        let keys: Vec<&String> = v.as_object().unwrap().keys().collect();
        assert_eq!(keys, ["b", "a", "projects"]);
        assert!(!with_suffix(&file, ".lock").exists());
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
        fs::write(&file, "{broken").unwrap();
        assert!(trust_claude(&file, "/r").is_err());
        assert_eq!(fs::read_to_string(&file).unwrap(), "{broken");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn trust_codex_appends_once() {
        let dir = env::temp_dir().join(format!("vega-trust-codex-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("config.toml");
        fs::write(&file, CONFIG).unwrap();
        let line = trust_codex(&file, "/src/sk/new", Some("/src/sk/main")).unwrap();
        assert_eq!(line, ", 2 hook approvals copied from /src/sk/main");
        assert_eq!(
            trust_codex(&file, "/src/sk/new", Some("/src/sk/main")).unwrap(),
            ""
        );
        let text = fs::read_to_string(&file).unwrap();
        assert!(trusted(&text, "/src/sk/new"));
        assert_eq!(text.matches("[projects.\"/src/sk/new\"]").count(), 1);
        fs::remove_dir_all(&dir).unwrap();
    }
}
