//! Running bd (and dolt) for a project: a timeout, the progress spinner
//! stripped from the output, and the outcome of a pull or push classified.

use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use crate::store::Project;

/// The output of a finished (or killed) command: success and the cleaned
/// stdout plus stderr.
#[derive(Debug, Clone)]
pub struct Run {
    pub ok: bool,
    pub out: String,
    pub timed_out: bool,
}

/// Runs `cmd` with stdin closed; kills it after `limit`.
pub fn run(mut cmd: Command, limit: Duration) -> Run {
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            return Run {
                ok: false,
                out: e.to_string(),
                timed_out: false,
            }
        }
    };
    // Drain both pipes on threads: a chatty child must not block on a full pipe.
    let drain = |r: Option<Box<dyn Read + Send>>| {
        thread::spawn(move || {
            let mut buf = Vec::new();
            if let Some(mut r) = r {
                let _ = r.read_to_end(&mut buf);
            }
            buf
        })
    };
    let out = drain(
        child
            .stdout
            .take()
            .map(|r| Box::new(r) as Box<dyn Read + Send>),
    );
    let err = drain(
        child
            .stderr
            .take()
            .map(|r| Box::new(r) as Box<dyn Read + Send>),
    );
    let start = Instant::now();
    let (ok, timed_out) = loop {
        match child.try_wait() {
            Ok(Some(st)) => break (st.success(), false),
            Ok(None) if start.elapsed() >= limit => {
                let _ = child.kill();
                let _ = child.wait();
                break (false, true);
            }
            Ok(None) => thread::sleep(Duration::from_millis(50)),
            Err(_) => break (false, false),
        }
    };
    let mut text = String::from_utf8_lossy(&out.join().unwrap_or_default()).into_owned();
    text.push('\n');
    text.push_str(&String::from_utf8_lossy(&err.join().unwrap_or_default()));
    Run {
        ok,
        out: clean(&text),
        timed_out,
    }
}

/// Drops dolt's progress spinner (`- Fetching...\ Fetching...`, joined by
/// carriage returns, or by backspaces in dolt 2.1) and blank lines.
pub fn clean(text: &str) -> String {
    text.replace('\u{8}', "")
        .split('\n')
        .map(|line| {
            let last = line.rsplit('\r').next().unwrap_or(line);
            strip_spinner(last).trim().to_string()
        })
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

fn strip_spinner(mut s: &str) -> &str {
    loop {
        let t = s.trim_start();
        let mut chars = t.chars();
        let spin =
            matches!(chars.next(), Some('-' | '\\' | '|' | '/')) && chars.next() == Some(' ');
        if !spin {
            return t;
        }
        // `- Fetching...` up to and including its `...`.
        match t.find("...") {
            Some(i) => s = &t[i + 3..],
            None => return t,
        }
    }
}

/// The last line of `out` that says something: the error, usually.
pub fn last_line(out: &str) -> String {
    let line = out.lines().last().unwrap_or("").trim();
    line.chars().take(300).collect()
}

/// bd's pre-push integrity check (fsck) limit. bd's default is too short for
/// a store near 1 GB (spellkave, 894 MB: `fsck did not complete within the
/// configured timeout`). A `BEADS_FSCK_TIMEOUT` in vegad's env wins.
pub const FSCK_TIMEOUT: &str = "5m";

/// A bd command for project `p`, with vegad's actor. It runs in the checkout
/// `vega bd init` ran in; once that worktree is removed, in the dir that
/// holds `.beads` (bd puts it next to the git common dir).
pub fn bd(p: &Project, actor: &str) -> Command {
    let dir = if p.dir.is_dir() {
        p.dir.as_path()
    } else {
        p.beads_dir().and_then(Path::parent).unwrap_or(&p.dir)
    };
    let mut c = Command::new(&p.bd);
    c.current_dir(dir).env("BEADS_ACTOR", actor);
    if std::env::var_os("BEADS_FSCK_TIMEOUT").is_none() {
        c.env("BEADS_FSCK_TIMEOUT", FSCK_TIMEOUT);
    }
    c
}

/// What a pull or push came to.
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    Ok,
    /// A merge conflict: bd aborted the merge; an operator resolves it.
    Conflict(String),
    /// A push the remote rejected as non-fast-forward: pull first, retry.
    Behind(String),
    /// The remote did not answer: retry on the next interval.
    Unreachable(String),
    /// No conflict, a transient failure (bd's auto-commit raced an agent's
    /// write, a peer pushed after bd's fetch, bd restarted its local server
    /// on a new port, a remote's HTTP 400): try once more now (a
    /// [`fast_forwardable`] one by [`FAST_FORWARD_SQL`]), then on the next
    /// interval; never a stop.
    Retry(String),
    /// Anything else (a missing database, a rejected push): stop.
    Failed(String),
}

