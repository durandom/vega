//! The relevant diff, its key, and the cut of a push into segments
//! (gate.md section 3).

use std::path::Path;

use crate::config::{Config, Review};
use crate::git::{commit, git, git_ok, object};
use crate::sha256;

/// The rubric `vega gate` uses when a review names none.
pub const DEFAULT_RUBRIC: &str = include_str!("../rubric/SKILL.md");

const KEY_VERSION: &str = "vega-gate/1";

#[derive(Debug, Clone, PartialEq)]
pub enum Kind {
    /// A run of non-merge commits on the first-parent walk.
    Run { first: String, last: String },
    /// The branch a merge brought in: `merge-base(P1, P2)..P2`.
    Branch {
        merge: String,
        parent: usize,
        tip: String,
        name: Option<String>,
    },
    /// A merge's own changes (its combined diff).
    Merge { merge: String },
}

#[derive(Debug, Clone)]
pub struct Segment {
    pub kind: Kind,
    /// The range `from..to`; for a merge segment both are the merge.
    pub from: String,
    pub to: String,
    /// The relevant diff, `index` lines dropped; empty: nothing to review.
    pub diff: String,
    /// The reviews whose paths the diff touches, in config order.
    pub reviews: Vec<String>,
    pub key: String,
}

impl Segment {
    /// For stderr lines: `direct 328679b`, `branch review-patch (3393533^2)`, `merge 3393533`.
    pub fn label(&self) -> String {
        match &self.kind {
            Kind::Run { first, last } if first == last => format!("direct {}", short(first)),
            Kind::Run { first, last } => format!("direct {}..{}", short(first), short(last)),
            Kind::Branch {
                merge,
                parent,
                name,
                ..
            } => format!(
                "branch {} ({}^{parent})",
                name.as_deref().unwrap_or("-"),
                short(merge)
            ),
            Kind::Merge { merge } => format!("merge {}", short(merge)),
        }
    }

    /// The bead metadata `segment` value: `run <a>..<b>`, `branch <P2>`, `merge <M>`.
    pub fn meta(&self) -> String {
        match &self.kind {
            Kind::Run { .. } => format!("run {}..{}", self.from, self.to),
            Kind::Branch { tip, .. } => format!("branch {tip}"),
            Kind::Merge { merge } => format!("merge {merge}"),
        }
    }

    /// The command that shows this segment's change to a reviewer.
    pub fn show_cmd(&self, specs: &[String]) -> String {
        let specs = specs
            .iter()
            .map(|s| format!("'{s}'"))
            .collect::<Vec<_>>()
            .join(" ");
        match &self.kind {
            Kind::Merge { merge } => format!("git show --cc {merge} -- {specs}"),
            _ => format!("git diff {} {} -- {specs}", self.from, self.to),
        }
    }
}

pub fn short(sha: &str) -> &str {
    &sha[..sha.len().min(7)]
}

/// A push or a branch cut into segments against its target.
#[derive(Debug)]
pub struct Plan {
    pub target: String,
    /// Immutable push target; clone remote refs may point elsewhere.
    pub target_sha: String,
    pub base: String,
    pub head: String,
    pub config_blob: String,
    /// (review name, rubric id at the target) per review.
    pub rubrics: Vec<(String, String)>,
    /// Segments with a non-empty relevant diff, in walk order.
    pub segments: Vec<Segment>,
    /// Segments whose relevant diff is empty.
    pub empty: usize,
}

/// The rubric id of `r` at `target`: its tree or blob id, `default:<sha>` for
/// vega's own.
pub fn rubric_id(dir: &Path, target: &str, r: &Review) -> Result<String, String> {
    match &r.rubric {
        None => Ok(format!("default:{}", sha256(DEFAULT_RUBRIC.as_bytes()))),
        Some(p) => object(dir, &format!("{target}:{p}"))
            .ok_or_else(|| format!("rubric {p} of review {} missing at {target}", r.name)),
    }
}

