//! The segment walk and key stability on fixture repos (gate.md section 3).

mod common;

use std::path::Path;

use common::{commit, git, repo, write};
use vega_gate::git::config_at;
use vega_gate::segments::{plan, Kind, Plan};

fn plan_at(work: &Path, target: &str, head: &str) -> Plan {
    let (blob, cfg) = config_at(work, target).unwrap().unwrap();
    plan(work, &cfg, &blob, target, head).unwrap()
}

fn kinds(p: &Plan) -> Vec<&'static str> {
    p.segments
        .iter()
        .map(|s| match s.kind {
            Kind::Run { .. } => "run",
            Kind::Branch { .. } => "branch",
            Kind::Merge { .. } => "merge",
        })
        .collect()
}

#[test]
fn a_merged_branch_keeps_the_sentinels_key() {
    let tmp = tempfile::tempdir().unwrap();
    let work = repo(tmp.path());
    git(&work, &["checkout", "-q", "-b", "feat"]);
    commit(&work, "src/a.rs", "fn a() { 1 }\n", "a1");
    commit(&work, "src/b.rs", "fn b() {}\n", "b1");
    commit(&work, "docs/x.md", "doc 2\n", "docs only");
    // The sentinel's review: its branch against origin/main is one run.
    let sentinel = plan_at(&work, "origin/main", "HEAD");
    assert_eq!(kinds(&sentinel), vec!["run"]);
    let key = sentinel.segments[0].key.clone();
    assert!(sentinel.segments[0].diff.contains("src/b.rs"));
    assert!(!sentinel.segments[0].diff.contains("docs/x.md"));

    // Main moves meanwhile; the UAC merges the branch with a merge commit.
    git(&work, &["checkout", "-q", "main"]);
    commit(&work, "src/c.rs", "fn c() {}\n", "uac direct");
    git(
        &work,
        &["merge", "-q", "--no-ff", "-m", "Merge feat: thing", "feat"],
    );
    let uac = plan_at(&work, "origin/main", "main");
    // The direct commit is a run; the branch is a segment of its own with the
    // sentinel's key; a clean merge has no combined diff.
    assert_eq!(kinds(&uac), vec!["run", "branch"]);
    assert_eq!(uac.segments[1].key, key);
    assert_eq!(
        uac.segments[1].label(),
        format!(
            "branch feat ({}^2)",
            &git(&work, &["rev-parse", "--short=7", "main"])
        )
    );
    assert_eq!(uac.empty, 1);
    assert!(uac.segments[0].label().starts_with("direct "));
}

#[test]
fn a_fast_forward_merge_is_a_run_with_the_branch_key() {
    let tmp = tempfile::tempdir().unwrap();
    let work = repo(tmp.path());
    git(&work, &["checkout", "-q", "-b", "feat"]);
    commit(&work, "src/a.rs", "fn a() { 2 }\n", "a2");
    commit(&work, "src/a.rs", "fn a() { 3 }\n", "a3");
    let key = plan_at(&work, "origin/main", "HEAD").segments[0]
        .key
        .clone();
    git(&work, &["checkout", "-q", "main"]);
    git(&work, &["merge", "-q", "--ff-only", "feat"]);
    let uac = plan_at(&work, "origin/main", "main");
    assert_eq!(kinds(&uac), vec!["run"]);
    assert_eq!(uac.segments[0].key, key);
}

#[test]
fn a_rebased_branch_keeps_its_key() {
    let tmp = tempfile::tempdir().unwrap();
    let work = repo(tmp.path());
    git(&work, &["checkout", "-q", "-b", "feat"]);
    commit(&work, "src/a.rs", "fn a() { 4 }\n", "a4");
    let before = plan_at(&work, "origin/main", "HEAD").segments[0]
        .key
        .clone();
    // Main moves in another file of the reviewed paths; the branch rebases.
    git(&work, &["checkout", "-q", "main"]);
    commit(&work, "src/z.rs", "fn z() {}\n", "z");
    git(&work, &["push", "-q", "origin", "main"]);
    git(&work, &["checkout", "-q", "feat"]);
    git(&work, &["rebase", "-q", "main"]);
    let after = plan_at(&work, "origin/main", "HEAD");
    assert_eq!(after.segments.len(), 1);
    assert_eq!(after.segments[0].key, before);
    // A real change to the diff is a new key.
    git(&work, &["commit", "-q", "--amend", "-m", "a4 reworded"]);
    assert_eq!(
        plan_at(&work, "origin/main", "HEAD").segments[0].key,
        before
    );
    commit(&work, "src/a.rs", "fn a() { 5 }\n", "a5");
    assert_ne!(
        plan_at(&work, "origin/main", "HEAD").segments[0].key,
        before
    );
}