/// Classifies a finished pull or push by bd's and dolt's messages
/// (bd 1.3.1, dolt 2.4.0; observed in docs/design/beads-sync.md).
pub fn classify(r: &Run) -> Outcome {
    if r.ok {
        return Outcome::Ok;
    }
    let msg = last_line(&r.out);
    let lower = r.out.to_lowercase();
    if r.timed_out {
        Outcome::Unreachable(format!("timed out: {msg}"))
    } else if lower.contains("merge conflict") || lower.contains("conflicts in") {
        Outcome::Conflict(msg)
    } else if lower.contains("cannot merge with uncommitted changes")
        || lower.contains("http status: 400")
        || fast_forwardable(&lower)
        || lower.contains("invalid connection")
        || lower.contains("bad connection")
    {
        Outcome::Retry(msg)
    } else if lower.contains("tip of your current branch is behind")
        || lower.contains("non-fast-forward")
    {
        Outcome::Behind(msg)
    } else if lower.contains("code = unavailable")
        || lower.contains("code = deadlineexceeded")
        || lower.contains("connection refused")
        || lower.contains("no such host")
        || lower.contains("i/o timeout")
    {
        Outcome::Unreachable(msg)
    } else {
        Outcome::Failed(msg)
    }
}

/// bd's pull merged nothing, but the local branch is behind the remote tip
/// (a peer pushed after bd's fetch; vega-lhc.130): a fast-forward fixes it.
pub fn fast_forwardable(msg: &str) -> bool {
    msg.to_lowercase().contains("fast-forwardable")
}

/// The fast-forward a [`fast_forwardable`] pull needs, through bd's server
/// (`dolt pull origin main` by hand, without restarting the server); it
/// fails rather than merge when the branches diverged.
pub const FAST_FORWARD_SQL: &str = "CALL DOLT_PULL('origin', 'main', '--ff-only')";

/// Ahead, behind (against the remote-tracking `origin/main`, as of the last
/// fetch) and the head commit.
#[derive(Debug, Clone, PartialEq)]
pub struct Counts {
    pub ahead: u64,
    pub behind: u64,
    pub head: String,
}

pub const COUNTS_SQL: &str = "select (select count(*) from dolt_log('remotes/origin/main..main')) as ahead, (select count(*) from dolt_log('main..remotes/origin/main')) as behind, hashof('main') as head";

/// Parses `bd sql --json` output of [`COUNTS_SQL`].
pub fn parse_counts(out: &str) -> Option<Counts> {
    let start = out.find('[')?;
    let v: serde_json::Value = serde_json::from_str(&out[start..]).ok()?;
    let row = v.get(0)?;
    let num = |k: &str| {
        row.get(k)
            .and_then(|x| x.as_u64().or_else(|| x.as_str()?.parse().ok()))
    };
    Some(Counts {
        ahead: num("ahead")?,
        behind: num("behind")?,
        head: row.get("head")?.as_str()?.to_string(),
    })
}

/// Whether the remote holds `database`, without downloading it: a fetch of
/// a branch that does not exist into a throwaway repo. An existing database
/// answers `invalid ref spec`; a missing one `database not found`.
#[derive(Debug, Clone, PartialEq)]
pub enum Probe {
    Exists,
    Missing,
    Unreachable(String),
    Unknown(String),
}

/// The user for dolt remote calls, `DOLT_REMOTE_USER` when set: dolt only reads the password,
/// `DOLT_REMOTE_PASSWORD`, from the environment and defaults the user to `root`.
pub fn remote_user() -> Option<String> {
    std::env::var("DOLT_REMOTE_USER")
        .ok()
        .filter(|u| !u.is_empty())
}

/// Arguments for a `dolt` subcommand that talks to a remote: `--user <user>` when given, then
/// `rest`.
pub fn remote_args(user: Option<&str>, sub: &str, rest: &[&str]) -> Vec<String> {
    let mut a = vec![sub.to_string()];
    if let Some(u) = user {
        a.extend(["--user".to_string(), u.to_string()]);
    }
    a.extend(rest.iter().map(|r| r.to_string()));
    a
}

