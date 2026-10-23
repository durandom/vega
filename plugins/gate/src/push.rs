//! The git shim's decision for `git push` (gate.md section 1). Parsing the
//! argv is pure; everything after it runs only for a push.

use std::path::{Path, PathBuf};

use crate::git::{commit, config_at, current_branch, git, is_beads_repo};
use crate::segments::plan;
use crate::start::{self, Outcome};
use crate::{cache_dir, git::object};

/// What the shim does with a push.
#[derive(Debug, PartialEq)]
pub enum Decision {
    /// exec git, silently.
    Exec,
    /// One stderr line `vega gate: <line>`, then exec git.
    Note(String),
    /// stderr `vega gate: <line>`, exit 1.
    Block(String),
}

/// git's global options that take the next argument as their value.
const GLOBAL_WITH_VALUE: [&str; 8] = [
    "-C",
    "-c",
    "--git-dir",
    "--work-tree",
    "--namespace",
    "--config-env",
    "--super-prefix",
    "--attr-source",
];

/// The global options before the subcommand and the subcommand's index.
/// Pure: no IO, so non-push calls stay a plain exec.
pub fn subcommand(args: &[String]) -> Option<usize> {
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        if GLOBAL_WITH_VALUE.contains(&a) {
            i += 2;
        } else if a.starts_with('-') {
            i += 1;
        } else {
            return Some(i);
        }
    }
    None
}

pub fn is_push(args: &[String]) -> bool {
    subcommand(args).is_some_and(|i| args[i] == "push")
}

/// The push's own options and operands.
#[derive(Debug, Default, PartialEq)]
pub struct PushArgs {
    /// `-C` dirs, in order.
    pub dirs: Vec<String>,
    /// A global option the gate does not follow (`--git-dir`, `--work-tree`).
    pub unsupported: Option<String>,
    pub dry_run: bool,
    pub delete: bool,
    pub all: bool,
    pub remote: Option<String>,
    pub refspecs: Vec<String>,
}

/// Push options that take the next argument as their value.
const PUSH_WITH_VALUE: [&str; 5] = ["-o", "--push-option", "--repo", "--receive-pack", "--exec"];

pub fn parse(args: &[String]) -> Option<PushArgs> {
    let sub = subcommand(args)?;
    let mut p = PushArgs::default();
    let mut i = 0;
    while i < sub {
        match args[i].as_str() {
            "-C" => p.dirs.push(args.get(i + 1)?.clone()),
            o @ ("--git-dir" | "--work-tree") => p.unsupported = Some(o.into()),
            o if o.starts_with("--git-dir=") || o.starts_with("--work-tree=") => {
                p.unsupported = Some(o.split('=').next().unwrap_or(o).into())
            }
            _ => {}
        }
        i += if GLOBAL_WITH_VALUE.contains(&args[i].as_str()) {
            2
        } else {
            1
        };
    }
    let mut operands = Vec::new();
    let mut it = args[sub + 1..].iter();
    let mut opts_done = false;
    while let Some(a) = it.next() {
        let s = a.as_str();
        if opts_done || !s.starts_with('-') || s == "-" {
            operands.push(a.clone());
            continue;
        }
        match s {
            "--" => opts_done = true,
            "--dry-run" => p.dry_run = true,
            "--delete" => p.delete = true,
            "--all" | "--branches" | "--mirror" => p.all = true,
            "--repo" => p.remote = it.next().cloned(),
            _ if s.starts_with("--repo=") => p.remote = Some(s[7..].to_string()),
            _ if PUSH_WITH_VALUE.contains(&s) => {
                it.next();
            }
            _ if s.starts_with("--") => {}
            // Short flags, possibly bundled: -fu, -nv, -o <opt>.
            _ => {
                let flags = &s[1..];
                p.dry_run |= flags.contains('n');
                p.delete |= flags.contains('d');
                if flags.ends_with('o') {
                    it.next();
                }
            }
        }
    }
    let mut operands = operands.into_iter();
    if p.remote.is_none() {
        p.remote = operands.next();
    }
    p.refspecs = operands.collect();
    Some(p)
}

/// One branch a push updates.
#[derive(Debug, PartialEq)]
pub struct Dest {
    pub head: String,
    pub branch: String,
    /// `refs/remotes/<remote>/<branch>`, else `refs/remotes/<remote>/HEAD`.
    pub target: String,
}