/// Cuts `base..head` into segments by the first-parent walk (gate.md, Segments).
pub fn plan(
    dir: &Path,
    cfg: &Config,
    config_blob: &str,
    target: &str,
    head: &str,
) -> Result<Plan, String> {
    let target_sha = commit(dir, target).ok_or(format!("target {target} is no commit"))?;
    let head = commit(dir, head).ok_or(format!("{head} is no commit"))?;
    let base = git(dir, &["merge-base", &target_sha, &head])
        .map(|s| s.trim().to_string())
        .map_err(|_| format!("{target} and {} share no history", short(&head)))?;
    let rubrics = cfg
        .reviews
        .iter()
        .map(|r| Ok((r.name.clone(), rubric_id(dir, &target_sha, r)?)))
        .collect::<Result<Vec<_>, String>>()?;
    let walk = git(
        dir,
        &[
            "rev-list",
            "--first-parent",
            "--reverse",
            "--parents",
            &format!("{base}..{head}"),
        ],
    )?;
    let mut ranges: Vec<(Kind, String, String)> = Vec::new();
    let mut run: Option<(String, String, String)> = None; // (from, first, last)
    for line in walk.lines() {
        let mut ids = line.split_whitespace().map(String::from);
        let Some(c) = ids.next() else { continue };
        let parents: Vec<String> = ids.collect();
        if parents.len() < 2 {
            let from = parents.first().cloned().unwrap_or_else(|| base.clone());
            run = Some(match run.take() {
                Some((f, first, _)) => (f, first, c),
                None => (from, c.clone(), c),
            });
            continue;
        }
        if let Some((from, first, last)) = run.take() {
            ranges.push((
                Kind::Run {
                    first,
                    last: last.clone(),
                },
                from,
                last,
            ));
        }
        for (i, p) in parents.iter().enumerate().skip(1) {
            // Main merged into a branch: that side is already on the target.
            if git_ok(dir, &["merge-base", "--is-ancestor", p, &target_sha]) {
                continue;
            }
            let fork = git(dir, &["merge-base", &parents[0], p])?
                .trim()
                .to_string();
            let subject = git(dir, &["log", "-1", "--format=%s", &c])?;
            ranges.push((
                Kind::Branch {
                    merge: c.clone(),
                    parent: i + 1,
                    tip: p.clone(),
                    name: branch_name(subject.trim()),
                },
                fork,
                p.clone(),
            ));
        }
        ranges.push((Kind::Merge { merge: c.clone() }, c.clone(), c));
    }
    if let Some((from, first, last)) = run {
        ranges.push((
            Kind::Run {
                first,
                last: last.clone(),
            },
            from,
            last,
        ));
    }
    let mut segments = Vec::new();
    let mut empty = 0;
    for (kind, from, to) in ranges {
        let mut diff = String::new();
        let mut reviews = Vec::new();
        for r in &cfg.reviews {
            let d = match &kind {
                Kind::Merge { merge } => combined_diff(dir, merge, r)?,
                _ => range_diff(dir, &from, &to, r)?,
            };
            if !d.is_empty() {
                diff.push_str(&format!("# review {}\n{d}", r.name));
                reviews.push(r.name.clone());
            }
        }
        if diff.is_empty() {
            empty += 1;
            continue;
        }
        let key = key(&diff, config_blob, &rubrics, &reviews);
        segments.push(Segment {
            kind,
            from,
            to,
            diff,
            reviews,
            key,
        });
    }
    Ok(Plan {
        target: target.to_string(),
        target_sha,
        base,
        head,
        config_blob: config_blob.to_string(),
        rubrics,
        segments,
        empty,
    })
}

/// Options that make a diff the same on every host, whatever its git config.
const DIFF_OPTS: [&str; 9] = [
    "--no-color",
    "--no-ext-diff",
    "--no-textconv",
    "--no-renames",
    "-U3",
    "--diff-algorithm=myers",
    "--src-prefix=a/",
    "--dst-prefix=b/",
    "--ignore-submodules=none",
];

/// The relevant diff of `a..b` for one review, `index` lines dropped.
pub fn range_diff(dir: &Path, a: &str, b: &str, r: &Review) -> Result<String, String> {
    let mut args = vec!["-c", "core.quotePath=true", "diff"];
    args.extend(DIFF_OPTS);
    args.extend([a, b, "--"]);
    let specs = r.pathspecs();
    args.extend(specs.iter().map(String::as_str));
    git(dir, &args).map(|d| drop_index_lines(&d))
}