/// `user`: see [`remote_user`].
pub fn probe(dolt: &Path, url: &str, scratch: &Path, user: Option<&str>) -> Probe {
    let init = {
        let mut c = Command::new(dolt);
        c.current_dir(scratch)
            .args(["init", "--name", "vega", "--email", "vega@localhost"]);
        run(c, Duration::from_secs(30))
    };
    if !init.ok {
        return Probe::Unknown(format!("dolt init: {}", last_line(&init.out)));
    }
    let mut add = Command::new(dolt);
    add.current_dir(scratch)
        .args(["remote", "add", "probe", url]);
    let add = run(add, Duration::from_secs(10));
    if !add.ok {
        return Probe::Unknown(format!("dolt remote add: {}", last_line(&add.out)));
    }
    let mut fetch = Command::new(dolt);
    fetch.current_dir(scratch).args(remote_args(
        user,
        "fetch",
        &[
            "probe",
            "refs/heads/vega-probe-none:refs/remotes/probe/none",
        ],
    ));
    probe_result(&run(fetch, Duration::from_secs(60)))
}

pub fn probe_result(r: &Run) -> Probe {
    let lower = r.out.to_lowercase();
    if lower.contains("invalid ref spec") || r.ok {
        Probe::Exists
    } else if lower.contains("database not found") {
        Probe::Missing
    } else {
        match classify(r) {
            Outcome::Unreachable(m) => Probe::Unreachable(m),
            _ => Probe::Unknown(last_line(&r.out)),
        }
    }
}

