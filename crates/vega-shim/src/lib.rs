//! Shared mechanics of the agent-only shims (`shims/<tool>`, P2-P4 in
//! docs/design/agent-tooling-principles.md): resolve the real binary and
//! `exec` it, the session id, once-per-session hints and per-pane call counts.
//!
//! Everything here is best effort: a shim that cannot write its state still
//! passes the call through unchanged.

use std::env;
use std::ffi::OsString;
use std::fs;
use std::io::Write;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::time::{SystemTime, UNIX_EPOCH};

/// Replaces this process with the real `tool`, same argv (including `argv[0]`)
/// and stdio, so stdout, TTY, signals and exit code are the original's. Only
/// returns on failure: 127 when no real binary exists, 126 when exec fails.
pub fn exec_real(tool: &str, env_var: Option<&str>) -> ExitCode {
    let self_path = env::current_exe().ok().and_then(|p| p.canonicalize().ok());
    let Some(real) = resolve_real(tool, env_var, self_path.as_deref()) else {
        let via = env_var
            .map(|v| format!("{v} or PATH"))
            .unwrap_or("PATH".into());
        eprintln!(
            "{{\"error\":\"shim_target_not_found\",\"tool\":\"{tool}\",\"message\":\"no real {tool} binary found via {via} (excluding this shim)\"}}"
        );
        return ExitCode::from(127);
    };
    let mut args = env::args_os();
    let arg0 = args.next().unwrap_or_else(|| OsString::from(tool));
    let err = Command::new(&real).arg0(arg0).args(args).exec();
    eprintln!(
        "{{\"error\":\"shim_exec_failed\",\"tool\":\"{tool}\",\"target\":\"{}\",\"message\":\"{}\"}}",
        real.display(),
        err
    );
    ExitCode::from(126)
}

/// The real binary: `env_var` first (if set), then the first `tool` on `PATH`.
/// Candidates in this shim's directory, symlinks and hard links to this shim
/// are skipped, which prevents exec recursion.
pub fn resolve_real(
    tool: &str,
    env_var: Option<&str>,
    self_path: Option<&Path>,
) -> Option<PathBuf> {
    let from_env = env_var.and_then(env::var_os).map(PathBuf::from);
    let from_path = env::var_os("PATH")
        .into_iter()
        .flat_map(|p| env::split_paths(&p).collect::<Vec<_>>())
        .map(|dir| dir.join(tool));
    from_env
        .into_iter()
        .chain(from_path)
        .find(|c| is_executable(c) && !is_self(c, self_path) && !in_self_dir(c, self_path))
        .and_then(|c| c.canonicalize().ok())
}

fn is_executable(path: &Path) -> bool {
    path.metadata()
        .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

fn is_self(candidate: &Path, self_path: Option<&Path>) -> bool {
    match (
        candidate.metadata(),
        self_path.and_then(|s| s.metadata().ok()),
    ) {
        (Ok(c), Some(s)) => c.dev() == s.dev() && c.ino() == s.ino(),
        _ => false,
    }
}

fn in_self_dir(candidate: &Path, self_path: Option<&Path>) -> bool {
    let dir = candidate.parent().and_then(|p| p.canonicalize().ok());
    let own = self_path
        .and_then(Path::parent)
        .and_then(|p| p.canonicalize().ok());
    dir.is_some() && dir == own
}

/// PATH for a shim's children: remove its own directory (including aliases)
/// and directories with a symlink/hard link to this executable under `tool`.
/// Keeping this PATH for the real tool also prevents its helpers re-entering us.
pub fn path_without_self(tool: &str, self_path: &Path) -> OsString {
    let paths = env::var_os("PATH").unwrap_or_default();
    let dirs = env::split_paths(&paths).filter(|dir| {
        let candidate = dir.join(tool);
        !in_self_dir(&candidate, Some(self_path)) && !is_self(&candidate, Some(self_path))
    });
    // split_paths yields entries join_paths can represent.
    env::join_paths(dirs).expect("PATH entries must be joinable")
}

/// The agent session (P3): the harness's session id, else the herdr pane.
pub fn session_id() -> Option<String> {
    ["CLAUDE_CODE_SESSION_ID", "CODEX_THREAD_ID", "HERDR_PANE_ID"]
        .iter()
        .find_map(|k| env::var(k).ok().filter(|v| !v.is_empty()))
}

/// The herdr pane this call runs in, else the session marked with its
/// harness (`codex:<thread id>`, `claude:<session id>`); `unknown` without
/// both. Only a pane id passes [`is_pane`].
pub fn pane_key() -> String {
    let var = |k: &str| env::var(k).ok().filter(|v| !v.is_empty());
    var("HERDR_PANE_ID")
        .or_else(|| var("CODEX_THREAD_ID").map(|t| format!("codex:{t}")))
        .or_else(|| var("CLAUDE_CODE_SESSION_ID").map(|t| format!("claude:{t}")))
        .unwrap_or_else(|| "unknown".into())
}

/// herdr pane ids look like `w3V:p17`: alphanumerics, one colon. Session
/// keys (`codex:01a1-…`, a bare UUID) and `unknown` are not panes.
pub fn is_pane(key: &str) -> bool {
    let ok = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric());
    matches!(key.split_once(':'), Some((w, p)) if ok(w) && ok(p))
}