/// A merge's combined diff for one review (`--cc`: only hunks that differ
/// from every parent, i.e. conflict resolutions and evil changes).
pub fn combined_diff(dir: &Path, merge: &str, r: &Review) -> Result<String, String> {
    let mut args = vec![
        "-c",
        "core.quotePath=true",
        "diff-tree",
        "-p",
        "--cc",
        "--no-commit-id",
    ];
    args.extend(DIFF_OPTS);
    args.extend([merge, "--"]);
    let specs = r.pathspecs();
    args.extend(specs.iter().map(String::as_str));
    git(dir, &args).map(|d| drop_index_lines(&d))
}

/// `index` header lines hold pre-image blob ids that change when the target
/// moves without touching these lines; content lines never start with it
/// (they carry a ` `, `+` or `-` prefix).
pub fn drop_index_lines(diff: &str) -> String {
    diff.lines()
        .filter(|l| !l.starts_with("index "))
        .map(|l| format!("{l}\n"))
        .collect()
}

/// `sha256("vega-gate/1" ‖ sha256(diff) ‖ gate.toml blob ‖ rubric id of each
/// matched review)`, 64 hex.
pub fn key(
    diff: &str,
    config_blob: &str,
    rubrics: &[(String, String)],
    reviews: &[String],
) -> String {
    let mut s = format!(
        "{KEY_VERSION}\n{}\n{config_blob}\n",
        sha256(diff.as_bytes())
    );
    for (name, id) in rubrics.iter().filter(|(n, _)| reviews.contains(n)) {
        s.push_str(&format!("{name}={id}\n"));
    }
    sha256(s.as_bytes())
}

/// The branch name of a merge subject: `Merge branch 'x'`, `Merge x: ...`,
/// `Merge pull request #1 from o/x`, `Merge remote-tracking branch 'origin/x'`.
pub fn branch_name(subject: &str) -> Option<String> {
    let rest = subject.strip_prefix("Merge ")?;
    let quoted = |r: &str| r.split('\'').next().map(String::from);
    let name = if let Some(r) = rest.strip_prefix("branch '") {
        quoted(r)
    } else if let Some(r) = rest.strip_prefix("remote-tracking branch '") {
        quoted(r).map(|b| b.split_once('/').map_or(b.clone(), |(_, n)| n.into()))
    } else if let Some(r) = rest.strip_prefix("pull request #") {
        r.split(" from ")
            .nth(1)
            .and_then(|s| s.split_whitespace().next())
            .map(|s| s.split_once('/').map_or(s, |(_, n)| n).to_string())
    } else {
        rest.split_whitespace()
            .next()
            .map(|w| w.trim_end_matches(':').to_string())
    };
    name.filter(|n| !n.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn branch_names_from_merge_subjects() {
        for (s, want) in [
            ("Merge branch 'fix-login'", Some("fix-login")),
            ("Merge branch 'a' into main", Some("a")),
            (
                "Merge review-patch: live-patch loop (vega-lhc.149)",
                Some("review-patch"),
            ),
            ("Merge pull request #12 from durandom/topic", Some("topic")),
            ("Merge remote-tracking branch 'origin/x'", Some("x")),
            ("fix: something", None),
        ] {
            assert_eq!(branch_name(s).as_deref(), want, "{s}");
        }
    }

    #[test]
    fn index_lines_go_content_stays() {
        let d = "diff --git a/x b/x\nindex 1234..5678 100644\n--- a/x\n+++ b/x\n@@ -1 +1 @@\n-index old\n+index new\n";
        let out = drop_index_lines(d);
        assert!(!out.contains("1234"));
        assert!(out.contains("-index old\n+index new"));
    }

    #[test]
    fn key_depends_on_diff_config_and_matched_rubrics() {
        let rub = vec![
            ("a".to_string(), "r1".to_string()),
            ("b".into(), "r2".into()),
        ];
        let k = key("d", "c", &rub, &["a".into()]);
        assert_eq!(k.len(), 64);
        assert_eq!(k, key("d", "c", &rub, &["a".into()]));
        assert_ne!(k, key("d2", "c", &rub, &["a".into()]));
        assert_ne!(k, key("d", "c2", &rub, &["a".into()]));
        let rub2 = vec![
            ("a".to_string(), "r1x".to_string()),
            ("b".into(), "r2".into()),
        ];
        assert_ne!(k, key("d", "c", &rub2, &["a".into()]));
        // A rubric of another review does not touch this key.
        let rub3 = vec![
            ("a".to_string(), "r1".to_string()),
            ("b".into(), "r2x".into()),
        ];
        assert_eq!(k, key("d", "c", &rub3, &["a".into()]));
    }
}