fn cfg(dir: &Path, key: &str) -> Option<String> {
    git(dir, &["config", "--get", key])
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// The branches the push updates. Tags, deletes and other refs are left
/// out; an empty list means nothing to gate.
pub fn destinations(dir: &Path, p: &PushArgs) -> Result<Vec<Dest>, String> {
    let cur = current_branch(dir);
    let remote = p.remote.clone().unwrap_or_else(|| {
        cur.as_ref()
            .and_then(|b| cfg(dir, &format!("branch.{b}.pushRemote")))
            .or_else(|| cfg(dir, "remote.pushDefault"))
            .or_else(|| {
                cur.as_ref()
                    .and_then(|b| cfg(dir, &format!("branch.{b}.remote")))
            })
            .unwrap_or_else(|| "origin".into())
    });
    // (src, dst branch)
    let mut pairs: Vec<(String, String)> = Vec::new();
    if p.all {
        for b in git(
            dir,
            &["for-each-ref", "--format=%(refname:short)", "refs/heads"],
        )?
        .lines()
        {
            pairs.push((b.to_string(), b.to_string()));
        }
    } else if p.refspecs.is_empty() {
        let Some(b) = cur.clone() else {
            return Ok(Vec::new());
        };
        let dst = git(
            dir,
            &[
                "rev-parse",
                "--abbrev-ref",
                "--symbolic-full-name",
                "@{push}",
            ],
        )
        .ok()
        .and_then(|s| {
            s.trim()
                .strip_prefix(&format!("{remote}/"))
                .map(String::from)
        })
        .unwrap_or_else(|| b.clone());
        pairs.push((b, dst));
    } else {
        for spec in &p.refspecs {
            let spec = spec.trim_start_matches('+');
            if spec.contains('*') {
                return Err(format!("pattern refspec {spec}"));
            }
            let (src, dst) = match spec.split_once(':') {
                Some((s, d)) => (s.to_string(), Some(d.to_string())),
                None => (spec.to_string(), None),
            };
            if src.is_empty() {
                continue; // :<ref> deletes
            }
            let is_tag = src.starts_with("refs/tags/")
                || (object(dir, &format!("refs/heads/{src}")).is_none()
                    && object(dir, &format!("refs/tags/{src}")).is_some());
            let dst = match dst {
                Some(d) => d,
                None if is_tag => continue,
                None if src == "HEAD" => match cur.clone() {
                    Some(b) => b,
                    None => continue,
                },
                None => src.clone(),
            };
            let dst = match dst.strip_prefix("refs/heads/") {
                Some(b) => b.to_string(),
                None if dst.starts_with("refs/") => continue,
                None => dst,
            };
            if dst.is_empty() {
                continue;
            }
            pairs.push((src, dst));
        }
    }
    let mut out = Vec::new();
    for (src, branch) in pairs {
        let head = commit(dir, &src).ok_or(format!("{src} is no commit"))?;
        let tracking = format!("refs/remotes/{remote}/{branch}");
        let target = if commit(dir, &tracking).is_some() {
            tracking
        } else {
            let h = format!("refs/remotes/{remote}/HEAD");
            commit(dir, &h).map(|_| h).ok_or(format!(
                "no target for {branch}: neither {tracking} nor {remote}/HEAD"
            ))?
        };
        out.push(Dest {
            head,
            branch,
            target,
        });
    }
    Ok(out)
}

/// The reviewer refusal: `VEGA_GATE_ROLE=reviewer`, or a cwd inside a gate clone.
pub fn is_reviewer(cwd: &Path) -> bool {
    let canon = |p: &Path| p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
    std::env::var("VEGA_GATE_ROLE").is_ok_and(|r| r == "reviewer")
        || canon(cwd).starts_with(canon(&cache_dir().join("clones")))
}

/// The gate for a push whose argv is `args`, run in `cwd`. Errors are
/// returned for the caller to fail open.
pub fn decide(args: &[String], cwd: &Path) -> Result<Decision, String> {
    let Some(p) = parse(args) else {
        return Ok(Decision::Exec);
    };
    if is_reviewer(cwd) {
        return Ok(Decision::Block("a gate reviewer does not push".into()));
    }
    if p.dry_run || p.delete {
        return Ok(Decision::Exec);
    }
    if let Some(o) = &p.unsupported {
        return Err(format!("global option {o} not followed"));
    }
    let dir: PathBuf = p.dirs.iter().fold(cwd.to_path_buf(), |d, c| d.join(c));
    let dests = destinations(&dir, &p)?;
    let mut plans = Vec::new();
    for d in &dests {
        let Some((blob, config)) = config_at(&dir, &d.target)? else {
            continue;
        };
        if !is_beads_repo(&dir) {
            let note = "repo has .vega/gate.toml but no .beads; push not gated (beads repos only)";
            return Ok(
                if vega_shim::first_time(vega_shim::session_id().as_deref(), "gate-no-beads") {
                    Decision::Note(note.into())
                } else {
                    Decision::Exec
                },
            );
        }
        let pl = plan(&dir, &config, &blob, &d.target, &d.head)?;
        if !pl.segments.is_empty() {
            plans.push((d, config, pl));
        }
    }
    if plans.is_empty() {
        return Ok(Decision::Exec);
    }
    if std::env::var("VEGA_GATE").is_ok_and(|v| v == "skip") {
        let mut ids = Vec::new();
        for (d, _, pl) in &plans {
            ids.extend(start::skip(&dir, pl, &d.branch)?);
        }
        if ids.is_empty() {
            return Ok(Decision::Exec);
        }
        return Ok(Decision::Note(format!(
            "skipped (VEGA_GATE=skip), recorded as {}",
            ids.join(", ")
        )));
    }
    let mut blocks = Vec::new();
    let mut notes = Vec::new();
    for (d, config, pl) in &plans {
        let l = start::lookup(&dir, pl)?;
        let missing: Vec<String> = l
            .missing
            .iter()
            .chain(l.running.iter().map(|(s, _)| s))
            .map(|s| s.label())
            .collect();
        match start::start_from(&dir, pl, config, &d.branch, l)? {
            Outcome::Passed => {}
            Outcome::FailOpen(m) => notes.push(m),
            Outcome::Running(m) | Outcome::Failed(m) => {
                blocks.push(m);
                if !missing.is_empty() {
                    blocks.push(format!("not reviewed: {}", missing.join(", ")));
                }
            }
        }
    }
    Ok(if !blocks.is_empty() {
        Decision::Block(blocks.join("\nvega gate: "))
    } else if !notes.is_empty() {
        Decision::Note(notes.join("\nvega gate: "))
    } else {
        Decision::Exec
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a(s: &str) -> Vec<String> {
        s.split_whitespace().map(String::from).collect()
    }

    #[test]
    fn finds_push_behind_global_options() {
        assert!(is_push(&a("push")));
        assert!(is_push(&a("-C ../x -c a.b=c --no-pager push origin main")));
        assert!(is_push(&a("--git-dir /g push")));
        assert!(!is_push(&a("-C push status")));
        assert!(!is_push(&a("status")));
        assert!(!is_push(&a("log --grep push")));
        assert!(!is_push(&a("--version")));
        assert!(!is_push(&[]));
    }

    #[test]
    fn parses_push_options_and_operands() {
        let p = parse(&a(
            "-C sub push -u --force-with-lease -o ci.skip origin HEAD:main",
        ))
        .unwrap();
        assert_eq!(p.dirs, vec!["sub"]);
        assert_eq!(p.remote.as_deref(), Some("origin"));
        assert_eq!(p.refspecs, vec!["HEAD:main"]);
        assert!(!p.dry_run && !p.delete && !p.all);
        assert!(parse(&a("push -n")).unwrap().dry_run);
        assert!(parse(&a("push -nv")).unwrap().dry_run);
        assert!(parse(&a("push --dry-run")).unwrap().dry_run);
        assert!(parse(&a("push -d origin x")).unwrap().delete);
        assert!(parse(&a("push --delete origin x")).unwrap().delete);
        assert!(parse(&a("push --all")).unwrap().all);
        let p = parse(&a("push --repo=up main")).unwrap();
        assert_eq!(
            (p.remote.as_deref(), p.refspecs.clone()),
            (Some("up"), vec!["main".to_string()])
        );
        assert_eq!(
            parse(&a("--work-tree /w push"))
                .unwrap()
                .unsupported
                .as_deref(),
            Some("--work-tree")
        );
    }
}
