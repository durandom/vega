//! `.vega/schedule.toml`: repo-bound jobs, parsed strictly (an unknown key
//! fails the whole file: `daly` must not mean "never"), and the hash a host
//! approves.

use std::collections::{BTreeMap, BTreeSet};

use jiff::SignedDuration;
use sha2::{Digest, Sha256};
use toml_edit::{DocumentMut, Item, TableLike, Value};

use crate::when::{parse_duration, When};
use crate::Error;

pub const DEFAULT_TIMEOUT: SignedDuration = SignedDuration::from_hours(1);
pub const MAX_TIMEOUT: SignedDuration = SignedDuration::from_hours(24);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Notify {
    Failure,
    Always,
    Never,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Job {
    /// `[a-z0-9-]+`, unique in the file.
    pub name: String,
    /// A repo-relative file ([`repo_path`]); never looked up on PATH.
    pub run: String,
    pub args: Vec<String>,
    /// Daily or weekly, never once.
    pub when: When,
    pub hosts: Vec<String>,
    pub timeout: SignedDuration,
    pub notify: Notify,
    /// Further repo-relative files whose blobs enter the hash.
    pub pin: Vec<String>,
    pub catchup: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Schedule {
    pub jobs: Vec<Job>,
}

fn invalid(msg: impl Into<String>) -> Error {
    Error::new("schedule_invalid", msg)
}

/// The path rule for `run` and `pin`: relative to the repo root, `/`
/// separated, no absolute path, no `..`, `.` or empty component. Whether
/// the file exists, is executable, stays inside the repo after symlinks and
/// matches HEAD is the runner's check.
pub fn repo_path(p: &str) -> Result<(), Error> {
    let bad = |why: &str| Err(Error::new("path_invalid", format!("{p:?}: {why}")));
    if p.is_empty() {
        return bad("empty");
    }
    if p.starts_with('/') {
        return bad("absolute; want a path relative to the repo root");
    }
    if p.contains('\\') || p.contains('\0') {
        return bad("want a `/` separated path");
    }
    for c in p.split('/') {
        match c {
            "" => return bad("empty path component"),
            ".." => return bad("`..` leaves the repo root"),
            "." => return bad("drop the `.` component"),
            _ => {}
        }
    }
    Ok(())
}

fn valid_name(n: &str) -> bool {
    !n.is_empty()
        && n.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// Parses `.vega/schedule.toml`. A file without jobs is valid.
pub fn parse(text: &str) -> Result<Schedule, Error> {
    let doc: DocumentMut = text
        .parse()
        .map_err(|e| invalid(format!("not TOML: {e}")))?;
    let mut jobs: Vec<Job> = vec![];
    for (key, item) in doc.as_table().iter() {
        if key != "job" {
            return Err(invalid(format!("unknown key {key:?}; want [[job]] tables")));
        }
        let tables: Vec<&dyn TableLike> = match item {
            Item::ArrayOfTables(a) => a.iter().map(|t| t as &dyn TableLike).collect(),
            Item::Value(Value::Array(a)) => a
                .iter()
                .map(|v| v.as_inline_table().map(|t| t as &dyn TableLike))
                .collect::<Option<_>>()
                .ok_or_else(|| invalid("job: want [[job]] tables"))?,
            _ => return Err(invalid("job: want [[job]] tables")),
        };
        for (i, t) in tables.into_iter().enumerate() {
            let label = match t.get("name").and_then(Item::as_str) {
                Some(n) => format!("job {} ({n})", i + 1),
                None => format!("job {}", i + 1),
            };
            let job =
                parse_job(t).map_err(|e| Error::new(e.code, format!("{label}: {}", e.message)))?;
            if jobs.iter().any(|j| j.name == job.name) {
                return Err(invalid(format!("{label}: duplicate name")));
            }
            jobs.push(job);
        }
    }
    Ok(Schedule { jobs })
}

const KEYS: [&str; 10] = [
    "name", "run", "args", "daily", "weekly", "hosts", "timeout", "notify", "pin", "catchup",
];

fn parse_job(t: &dyn TableLike) -> Result<Job, Error> {
    for (k, _) in t.iter() {
        if !KEYS.contains(&k) {
            return Err(invalid(format!(
                "unknown key {k:?}; known: {}",
                KEYS.join(", ")
            )));
        }
    }
    let name = req_str(t, "name")?;
    if !valid_name(&name) {
        return Err(invalid(format!("name {name:?}: want [a-z0-9-]+")));
    }
    let run = req_str(t, "run")?;
    repo_path(&run)?;
    let args = opt_strs(t, "args")?.unwrap_or_default();
    let when = match (opt_str(t, "daily")?, opt_str(t, "weekly")?) {
        (Some(d), None) => When::daily(&d)?,
        (None, Some(w)) => When::weekly(&w)?,
        _ => return Err(invalid("want exactly one of daily, weekly")),
    };
    let hosts = opt_strs(t, "hosts")?
        .ok_or_else(|| invalid("hosts is required (no implicit every host)"))?;
    if hosts.is_empty() || hosts.iter().any(|h| h.is_empty()) {
        return Err(invalid("hosts: want at least one host name"));
    }
    let timeout = match opt_str(t, "timeout")? {
        None => DEFAULT_TIMEOUT,
        Some(s) => parse_duration(&s).map_err(|e| invalid(format!("timeout: {}", e.message)))?,
    };
    if timeout > MAX_TIMEOUT {
        return Err(invalid("timeout: at most 24h"));
    }
    let notify = match opt_str(t, "notify")?.as_deref() {
        None | Some("failure") => Notify::Failure,
        Some("always") => Notify::Always,
        Some("never") => Notify::Never,
        Some(n) => {
            return Err(invalid(format!(
                "notify {n:?}: want failure, always or never"
            )))
        }
    };
    let pin = opt_strs(t, "pin")?.unwrap_or_default();
    for p in &pin {
        repo_path(p)?;
    }
    let catchup = match t.get("catchup") {
        None => true,
        Some(i) => i
            .as_bool()
            .ok_or_else(|| invalid("catchup: want true or false"))?,
    };
    Ok(Job {
        name,
        run,
        args,
        when,
        hosts,
        timeout,
        notify,
        pin,
        catchup,
    })
}

fn opt_str(t: &dyn TableLike, key: &str) -> Result<Option<String>, Error> {
    t.get(key)
        .map(|i| {
            i.as_str()
                .map(str::to_string)
                .ok_or_else(|| invalid(format!("{key}: want a string")))
        })
        .transpose()
}

fn req_str(t: &dyn TableLike, key: &str) -> Result<String, Error> {
    opt_str(t, key)?.ok_or_else(|| invalid(format!("{key} is required")))
}

fn opt_strs(t: &dyn TableLike, key: &str) -> Result<Option<Vec<String>>, Error> {
    let Some(item) = t.get(key) else {
        return Ok(None);
    };
    let bad = || invalid(format!("{key}: want an array of strings"));
    item.as_array()
        .ok_or_else(bad)?
        .iter()
        .map(|v| v.as_str().map(str::to_string).ok_or_else(bad))
        .collect::<Result<_, _>>()
        .map(Some)
}

impl Schedule {
    /// The jobs that name `host`.
    pub fn for_host<'a>(&'a self, host: &'a str) -> impl Iterator<Item = &'a Job> + 'a {
        self.jobs
            .iter()
            .filter(move |j| j.hosts.iter().any(|h| h == host))
    }
}

impl Job {
    /// `run` and the `pin` entries, sorted and unique: the files whose blob
    /// ids at HEAD enter [`Job::hash`].
    pub fn hashed_paths(&self) -> Vec<&str> {
        let set: BTreeSet<&str> = std::iter::once(self.run.as_str())
            .chain(self.pin.iter().map(String::as_str))
            .collect();
        set.into_iter().collect()
    }

    /// Hex SHA-256 over `project id, name, run, args, hosts` and the sorted
    /// `(path, blob id)` pairs of [`Job::hashed_paths`]; `blobs` maps each of
    /// them to its blob id at HEAD. `daily`, `weekly`, `timeout`, `notify`
    /// and `catchup` stay out: they change neither what runs nor where.
    ///
    /// Canonical form: a version tag, then every field as an 8-byte
    /// big-endian length plus its bytes, lists prefixed by their count;
    /// `args` keep their order, `hosts` are a sorted set.
    pub fn hash(
        &self,
        project_id: &str,
        blobs: &BTreeMap<String, String>,
    ) -> Result<String, Error> {
        let mut h = Sha256::new();
        h.update(b"vega-schedule job v1\0");
        let field = |h: &mut Sha256, s: &str| {
            h.update((s.len() as u64).to_be_bytes());
            h.update(s.as_bytes());
        };
        let count = |h: &mut Sha256, n: usize| h.update((n as u64).to_be_bytes());
        field(&mut h, project_id);
        field(&mut h, &self.name);
        field(&mut h, &self.run);
        count(&mut h, self.args.len());
        for a in &self.args {
            field(&mut h, a);
        }
        let hosts: BTreeSet<&str> = self.hosts.iter().map(String::as_str).collect();
        count(&mut h, hosts.len());
        for host in hosts {
            field(&mut h, host);
        }
        let paths = self.hashed_paths();
        count(&mut h, paths.len());
        for p in paths {
            let blob = blobs.get(p).filter(|b| !b.is_empty()).ok_or_else(|| {
                Error::new(
                    "blob_missing",
                    format!("{p}: no blob id at HEAD (not committed?)"),
                )
            })?;
            field(&mut h, p);
            field(&mut h, blob);
        }
        Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jiff::civil::{Time, Weekday};

    const EXAMPLE: &str = r#"
[[job]]
name = "fristen-execute"
run = "scripts/fristen-execute"
args = ["--batch"]
daily = "07:30"
hosts = ["proteus"]
timeout = "15m"
notify = "failure"
pin = ["justfile"]
catchup = true

[[job]]
name = "triage"
run = "bin/triage"
weekly = "mon 03:00"
hosts = ["proteus", "ganymede"]
"#;

    fn one(extra: &str) -> Result<Schedule, Error> {
        parse(&format!(
            "[[job]]\nname = \"a\"\nrun = \"x\"\nhosts = [\"h\"]\n{extra}\n"
        ))
    }

    #[test]
    fn parses_the_design_example() {
        let s = parse(EXAMPLE).unwrap();
        let j = &s.jobs[0];
        assert_eq!(j.run, "scripts/fristen-execute");
        assert_eq!(j.args, vec!["--batch"]);
        assert_eq!(j.when, When::Daily(Time::new(7, 30, 0, 0).unwrap()));
        assert_eq!(j.timeout, SignedDuration::from_mins(15));
        assert_eq!(j.pin, vec!["justfile"]);
        let t = &s.jobs[1];
        assert_eq!(
            t.when,
            When::Weekly(Weekday::Monday, Time::new(3, 0, 0, 0).unwrap())
        );
        assert_eq!(
            (t.timeout, t.notify, t.catchup),
            (DEFAULT_TIMEOUT, Notify::Failure, true)
        );
        assert!(t.args.is_empty() && t.pin.is_empty());
        assert_eq!(
            s.for_host("ganymede")
                .map(|j| j.name.as_str())
                .collect::<Vec<_>>(),
            vec!["triage"]
        );
        assert_eq!(s.for_host("proteus").count(), 2);
        assert_eq!(parse("").unwrap(), Schedule::default());
    }

    #[test]
    fn unknown_keys_fail() {
        let e = one("daly = \"07:00\"").unwrap_err();
        assert_eq!(e.code, "schedule_invalid");
        assert!(
            e.message.contains("job 1 (a): unknown key \"daly\""),
            "{}",
            e.message
        );
        assert!(parse("[[jobs]]\nname = \"a\"").is_err());
        assert!(parse("version = 1").is_err());
        // The old keys are gone.
        for k in [
            "cmd = \"x\"",
            "cwd = \"x\"",
            "session = \"gui\"",
            "env = {}",
        ] {
            assert!(one(&format!("daily = \"07:00\"\n{k}")).is_err(), "{k}");
        }
    }

    #[test]
    fn validates_fields() {
        assert!(one("daily = \"07:00\"").is_ok());
        for bad in [
            "",                                          // no slot
            "daily = \"07:00\"\nweekly = \"mon 07:00\"", // two slots
            "daily = \"7:00\"",
            "daily = 700",
            "daily = \"07:00\"\ntimeout = \"25h\"",
            "daily = \"07:00\"\ntimeout = \"0m\"",
            "daily = \"07:00\"\nnotify = \"sometimes\"",
            "daily = \"07:00\"\ncatchup = \"yes\"",
            "daily = \"07:00\"\nargs = \"--x\"",
            "daily = \"07:00\"\nargs = [1]",
        ] {
            assert!(one(bad).is_err(), "{bad}");
        }
        assert!(
            parse("[[job]]\nname = \"a\"\nrun = \"x\"\ndaily = \"07:00\"").is_err(),
            "hosts required"
        );
        assert!(
            parse("[[job]]\nname = \"a\"\nrun = \"x\"\ndaily = \"07:00\"\nhosts = []").is_err()
        );
        assert!(
            parse("[[job]]\nname = \"A_b\"\nrun = \"x\"\ndaily = \"07:00\"\nhosts = [\"h\"]")
                .is_err()
        );
        let dup = "[[job]]\nname = \"a\"\nrun = \"x\"\ndaily = \"07:00\"\nhosts = [\"h\"]\n";
        assert!(parse(&format!("{dup}{dup}"))
            .unwrap_err()
            .message
            .contains("duplicate"));
        // Inline array of tables is the same TOML.
        let s = parse("job = [{name = \"a\", run = \"x\", daily = \"07:00\", hosts = [\"h\"]}]")
            .unwrap();
        assert_eq!(s.jobs.len(), 1);
    }

    #[test]
    fn path_rules() {
        for ok in ["x", "scripts/fristen-execute", "a/b.c/d", "just"] {
            assert!(repo_path(ok).is_ok(), "{ok}");
        }
        for bad in [
            "",
            "/usr/bin/just",
            "../x",
            "a/../b",
            "a/..",
            "./x",
            "a//b",
            "a/",
            "a\\b",
        ] {
            assert_eq!(repo_path(bad).unwrap_err().code, "path_invalid", "{bad}");
        }
        assert_eq!(
            one("daily = \"07:00\"\npin = [\"../secrets\"]")
                .unwrap_err()
                .code,
            "path_invalid"
        );
        let e =
            parse("[[job]]\nname = \"a\"\nrun = \"/bin/sh\"\ndaily = \"07:00\"\nhosts = [\"h\"]")
                .unwrap_err();
        assert_eq!(e.code, "path_invalid");
        assert!(e.message.starts_with("job 1 (a): "), "{}", e.message);
    }

    fn blobs(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(p, b)| (p.to_string(), b.to_string()))
            .collect()
    }

    #[test]
    fn hash_is_stable() {
        let j = &parse(EXAMPLE).unwrap().jobs[0];
        let b = blobs(&[("scripts/fristen-execute", "aaaa"), ("justfile", "bbbb")]);
        let h = j.hash("github.com/o/brainz", &b).unwrap();
        // Golden value: changing the canonical form needs a new version tag
        // (and re-approval on every host).
        assert_eq!(
            h,
            "926e26d2a4cf942919a491b96121d02663b0427b2d103da16e82fae56b7a439e"
        );
        assert_eq!(j.hash("github.com/o/brainz", &b).unwrap(), h);
    }

    #[test]
    fn hash_covers_what_runs_and_where() {
        let j = parse(EXAMPLE).unwrap().jobs[0].clone();
        let b = blobs(&[("scripts/fristen-execute", "aaaa"), ("justfile", "bbbb")]);
        let base = j.hash("p", &b).unwrap();
        let differs =
            |j2: &Job, p: &str, b2: &BTreeMap<String, String>| j2.hash(p, b2).unwrap() != base;
        assert!(differs(&j, "q", &b), "project id");
        let mut k = j.clone();
        k.name = "other".into();
        assert!(differs(&k, "p", &b), "name");
        let mut k = j.clone();
        k.args.push("--dry".into());
        assert!(differs(&k, "p", &b), "args");
        let mut k = j.clone();
        k.hosts.push("ganymede".into());
        assert!(differs(&k, "p", &b), "hosts");
        let script = blobs(&[("scripts/fristen-execute", "cccc"), ("justfile", "bbbb")]);
        assert!(differs(&j, "p", &script), "script blob");
        let pinned = blobs(&[("scripts/fristen-execute", "aaaa"), ("justfile", "dddd")]);
        assert!(differs(&j, "p", &pinned), "pinned blob");
        // Field boundaries are unambiguous.
        let mut k = j.clone();
        k.args = vec!["--ba".into(), "tch".into()];
        assert!(differs(&k, "p", &b), "args split");

        // What cannot change what runs or where stays out.
        let mut k = j.clone();
        k.when = When::weekly("sun 01:00").unwrap();
        k.timeout = SignedDuration::from_hours(2);
        k.notify = Notify::Never;
        k.catchup = false;
        assert_eq!(k.hash("p", &b).unwrap(), base);
        // hosts are a set; extra blob entries do not matter.
        let mut k = j.clone();
        k.hosts = vec!["proteus".into(), "proteus".into()];
        assert_eq!(k.hash("p", &b).unwrap(), base);
        let mut more = b.clone();
        more.insert("README".into(), "eeee".into());
        assert_eq!(j.hash("p", &more).unwrap(), base);
    }

    #[test]
    fn hash_needs_every_blob() {
        let j = &parse(EXAMPLE).unwrap().jobs[0];
        assert_eq!(
            j.hashed_paths(),
            vec!["justfile", "scripts/fristen-execute"]
        );
        let e = j
            .hash("p", &blobs(&[("scripts/fristen-execute", "aaaa")]))
            .unwrap_err();
        assert_eq!(e.code, "blob_missing");
    }
}
