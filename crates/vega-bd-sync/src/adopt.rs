//! `vega bd init --adopt`: moves an existing local beads database (embedded
//! or server mode) to the remote. The local history is pushed to the remote
//! database, `.beads` is backed up, its database parts are removed, and a
//! fresh `bd init --server --remote` clones the history back
//! (docs/design/beads-sync.md, "Adopting an existing database").
//!
//! Force is used once at most: onto a remote database that holds only
//! Dolt's initial commit (just created, or never pushed to). A remote with
//! any other history that is not part of the local one is refused.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use crate::bd::{self, last_line, run};

/// How bd stores the local database (`.beads/metadata.json`: dolt_mode).
#[derive(Debug, Clone, PartialEq)]
pub enum Mode {
    Embedded,
    Server,
}

/// The local database to adopt.
#[derive(Debug, Clone, PartialEq)]
pub struct Local {
    pub mode: Mode,
    /// `.beads`.
    pub beads: PathBuf,
    /// bd's data dir (`bd info`: database_path), a child of `.beads`.
    pub data: PathBuf,
    /// The Dolt repo: `<data>/<dolt_database>`.
    pub repo: PathBuf,
}

/// Reads `metadata.json` next to bd's data dir `data`. Only a local Dolt
/// repo under `.beads` qualifies: shared or external servers do not.
pub fn local(data: &Path) -> Result<Local, String> {
    let beads = data
        .parent()
        .ok_or_else(|| format!("data dir {} has no parent", data.display()))?;
    let meta = beads.join("metadata.json");
    let text = fs::read_to_string(&meta).map_err(|e| format!("{}: {e}", meta.display()))?;
    let v: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| format!("{}: {e}", meta.display()))?;
    let mode = match v["dolt_mode"].as_str().unwrap_or("embedded") {
        "embedded" => Mode::Embedded,
        "server" => Mode::Server,
        other => {
            return Err(format!(
                "dolt_mode `{other}` (only embedded and server are adopted)"
            ))
        }
    };
    if let Some(host) = v["dolt_server_host"].as_str() {
        if !matches!(host, "" | "127.0.0.1" | "localhost" | "::1") {
            return Err(format!("an external Dolt server at {host}"));
        }
    }
    let db = v["dolt_database"]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| format!("{}: no dolt_database", meta.display()))?;
    let repo = data.join(db);
    if !repo.join(".dolt").is_dir() {
        return Err(format!("no Dolt repo at {}", repo.display()));
    }
    Ok(Local {
        mode,
        beads: beads.to_path_buf(),
        data: data.to_path_buf(),
        repo,
    })
}

/// The temporary remote name in the local repo; removed again after the push.
pub const REMOTE: &str = "vega-adopt";

/// What the remote holds relative to the local history.
#[derive(Debug, Clone, PartialEq)]
pub struct Heads {
    pub local: String,
    /// None: the remote database has no `main` branch.
    pub remote: Option<String>,
    pub remote_commits: u64,
    /// The remote head is in the local history (a fast-forward).
    pub remote_in_local: bool,
    /// The local head is in the remote history (pushed before).
    pub local_in_remote: bool,
}

/// How the local history reaches the remote.
#[derive(Debug, Clone, PartialEq)]
pub enum Plan {
    /// The remote already holds the local head (or more on top of it).
    Present,
    /// A plain push.
    Push,
    /// The remote holds only Dolt's initial commit: one force push.
    ForceOntoEmpty,
    /// Foreign history: refuse.
    Diverged,
}

pub fn plan(h: &Heads) -> Plan {
    match &h.remote {
        None => Plan::Push,
        Some(r) if *r == h.local || h.local_in_remote => Plan::Present,
        Some(_) if h.remote_in_local => Plan::Push,
        Some(_) if h.remote_commits <= 1 => Plan::ForceOntoEmpty,
        Some(_) => Plan::Diverged,
    }
}

/// What [`push`] did.
#[derive(Debug, Clone, PartialEq)]
pub struct Pushed {
    pub plan: Plan,
    /// The local head that is now on the remote.
    pub head: String,
}

fn dolt(dolt: &Path, dir: &Path, args: &[&str], secs: u64) -> bd::Run {
    let mut c = Command::new(dolt);
    c.current_dir(dir).args(args);
    run(c, Duration::from_secs(secs))
}