/// True the first time `key` is seen in `session` (P4: each hint once per
/// session); marks it seen. Without a session or a writable cache: true, so a
/// hint shows rather than never.
pub fn first_time(session: Option<&str>, key: &str) -> bool {
    let (Some(session), Some(cache)) = (session, cache_dir()) else {
        return true;
    };
    let dir = cache.join("vega").join("seen").join(safe(session));
    let marker = dir.join(safe(key));
    if marker.exists() {
        return false;
    }
    let _ = fs::create_dir_all(&dir).and_then(|_| fs::write(&marker, b""));
    true
}

fn cache_dir() -> Option<PathBuf> {
    env::var_os("XDG_CACHE_HOME")
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|h| Path::new(&h).join(".cache")))
}

fn state_dir() -> Option<PathBuf> {
    env::var_os("XDG_STATE_HOME")
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|h| Path::new(&h).join(".local/state")))
}

/// A file-name-safe form of a pane id or session id (`w3V:p1` → `w3V_p1`).
pub fn safe(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || "-_.".contains(c) {
                c
            } else {
                '_'
            }
        })
        .collect()
}

pub fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Per-pane call counts of a shimmed tool, host-local:
/// `$XDG_STATE_HOME/vega/calls/<tool>/<pane>`, one line
/// `<unix>\t<pane>\t<command>` per call (the command's first words only,
/// never arguments).
pub mod calls {
    use super::*;

    /// A file above this size keeps only the last hour on the next call.
    const PRUNE_BYTES: u64 = 32 * 1024;
    pub const HOUR: i64 = 3600;

    fn dir(tool: &str) -> Option<PathBuf> {
        state_dir().map(|s| s.join("vega").join("calls").join(tool))
    }