/// The URL/path-safe subset of Dolt database names `vega bd` accepts.
/// Hyphens require SQL identifier quoting; `create_database` uses backticks.
pub fn valid_database(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// What `CREATE DATABASE` on the remote's SQL port came to.
#[derive(Debug, Clone, PartialEq)]
pub enum Create {
    Ok,
    /// Wrong password or a user without the CREATE grant.
    Denied(String),
    Unreachable(String),
    Failed(String),
}

/// Creates `database` on the remote's SQL server (`host:port`) as `user`:
/// `CREATE DATABASE IF NOT EXISTS`, the only statement vega sends there, so
/// an existing database (another clone won the race) is left untouched. The
/// password reaches `dolt sql` through `DOLT_CLI_PASSWORD`, never argv; dolt's
/// messages do not echo it. Runs in `dir` (any dir; no repo needed).
pub fn create_database(
    dolt: &Path,
    endpoint: &str,
    user: &str,
    password: &str,
    database: &str,
    dir: &Path,
) -> Create {
    if !valid_database(database) {
        return Create::Failed(format!("database `{database}` is not a plain name"));
    }
    let Some((host, port)) = endpoint.rsplit_once(':') else {
        return Create::Failed(format!("SQL endpoint `{endpoint}` is not host:port"));
    };
    let mut c = Command::new(dolt);
    c.current_dir(dir)
        .env("DOLT_CLI_PASSWORD", password)
        .args(["--host", host, "--port", port, "--user", user])
        // titan's sql-server has no TLS; the tailnet encrypts.
        .arg("--no-tls")
        .args(["sql", "-q"])
        .arg(format!("CREATE DATABASE IF NOT EXISTS `{database}`"));
    create_result(&run(c, Duration::from_secs(60)))
}

pub fn create_result(r: &Run) -> Create {
    if r.ok {
        return Create::Ok;
    }
    let msg = last_line(&r.out);
    let lower = r.out.to_lowercase();
    if lower.contains("access denied") || lower.contains("command denied") {
        Create::Denied(msg)
    } else if r.timed_out
        || lower.contains("connection refused")
        || lower.contains("no such host")
        || lower.contains("i/o timeout")
    {
        Create::Unreachable(msg)
    } else {
        Create::Failed(msg)
    }
}

/// A local dolt sql-server with remotesapi for tests (as on titan).
#[cfg(test)]
pub(crate) mod testsrv {
    use super::run;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process::{Command, Stdio};
    use std::thread;
    use std::time::{Duration, Instant};

    /// dolt on PATH, if any: tests that need it skip without it.
    pub fn dolt() -> Option<PathBuf> {
        std::env::var_os("PATH").and_then(|p| {
            std::env::split_paths(&p)
                .map(|d| d.join("dolt"))
                .find(|p| p.is_file())
        })
    }

    /// An executable `dolt` script in `dir` running `body`. It is run once before it is
    /// returned, retrying ETXTBSY: a fork in a parallel test may still hold the write fd.
    pub fn fake_dolt(dir: &Path, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let p = dir.join("dolt");
        fs::write(
            &p,
            format!("#!/bin/sh\n[ \"$1\" = --warmup ] && exit 0\n{body}\n"),
        )
        .unwrap();
        fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
        for _ in 0..50 {
            match Command::new(&p).arg("--warmup").status() {
                Err(e) if e.raw_os_error() == Some(26) => thread::sleep(Duration::from_millis(20)),
                r => {
                    assert!(r.unwrap().success());
                    return p;
                }
            }
        }
        panic!("{}: ETXTBSY", p.display());
    }

    /// A local dolt sql-server with remotesapi, killed on drop.
    pub struct Server {
        child: std::process::Child,
        pub sql: String,
        pub remote: String,
        _dir: tempfile::TempDir,
    }

    impl Drop for Server {
        fn drop(&mut self) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }

    fn free_port() -> u16 {
        std::net::TcpListener::bind("127.0.0.1:0")
            .and_then(|l| l.local_addr())
            .map(|a| a.port())
            .unwrap()
    }

    pub fn server(dolt: &Path) -> Server {
        // A user's `dolt creds` (global config `user.creds`) make the client send a bearer token,
        // which the test server rejects: run every dolt of the tests with an empty dolt home.
        static HOME: std::sync::OnceLock<tempfile::TempDir> = std::sync::OnceLock::new();
        let home = HOME.get_or_init(|| tempfile::tempdir().unwrap());
        std::env::set_var("DOLT_ROOT_PATH", home.path());
        let dir = tempfile::tempdir().unwrap();
        let (sql, api) = (free_port(), free_port());
        std::fs::write(
            dir.path().join("config.yaml"),
            format!("listener:\n  host: 127.0.0.1\n  port: {sql}\nremotesapi:\n  port: {api}\n"),
        )
        .unwrap();
        let child = Command::new(dolt)
            .current_dir(dir.path())
            .args(["sql-server", "--config", "config.yaml"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let s = Server {
            child,
            sql: format!("127.0.0.1:{sql}"),
            remote: format!("http://127.0.0.1:{api}"),
            _dir: dir,
        };
        let start = Instant::now();
        while std::net::TcpStream::connect(&s.sql).is_err() {
            assert!(
                start.elapsed() < Duration::from_secs(20),
                "sql-server did not start"
            );
            thread::sleep(Duration::from_millis(100));
        }
        s
    }

    impl Server {
        /// Runs `q` as root on the server's SQL port.
        pub fn root_sql(&self, dolt: &Path, q: &str) {
            let tmp = tempfile::tempdir().unwrap();
            let mut c = Command::new(dolt);
            c.current_dir(tmp.path())
                .env("DOLT_CLI_PASSWORD", "")
                .args(["--host", "127.0.0.1", "--port"])
                .arg(self.sql.rsplit_once(':').unwrap().1)
                .args(["--user", "root", "--no-tls", "sql", "-q", q]);
            let r = run(c, Duration::from_secs(30));
            assert!(r.ok, "{}", r.out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testsrv;
    use super::*;
    use std::fs;

    fn failed(out: &str) -> Run {
        Run {
            ok: false,
            out: clean(out),
            timed_out: false,
        }
    }

    #[test]
    fn clean_drops_backspace_spinners() {
        let raw = "- Fetching...\u{8}\u{8}\u{8}\u{8}\u{8}\u{8}\u{8}\u{8}\u{8}\u{8}\u{8}\u{8}\u{8}\\ Fetching...\u{8}\u{8}\u{8}\u{8}\u{8}\u{8}\u{8}\u{8}\u{8}\u{8}\u{8}\u{8}\u{8}database not found";
        assert_eq!(clean(raw), "database not found");
    }

    #[test]
    fn clean_drops_the_spinner() {
        let raw = "- Fetching...\r\\ Fetching...\r| Fetching...could not access dolt url 'http://h/X': rpc error: code = Unavailable desc = x\n";
        assert_eq!(
            clean(raw),
            "could not access dolt url 'http://h/X': rpc error: code = Unavailable desc = x"
        );
        assert_eq!(clean("- Uploading...- Uploading...done\n\n"), "done");
    }

    #[test]
    fn classifies_observed_messages() {
        let conflict = failed("Pulling from Dolt remote...\nError: failed to pull from origin/main: merge conflicts in issues require operator resolution; merge aborted and working set restored");
        assert!(matches!(classify(&conflict), Outcome::Conflict(_)));
        let behind = failed("hint: Updates were rejected because the tip of your current branch is behind\nhint: its remote counterpart.: exit status 1");
        assert!(matches!(classify(&behind), Outcome::Behind(_)));
        let down = failed("could not access dolt url 'http://127.0.0.1:1/X': rpc error: code = Unavailable desc = connection error");
        assert!(matches!(classify(&down), Outcome::Unreachable(_)));
        let missing = failed("could not access dolt url 'http://h/X': rpc error: code = Unknown desc = database not found: X");
        assert!(matches!(classify(&missing), Outcome::Failed(_)));
        let timeout = Run {
            ok: false,
            out: "x".into(),
            timed_out: true,
        };
        assert!(matches!(classify(&timeout), Outcome::Unreachable(_)));
    }

    /// vega-lhc.111: an agent's bd create between bd's auto-commit and its
    /// merge, and a transient remote 400; neither is a conflict.
    #[test]
    fn classifies_transient_pull_failures() {
        let dirty =
            failed("Error: failed to pull from origin/main: cannot merge with uncommitted changes");
        assert!(matches!(classify(&dirty), Outcome::Retry(_)));
        let http = failed("Error: failed to pull from origin/main: http status: 400");
        assert!(matches!(classify(&http), Outcome::Retry(_)));
    }

    /// vega-lhc.130, the recorded messages: a peer's push after bd's fetch
    /// (ganymede), and bd's local server restarted on a new port (proteus).
    #[test]
    fn classifies_self_healing_pull_failures() {
        let tip = failed("Error: pull merged nothing but the local branch is behind a fast-forwardable remote tip");
        assert!(matches!(classify(&tip), Outcome::Retry(ref m) if fast_forwardable(m)));
        let conn = failed("Error: failed to pull from origin/main: invalid connection");
        assert!(matches!(classify(&conn), Outcome::Retry(ref m) if !fast_forwardable(m)));
    }

    /// [`FAST_FORWARD_SQL`] in a clone behind its remote: at the remote's
    /// tip after it, as the manual `dolt pull origin main` was.
    #[test]
    fn fast_forward_sql_catches_up() {
        let Some(dolt) = testsrv::dolt() else {
            eprintln!("skipped: no dolt on PATH");
            return;
        };
        let s = testsrv::server(&dolt);
        s.root_sql(&dolt, "CREATE DATABASE FF");
        let tmp = tempfile::tempdir().unwrap();
        let url = format!("{}/FF", s.remote);
        let sh = |dir: &Path, args: &[&str]| {
            let mut c = Command::new(&dolt);
            c.current_dir(dir).args(args);
            let r = run(c, Duration::from_secs(60));
            assert!(r.ok, "dolt {args:?}: {}", r.out);
            r.out
        };
        sh(tmp.path(), &["clone", &url, "seed"]);
        let seed = tmp.path().join("seed");
        sh(&seed, &["sql", "-q", "create table t (id int primary key)"]);
        sh(&seed, &["commit", "-Am", "one", "--author", "t <t@t>"]);
        sh(&seed, &["push", "origin", "main"]);
        sh(tmp.path(), &["clone", &url, "behind"]);
        sh(&seed, &["sql", "-q", "insert into t values (1)"]);
        sh(&seed, &["commit", "-Am", "two", "--author", "t <t@t>"]);
        sh(&seed, &["push", "origin", "main"]);
        let behind = tmp.path().join("behind");
        let head = |dir: &Path| sh(dir, &["sql", "-r", "csv", "-q", "select hashof('main')"]);
        assert_ne!(head(&behind), head(&seed));
        sh(&behind, &["sql", "-q", FAST_FORWARD_SQL]);
        assert_eq!(head(&behind), head(&seed));
    }

    #[test]
    fn remote_args_add_the_user_only_when_given() {
        assert_eq!(remote_args(None, "fetch", &["probe"]), ["fetch", "probe"]);
        assert_eq!(
            remote_args(Some("goern"), "push", &["--force", "r", "main"]),
            ["push", "--user", "goern", "--force", "r", "main"]
        );
    }

    #[test]
    fn probe_fetches_as_the_user() {
        let t = tempfile::tempdir().unwrap();
        let log = t.path().join("log");
        let dolt = testsrv::fake_dolt(t.path(), &format!("echo \"$*\" >>'{}'", log.display()));
        let scratch = t.path().join("scratch");
        fs::create_dir_all(&scratch).unwrap();
        assert_eq!(
            probe(&dolt, "http://r/db", &scratch, Some("goern")),
            Probe::Exists
        );
        let calls = fs::read_to_string(&log).unwrap();
        assert!(
            calls.contains("fetch --user goern probe refs/heads/vega-probe-none:"),
            "{calls}"
        );
    }

    #[test]
    fn probe_reads_the_fetch_error() {
        assert_eq!(
            probe_result(&failed("fetch failed: invalid ref spec: 'vega-probe-none'")),
            Probe::Exists
        );
        assert_eq!(
            probe_result(&failed(
                "rpc error: code = Unknown desc = database not found: NOPE"
            )),
            Probe::Missing
        );
        assert!(matches!(
            probe_result(&failed(
                "rpc error: code = Unavailable desc = name resolver error"
            )),
            Probe::Unreachable(_)
        ));
    }

    #[test]
    fn parses_counts() {
        let out = "[\n  {\n    \"ahead\": 2,\n    \"behind\": 1,\n    \"head\": \"jfej1\"\n  }\n]";
        assert_eq!(
            parse_counts(out),
            Some(Counts {
                ahead: 2,
                behind: 1,
                head: "jfej1".into()
            })
        );
        assert_eq!(parse_counts("Error: no"), None);
    }

    #[test]
    fn create_reads_dolts_errors() {
        let denied = failed("failed to connect to the dolt sql-server at h:3306: Error 1045 (28000): Access denied for user 'vega'");
        assert!(matches!(create_result(&denied), Create::Denied(_)));
        let down = failed("failed to connect to the dolt sql-server at h:1: dial tcp h:1: connect: connection refused");
        assert!(matches!(create_result(&down), Create::Unreachable(_)));
    }

    #[test]
    fn database_names_are_safe_for_sql_urls_and_local_paths() {
        for name in ["RALLY_NG2", "oparl-lite-2", "123", "select"] {
            assert!(valid_database(name), "{name}");
        }
        for name in [
            "",
            "X`; SELECT 1; --",
            "a/b",
            "..",
            "a b",
            "a?b",
            "a#b",
            "a%b",
            "a\\b",
            "a`b",
            "a\nb",
        ] {
            assert!(!valid_database(name), "{name:?}");
        }
    }

    /// The init flow's remote half against a local server whose `vega` user
    /// holds only CREATE, as on titan: missing, created, then present.
    #[test]
    fn creates_a_missing_database_with_create_only() {
        let Some(dolt) = testsrv::dolt() else {
            eprintln!("skipped: no dolt on PATH");
            return;
        };
        let s = testsrv::server(&dolt);
        let tmp = tempfile::tempdir().unwrap();
        s.root_sql(
            &dolt,
            "CREATE USER 'vega'@'%' IDENTIFIED BY 'pw-test'; GRANT CREATE ON *.* TO 'vega'@'%';",
        );

        let database = "oparl-lite-2";
        let url = format!("{}/{database}", s.remote);
        let probe_in = |name: &str| {
            let d = tmp.path().join(name);
            std::fs::create_dir_all(&d).unwrap();
            probe(&dolt, &url, &d, None)
        };
        assert_eq!(probe_in("p1"), Probe::Missing);
        let create = |pw: &str| create_database(&dolt, &s.sql, "vega", pw, database, tmp.path());
        assert!(matches!(create("wrong"), Create::Denied(_)));
        assert_eq!(create("pw-test"), Create::Ok);
        assert_eq!(probe_in("p2"), Probe::Exists);
        // Local and remote names stay identical: no underscore mapping is needed.
        let mut clone = Command::new(&dolt);
        clone
            .current_dir(tmp.path())
            .args(["clone", &url, database]);
        let cloned = run(clone, Duration::from_secs(60));
        assert!(cloned.ok, "{}", cloned.out);
        assert!(tmp.path().join(database).join(".dolt").is_dir());
        // Again (another clone raced us): untouched, still Ok.
        assert_eq!(create("pw-test"), Create::Ok);
        assert!(matches!(
            create_database(
                &dolt,
                "127.0.0.1:1",
                "vega",
                "pw-test",
                database,
                tmp.path()
            ),
            Create::Unreachable(_)
        ));
    }

    #[test]
    fn run_times_out() {
        let mut c = Command::new("sleep");
        c.arg("5");
        let r = run(c, Duration::from_millis(200));
        assert!(r.timed_out && !r.ok);
    }
}