/// One row of `dolt sql -r json`.
fn row(dolt_bin: &Path, dir: &Path, q: &str) -> Result<serde_json::Value, String> {
    let r = dolt(dolt_bin, dir, &["sql", "-r", "json", "-q", q], 60);
    if !r.ok {
        return Err(last_line(&r.out));
    }
    let start = r.out.find('{').ok_or_else(|| last_line(&r.out))?;
    let v: serde_json::Value =
        serde_json::from_str(&r.out[start..]).map_err(|e| format!("dolt sql output: {e}"))?;
    v["rows"]
        .get(0)
        .cloned()
        .ok_or_else(|| "dolt sql returned no row".into())
}

fn num(v: &serde_json::Value) -> u64 {
    v.as_u64()
        .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
        .unwrap_or(0)
}

/// The local head of `main` in `repo`.
pub fn head(dolt_bin: &Path, repo: &Path) -> Result<String, String> {
    row(dolt_bin, repo, "select hashof('main') as h")?["h"]
        .as_str()
        .map(String::from)
        .ok_or_else(|| "no head".into())
}

fn heads(dolt_bin: &Path, repo: &Path) -> Result<Heads, String> {
    let local = head(dolt_bin, repo)?;
    let rref = format!("{REMOTE}/main");
    // A database without main answers `invalid ref spec`.
    let Ok(r) = row(
        dolt_bin,
        repo,
        &format!("select hashof('{rref}') as h, (select count(*) from dolt_log('{rref}')) as n, (select count(*) from dolt_log('main') where commit_hash = hashof('{rref}')) as ril, (select count(*) from dolt_log('{rref}') where commit_hash = hashof('main')) as lir"),
    ) else {
        return Ok(Heads {
            local,
            remote: None,
            remote_commits: 0,
            remote_in_local: false,
            local_in_remote: false,
        });
    };
    Ok(Heads {
        local,
        remote: r["h"].as_str().map(String::from),
        remote_commits: num(&r["n"]),
        remote_in_local: num(&r["ril"]) > 0,
        local_in_remote: num(&r["lir"]) > 0,
    })
}

/// Why [`push`] did not push.
#[derive(Debug, Clone, PartialEq)]
pub enum PushError {
    /// The remote holds foreign history; nothing was pushed.
    Diverged {
        remote_commits: u64,
    },
    Unreachable(String),
    Failed(String),
}

/// Pushes `main` of the local repo to `url` by [`plan`]. The repo gets the
/// temporary remote [`REMOTE`] and loses it again, whatever happens.
pub fn push(dolt_bin: &Path, repo: &Path, url: &str) -> Result<Pushed, PushError> {
    let _ = dolt(dolt_bin, repo, &["remote", "remove", REMOTE], 30);
    let add = dolt(dolt_bin, repo, &["remote", "add", REMOTE, url], 30);
    if !add.ok {
        return Err(PushError::Failed(format!(
            "dolt remote add: {}",
            last_line(&add.out)
        )));
    }
    let result = push_added(dolt_bin, repo);
    let _ = dolt(dolt_bin, repo, &["remote", "remove", REMOTE], 30);
    result
}

fn push_added(dolt_bin: &Path, repo: &Path) -> Result<Pushed, PushError> {
    let fetch_args = bd::remote_args("fetch", &[REMOTE]);
    let fetch = dolt(
        dolt_bin,
        repo,
        &fetch_args.iter().map(String::as_str).collect::<Vec<_>>(),
        600,
    );
    if !fetch.ok {
        return Err(match bd::classify(&fetch) {
            bd::Outcome::Unreachable(m) => PushError::Unreachable(m),
            _ => PushError::Failed(format!("dolt fetch: {}", last_line(&fetch.out))),
        });
    }
    let h = heads(dolt_bin, repo).map_err(PushError::Failed)?;
    let p = plan(&h);
    let args: Vec<String> = match p {
        Plan::Present => {
            return Ok(Pushed {
                plan: p,
                head: h.local,
            })
        }
        Plan::Diverged => {
            return Err(PushError::Diverged {
                remote_commits: h.remote_commits,
            })
        }
        Plan::Push => bd::remote_args("push", &[REMOTE, "main"]),
        Plan::ForceOntoEmpty => bd::remote_args("push", &["--force", REMOTE, "main"]),
    };
    let r = dolt(
        dolt_bin,
        repo,
        &args.iter().map(String::as_str).collect::<Vec<_>>(),
        1800,
    );
    match bd::classify(&r) {
        bd::Outcome::Ok => Ok(Pushed {
            plan: p,
            head: h.local,
        }),
        bd::Outcome::Unreachable(m) => Err(PushError::Unreachable(m)),
        bd::Outcome::Behind(m)
        | bd::Outcome::Conflict(m)
        | bd::Outcome::Retry(m)
        | bd::Outcome::Failed(m) => Err(PushError::Failed(format!("dolt push: {m}"))),
    }
}