    /// Appends one call of `command` by `pane`.
    pub fn record(tool: &str, pane: &str, command: &str, now: i64) {
        let Some(dir) = dir(tool) else { return };
        let file = dir.join(safe(pane));
        if fs::metadata(&file).is_ok_and(|m| m.len() > PRUNE_BYTES) {
            let kept: String = read(&file)
                .into_iter()
                .filter(|(t, ..)| now - t < HOUR)
                .map(|(t, p, c)| format!("{t}\t{p}\t{c}\n"))
                .collect();
            let _ = fs::write(&file, kept);
        }
        let _ = fs::create_dir_all(&dir);
        let _ = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&file)
            // One write: concurrent calls append whole lines.
            .and_then(|mut f| f.write_all(format!("{now}\t{pane}\t{command}\n").as_bytes()));
    }

    /// (unix, pane, command) per line.
    fn read(file: &Path) -> Vec<(i64, String, String)> {
        fs::read_to_string(file)
            .unwrap_or_default()
            .lines()
            .filter_map(|l| {
                let mut f = l.splitn(3, '\t');
                Some((f.next()?.parse().ok()?, f.next()?.into(), f.next()?.into()))
            })
            .collect()
    }

    /// Calls of `pane` within `window` seconds whose command is `command`.
    pub fn count_of(tool: &str, pane: &str, command: &str, window: i64, now: i64) -> usize {
        dir(tool)
            .map(|d| read(&d.join(safe(pane))))
            .unwrap_or_default()
            .iter()
            .filter(|(t, _, c)| now - t < window && c == command)
            .count()
    }

    /// Calls per pane within `window` seconds, most first; panes without
    /// calls in the window are left out.
    pub fn per_pane(tool: &str, window: i64, now: i64) -> Vec<(String, usize)> {
        let Some(entries) = dir(tool).and_then(|d| fs::read_dir(d).ok()) else {
            return Vec::new();
        };
        let mut out: Vec<(String, usize)> = entries
            .flatten()
            .filter_map(|e| {
                let recent: Vec<_> = read(&e.path())
                    .into_iter()
                    .filter(|(t, ..)| now - t < window)
                    .collect();
                Some((recent.last()?.1.clone(), recent.len()))
            })
            .collect();
        out.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // One test touches the env: the state and cache dirs are process-wide.
    #[test]
    fn calls_and_first_time_use_the_xdg_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        env::set_var("XDG_STATE_HOME", tmp.path().join("state"));
        env::set_var("XDG_CACHE_HOME", tmp.path().join("cache"));

        calls::record("gh", "w1:p2", "pr checks", 1000);
        calls::record("gh", "w1:p2", "pr checks", 1500);
        calls::record("gh", "w1:p2", "pr view", 1500);
        calls::record("gh", "w1:p3", "api user", 4000);
        assert_eq!(calls::count_of("gh", "w1:p2", "pr checks", 600, 1600), 1);
        assert_eq!(calls::count_of("gh", "w1:p2", "pr checks", 3600, 1600), 2);
        assert_eq!(
            calls::per_pane("gh", calls::HOUR, 4100),
            vec![("w1:p2".to_string(), 3), ("w1:p3".to_string(), 1)]
        );

        assert!(first_time(Some("s1"), "gh-poll"));
        assert!(!first_time(Some("s1"), "gh-poll"));
        assert!(first_time(Some("s2"), "gh-poll"));
        assert!(first_time(None, "gh-poll"));
    }

    #[test]
    fn only_pane_ids_are_panes() {
        assert!(is_pane("w3V:p1"));
        for k in [
            "codex:01a12009-494e",
            "01a12009-494e-7831",
            "unknown",
            "w3V:",
            ":p1",
        ] {
            assert!(!is_pane(k), "{k}");
        }
    }

    #[test]
    fn safe_keeps_ids_readable() {
        assert_eq!(safe("w3V:p1"), "w3V_p1");
        assert_eq!(safe("a/b c"), "a_b_c");
    }

    #[test]
    fn resolve_skips_self_and_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let bin = tmp.path().join("faketool");
        fs::write(&bin, "#!/bin/sh\n").unwrap();
        fs::set_permissions(&bin, fs::Permissions::from_mode(0o755)).unwrap();
        let canon = bin.canonicalize().unwrap();
        env::set_var("VEGA_SHIM_TEST_BIN", &bin);
        assert_eq!(
            resolve_real("faketool", Some("VEGA_SHIM_TEST_BIN"), None),
            Some(canon.clone())
        );
        // The env candidate is this shim: skipped; PATH has no faketool.
        assert_eq!(
            resolve_real("faketool", Some("VEGA_SHIM_TEST_BIN"), Some(&canon)),
            None
        );
        let alias = tmp.path().join("hardlink");
        fs::hard_link(&bin, &alias).unwrap();
        assert!(is_self(&alias, Some(&canon)));
        let copy = tmp.path().join("copy");
        fs::copy(&bin, &copy).unwrap();
        env::set_var("VEGA_SHIM_TEST_BIN", &copy);
        // Even a different executable in the shim directory is not a target.
        assert_eq!(
            resolve_real("faketool", Some("VEGA_SHIM_TEST_BIN"), Some(&canon)),
            None
        );
        env::remove_var("VEGA_SHIM_TEST_BIN");
    }
}