#[test]
fn main_merged_into_a_branch_adds_no_segment_for_main() {
    let tmp = tempfile::tempdir().unwrap();
    let work = repo(tmp.path());
    git(&work, &["checkout", "-q", "-b", "feat"]);
    commit(&work, "src/a.rs", "fn a() { 6 }\n", "a6");
    git(&work, &["checkout", "-q", "main"]);
    commit(&work, "src/m.rs", "fn m() {}\n", "main work");
    git(&work, &["push", "-q", "origin", "main"]);
    git(&work, &["checkout", "-q", "feat"]);
    git(
        &work,
        &[
            "merge",
            "-q",
            "--no-ff",
            "-m",
            "Merge branch 'main' into feat",
            "main",
        ],
    );
    let p = plan_at(&work, "origin/main", "HEAD");
    assert_eq!(kinds(&p), vec!["run"]);
    assert!(!p.segments[0].diff.contains("src/m.rs"));
    assert_eq!(p.empty, 1);
}

#[test]
fn an_evil_merge_is_reviewed_on_its_own() {
    let tmp = tempfile::tempdir().unwrap();
    let work = repo(tmp.path());
    git(&work, &["checkout", "-q", "-b", "feat"]);
    commit(&work, "src/a.rs", "fn a() { 7 }\n", "a7");
    git(&work, &["checkout", "-q", "main"]);
    git(&work, &["merge", "-q", "--no-ff", "--no-commit", "feat"]);
    // A change in the merge commit that neither parent has.
    write(&work, "src/a.rs", "fn a() { 7 }\nfn evil() {}\n");
    git(&work, &["add", "-A"]);
    git(&work, &["commit", "-q", "-m", "Merge feat"]);
    let p = plan_at(&work, "origin/main", "main");
    assert_eq!(kinds(&p), vec!["branch", "merge"]);
    assert!(p.segments[1].diff.contains("evil"));
    assert!(p.segments[1].label().starts_with("merge "));
}

#[test]
fn keys_change_with_the_targets_rubric_or_config_only() {
    let tmp = tempfile::tempdir().unwrap();
    let work = repo(tmp.path());
    git(&work, &["checkout", "-q", "-b", "feat"]);
    commit(&work, "src/a.rs", "fn a() { 8 }\n", "a8");
    // The branch edits the rubric: the target's rubric still counts.
    commit(&work, "rubric/SKILL.md", "# weakened\n", "rubric on branch");
    let k = plan_at(&work, "origin/main", "HEAD").segments[0]
        .key
        .clone();
    assert_eq!(plan_at(&work, "origin/main", "HEAD").segments[0].key, k);

    git(&work, &["checkout", "-q", "main"]);
    commit(&work, "rubric/SKILL.md", "# rubric v2\n", "rubric v2");
    git(&work, &["push", "-q", "origin", "main"]);
    git(&work, &["checkout", "-q", "feat"]);
    let k2 = plan_at(&work, "origin/main", "HEAD").segments[0]
        .key
        .clone();
    assert_ne!(k2, k);

    git(&work, &["checkout", "-q", "main"]);
    let cfg = common::CONFIG.replace("20m", "30m");
    commit(&work, ".vega/gate.toml", &cfg, "config");
    git(&work, &["push", "-q", "origin", "main"]);
    git(&work, &["checkout", "-q", "feat"]);
    assert_ne!(plan_at(&work, "origin/main", "HEAD").segments[0].key, k2);
}

#[test]
fn changes_outside_the_paths_need_no_review() {
    let tmp = tempfile::tempdir().unwrap();
    let work = repo(tmp.path());
    git(&work, &["checkout", "-q", "-b", "docs"]);
    commit(&work, "docs/x.md", "more\n", "docs");
    commit(&work, "src/fixtures/f.txt", "data\n", "fixture");
    let p = plan_at(&work, "origin/main", "HEAD");
    assert!(p.segments.is_empty());
    assert_eq!(p.empty, 1);
}