/// Copies `.beads` to `dest` (which must not exist), modes and times kept.
pub fn backup(beads: &Path, dest: &Path) -> Result<(), String> {
    if dest.exists() {
        return Err(format!("{} exists", dest.display()));
    }
    if let Some(p) = dest.parent() {
        fs::create_dir_all(p).map_err(|e| format!("{}: {e}", p.display()))?;
    }
    let mut c = Command::new("cp");
    c.arg("-pR").arg(beads).arg(dest);
    let r = run(c, Duration::from_secs(600));
    if r.ok {
        Ok(())
    } else {
        Err(format!("cp: {}", last_line(&r.out)))
    }
}

/// The files a bd database consists of besides its data dir: metadata,
/// locks, the managed server's config and runtime files.
const DB_FILES: &[&str] = &[
    "metadata.json",
    "embeddeddolt.gate.lock",
    "dolt.gate.lock",
    "dolt-server-config.yaml",
    "dolt-server.lock",
    "dolt-server.log",
    "dolt-server.pid",
    "dolt-server.port",
];

/// Removes the database from `.beads` (data dir and [`DB_FILES`]); the rest
/// (config.yaml, PRIME.md, README.md, .gitignore, backups) stays. Returns
/// the names removed.
pub fn clear(l: &Local) -> Result<Vec<String>, String> {
    let mut removed = Vec::new();
    fs::remove_dir_all(&l.data).map_err(|e| format!("{}: {e}", l.data.display()))?;
    removed.push(
        l.data
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
    );
    for f in DB_FILES {
        let p = l.beads.join(f);
        if p.exists() {
            fs::remove_file(&p).map_err(|e| format!("{}: {e}", p.display()))?;
            removed.push((*f).to_string());
        }
    }
    Ok(removed)
}

/// Whether a bd `config.yaml` sets anything (a line that is not a comment).
pub fn has_settings(yaml: &str) -> bool {
    yaml.lines().any(|l| {
        let t = l.trim();
        !t.is_empty() && !t.starts_with('#')
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bd::testsrv;

    fn heads(remote: Option<&str>, n: u64, ril: bool, lir: bool) -> Heads {
        Heads {
            local: "L".into(),
            remote: remote.map(String::from),
            remote_commits: n,
            remote_in_local: ril,
            local_in_remote: lir,
        }
    }

    #[test]
    fn plans_force_only_onto_an_empty_database() {
        assert_eq!(plan(&heads(None, 0, false, false)), Plan::Push);
        assert_eq!(plan(&heads(Some("L"), 5, true, true)), Plan::Present);
        assert_eq!(plan(&heads(Some("R"), 9, false, true)), Plan::Present);
        assert_eq!(plan(&heads(Some("R"), 3, true, false)), Plan::Push);
        assert_eq!(
            plan(&heads(Some("R"), 1, false, false)),
            Plan::ForceOntoEmpty
        );
        assert_eq!(plan(&heads(Some("R"), 2, false, false)), Plan::Diverged);
    }

    #[test]
    fn reads_the_mode_from_metadata() {
        let t = tempfile::tempdir().unwrap();
        let beads = t.path().join(".beads");
        let data = beads.join("embeddeddolt");
        fs::create_dir_all(data.join("x/.dolt")).unwrap();
        let meta = |s: &str| fs::write(beads.join("metadata.json"), s).unwrap();
        meta(r#"{"dolt_mode":"embedded","dolt_database":"x"}"#);
        let l = local(&data).unwrap();
        assert_eq!(l.mode, Mode::Embedded);
        assert_eq!(l.repo, data.join("x"));
        meta(r#"{"dolt_mode":"server","dolt_database":"x","dolt_server_host":"10.0.0.1"}"#);
        assert!(local(&data).unwrap_err().contains("external"));
        meta(r#"{"dolt_mode":"shared-server","dolt_database":"x"}"#);
        assert!(local(&data).is_err());
        meta(r#"{"dolt_mode":"server","dolt_database":"y"}"#);
        assert!(local(&data).unwrap_err().contains("no Dolt repo"));

        fs::write(beads.join("PRIME.md"), "keep").unwrap();
        fs::write(beads.join("dolt-server.pid"), "1").unwrap();
        meta(r#"{"dolt_mode":"embedded","dolt_database":"x"}"#);
        let l = local(&data).unwrap();
        let removed = clear(&l).unwrap();
        assert_eq!(
            removed,
            ["embeddeddolt", "metadata.json", "dolt-server.pid"]
        );
        assert!(beads.join("PRIME.md").is_file() && !data.exists());
    }

    #[test]
    fn config_settings() {
        assert!(!has_settings("# a: b\n\n  # c\n"));
        assert!(has_settings("# x\nno-git-ops: true\n"));
    }

    /// A Dolt repo in `dir` with `commits` commits on top of the initial one.
    fn repo(dolt_bin: &Path, dir: &Path, commits: u32) {
        fs::create_dir_all(dir).unwrap();
        let ok = |args: &[&str]| {
            let r = dolt(dolt_bin, dir, args, 60);
            assert!(r.ok, "dolt {args:?}: {}", r.out);
        };
        ok(&["init", "--name", "t", "--email", "t@localhost"]);
        ok(&["sql", "-q", "create table t (i int primary key)"]);
        for i in 0..commits {
            ok(&["sql", "-q", &format!("insert into t values ({i})")]);
            ok(&["add", "-A"]);
            ok(&["commit", "-m", &format!("c{i}")]);
        }
    }

    /// Against a local sql-server with remotesapi: force onto a freshly
    /// created database, then a fast-forward, a no-op, and a refusal of
    /// foreign history.
    #[test]
    fn pushes_into_a_remote_database() {
        let Some(dolt_bin) = testsrv::dolt() else {
            eprintln!("skipped: no dolt on PATH");
            return;
        };
        let s = testsrv::server(&dolt_bin);
        let t = tempfile::tempdir().unwrap();
        s.root_sql(
            &dolt_bin,
            "CREATE DATABASE `adopt-hyphen-2`; CREATE DATABASE OTHER",
        );
        let url = format!("{}/adopt-hyphen-2", s.remote);

        let a = t.path().join("a");
        repo(&dolt_bin, &a, 2);
        let p = push(&dolt_bin, &a, &url).unwrap();
        assert_eq!(p.plan, Plan::ForceOntoEmpty);
        assert_eq!(p.head, head(&dolt_bin, &a).unwrap());
        let r = dolt(&dolt_bin, &a, &["remote"], 10);
        assert!(!r.out.contains(REMOTE), "temporary remote left: {}", r.out);

        assert_eq!(push(&dolt_bin, &a, &url).unwrap().plan, Plan::Present);
        let r = dolt(
            &dolt_bin,
            &a,
            &["sql", "-q", "insert into t values (99)"],
            30,
        );
        assert!(r.ok, "{}", r.out);
        let r = dolt(&dolt_bin, &a, &["commit", "-Am", "more"], 30);
        assert!(r.ok, "{}", r.out);
        assert_eq!(push(&dolt_bin, &a, &url).unwrap().plan, Plan::Push);

        // Another repo's history on the same database: refused, untouched.
        let b = t.path().join("b");
        repo(&dolt_bin, &b, 1);
        assert!(matches!(
            push(&dolt_bin, &b, &url),
            Err(PushError::Diverged { .. })
        ));
        assert_eq!(push(&dolt_bin, &a, &url).unwrap().plan, Plan::Present);

        // A database someone else pushed to (more than the initial commit).
        let other = format!("{}/OTHER", s.remote);
        assert_eq!(
            push(&dolt_bin, &b, &other).unwrap().plan,
            Plan::ForceOntoEmpty
        );
        assert!(matches!(
            push(&dolt_bin, &a, &other),
            Err(PushError::Diverged { .. })
        ));
        assert!(matches!(
            push(&dolt_bin, &a, "http://127.0.0.1:1/X"),
            Err(PushError::Unreachable(_))
        ));
    }

    #[test]
    fn backup_copies_and_refuses_an_existing_target() {
        let t = tempfile::tempdir().unwrap();
        let beads = t.path().join(".beads");
        fs::create_dir_all(beads.join("dolt/x")).unwrap();
        fs::write(beads.join("dolt/x/f"), "data").unwrap();
        let dest = t.path().join("bk/1/.beads");
        backup(&beads, &dest).unwrap();
        assert_eq!(fs::read_to_string(dest.join("dolt/x/f")).unwrap(), "data");
        assert!(backup(&beads, &dest).is_err());
    }
}
