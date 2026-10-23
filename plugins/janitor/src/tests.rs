//! End to end on a scratch tree of real git repos.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::index;
use crate::scan::Kind;
use crate::views::{self, Filter, Live, Scope};

#[test]
fn guide_meets_the_skill_contract() {
    vega_plugin::assert_guide(crate::SKILL, "janitor");
}

/// git for building the tree, isolated from the user's config and hooks.
fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args([
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "-c",
            "commit.gpgsign=false",
        ])
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

fn commit(dir: &Path, file: &str) {
    fs::write(dir.join(file), file).unwrap();
    git(dir, &["add", file]);
    git(dir, &["commit", "-q", "-m", file]);
}

/// up/lanista.git (outside the root) and under root:
/// a/lanista (clone) + a/lanista.done (worktree, merged branch) + a gone worktree,
/// b4arena/lanista (second clone, dirty) with a nested clone, brainz.git (bare),
/// x/node_modules/dep (skipped).
fn tree() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let base = fs::canonicalize(tmp.path()).unwrap();
    let up = base.join("up");
    let root = base.join("src");
    fs::create_dir_all(up.join("seed")).unwrap();
    fs::create_dir_all(&root).unwrap();
    let seed = up.join("seed");
    git(&seed, &["init", "-q", "-b", "main"]);
    commit(&seed, "readme");
    git(&up, &["clone", "-q", "--bare", "seed", "lanista.git"]);
    let origin = up.join("lanista.git");
    let origin = origin.to_str().unwrap();

    fs::create_dir_all(root.join("a")).unwrap();
    fs::create_dir_all(root.join("b4arena")).unwrap();
    git(&root.join("a"), &["clone", "-q", origin, "lanista"]);
    git(&root.join("b4arena"), &["clone", "-q", origin, "lanista"]);
    let a = root.join("a/lanista");
    git(&a, &["branch", "done"]);
    git(&a, &["branch", "old"]);
    git(&a, &["worktree", "add", "-q", "../lanista.done", "done"]);
    git(
        &a,
        &["worktree", "add", "-q", "-b", "tmp", "../lanista.gone"],
    );
    fs::remove_dir_all(root.join("a/lanista.gone")).unwrap();
    commit(&a, "ahead");

    let b = root.join("b4arena/lanista");
    fs::write(b.join("scratch.txt"), "x").unwrap();
    git(&b, &["clone", "-q", origin, "refs-x"]);

    git(&root, &["clone", "-q", "--bare", origin, "brainz.git"]);
    fs::create_dir_all(root.join("x/node_modules/dep")).unwrap();
    git(&root.join("x/node_modules/dep"), &["init", "-q"]);
    (tmp, root, base.join("state/janitor.db"))
}

#[test]
fn scans_finds_and_reports() {
    let (_tmp, root, cache) = tree();
    let idx = index::rescan(std::slice::from_ref(&root), &cache).unwrap();
    let p = |rel: &str| root.join(rel).display().to_string();
    let get = |rel: &str| idx.entries.iter().find(|e| e.path == p(rel));

    let a = get("a/lanista").expect("clone");
    assert_eq!(a.kind, Kind::Clone);
    assert_eq!(a.branch.as_deref(), Some("main"));
    assert_eq!((a.ahead, a.dirty), (1, 0));
    assert_eq!(a.default_branch.as_deref(), Some("main"));
    assert_eq!(a.merged_branches, vec!["old"], "done is checked out");
    assert!(a.remote.as_deref().unwrap().ends_with("/up/lanista"));

    let wt = get("a/lanista.done").expect("worktree");
    assert_eq!(wt.kind, Kind::Worktree);
    assert_eq!(wt.main.as_deref(), Some(p("a/lanista").as_str()));
    assert!(wt.merged);

    let gone = get("a/lanista.gone").expect("prunable worktree");
    assert!(gone.prunable);
    assert_eq!(gone.branch.as_deref(), Some("tmp"));

    let b = get("b4arena/lanista").unwrap();
    assert_eq!(b.dirty, 2, "scratch.txt and the nested clone are untracked");
    assert!(get("b4arena/lanista/refs-x").is_some(), "nested clone");
    assert_eq!(get("brainz.git").unwrap().kind, Kind::Bare);
    assert!(get("x/node_modules/dep").is_none(), "node_modules skipped");

    // find: exact name, clones before worktrees, then fuzzy tiers.
    let hits = views::find(&idx.entries, "lanista");
    assert_eq!(hits[0].kind, Kind::Clone);
    assert_eq!(hits[1].kind, Kind::Clone);
    assert!(hits.iter().any(|e| e.path == p("a/lanista.done")));
    let hits = views::find(&idx.entries, "b4arena/lanista");
    assert_eq!(
        hits.len(),
        2,
        "substring of path: the clone and its nested clone"
    );
    assert_eq!(views::find(&idx.entries, "brnz")[0].path, p("brainz.git"));
    assert!(views::find(&idx.entries, "nothing-like-this").is_empty());

    let now = index::now();
    let live = Live::default();
    let dups = views::dups(&idx.entries);
    let lanista_dups = dups
        .values()
        .find(|v| v.len() == 4)
        .expect("4 clones of one remote");
    assert!(lanista_dups.iter().any(|e| e.path == p("brainz.git")));

    let report = views::report(&idx, &live, 6, 30, now);
    assert!(report.contains("worktree prune"), "{report}");
    assert!(report.contains("worktree remove"), "{report}");
    assert!(report.contains("branch -d old"), "{report}");
    assert!(report.contains("duplicate_clones[1]"), "{report}");
    assert!(report.contains("stale_clones: none"), "{report}");

    // A pane working in the merged worktree keeps it out of the report.
    let busy = Live {
        panes: vec![(p("a/lanista.done/src"), "w1:p1".into())],
        installs: vec![(p("b4arena/lanista/refs-x"), "uv refs".into())],
    };
    assert!(views::done_worktrees(&idx, &busy).is_empty());
    let hits = views::find(&idx.entries, "refs-x");
    let found = views::found(&idx.entries, &hits, &busy, now, "here", &[]);
    assert!(found.contains("uv refs"), "{found}");
    let list = views::list(&idx, &busy, Filter::All, 50, now, "here");
    assert!(list.contains("w1:p1"), "{list}");
    assert!(views::list(&idx, &live, Filter::Dirty, 50, now, "here").contains("dirty_checkouts[1]"));
    assert!(views::list(&idx, &live, Filter::Stale(6), 50, now, "here").contains("none"));

    let home = views::home(&idx, &live, "testhost", now);
    assert!(
        home.contains("checkouts: 5 (3 clones, 1 worktrees, 1 bare)"),
        "{home}"
    );
    assert!(home.contains("prunable worktrees,1"), "{home}");
}

#[test]
fn refreshes_only_what_moved() {
    let (_tmp, root, cache) = tree();
    let roots = vec![root.clone()];
    let first = index::rescan(&roots, &cache).unwrap();
    let mut idx = index::load_fresh(&roots, &cache).unwrap();
    assert_eq!(idx.entries, first.entries, "nothing moved");

    let a = root.join("a/lanista");
    let before = idx
        .entries
        .iter()
        .find(|e| e.path == a.display().to_string())
        .unwrap()
        .ahead;
    commit(&a, "more");
    assert!(index::refresh(&mut idx.entries));
    let e = idx
        .entries
        .iter()
        .find(|e| e.path == a.display().to_string())
        .unwrap();
    assert_eq!(e.ahead, before + 1);

    fs::remove_dir_all(root.join("b4arena")).unwrap();
    assert!(index::refresh(&mut idx.entries));
    assert!(idx.entries.iter().all(|e| !e.path.contains("b4arena")));
}

#[test]
fn daily_rewalk_finds_new_clones_and_keeps_known_ones() {
    let (_tmp, root, cache) = tree();
    let roots = vec![root.clone()];
    let mut idx = index::rescan(&roots, &cache).unwrap();
    let origin = root.join("b4arena/lanista");
    git(&root, &["clone", "-q", origin.to_str().unwrap(), "fresh"]);
    fs::remove_dir_all(root.join("brainz.git")).unwrap();
    idx.walked_at -= index::MAX_AGE + 1;
    index::rewalk(&roots, &mut idx);
    let paths: Vec<&str> = idx.entries.iter().map(|e| e.path.as_str()).collect();
    assert!(paths.contains(&root.join("fresh").to_str().unwrap()));
    assert!(!paths.iter().any(|p| p.ends_with("brainz.git")));
    assert_eq!(idx.entries.iter().filter(|e| e.prunable).count(), 1);
}

fn skill(dir: &Path, desc: &str) {
    fs::create_dir_all(dir).unwrap();
    fs::write(
        dir.join("SKILL.md"),
        format!("---\nname: x\ndescription: {desc}\n---\nbody\n"),
    )
    .unwrap();
}

#[test]
fn indexes_skills_and_answers_which_checkout() {
    let (_tmp, root, cache) = tree();
    let a = root.join("a/lanista");
    let b = root.join("b4arena/lanista");
    skill(
        &a.join(".agents/skills/discord"),
        "Talks to Discord channels.",
    );
    fs::create_dir_all(a.join(".claude/skills")).unwrap();
    std::os::unix::fs::symlink(
        "../../.agents/skills/discord",
        a.join(".claude/skills/discord"),
    )
    .unwrap();
    std::os::unix::fs::symlink("../nowhere", a.join(".claude/skills/ghost")).unwrap();
    skill(&a.join(".claude/skills/.hidden"), "hidden");
    fs::create_dir_all(a.join(".claude/skills/nomd")).unwrap();
    skill(&a.join("skills/lanista-own"), "The repo's own skill.");
    fs::write(
        a.join("skills-lock.json"),
        r#"{"version":1,"skills":{"discord":{"source":"o/discord"}}}"#,
    )
    .unwrap();
    skill(&b.join(".pi/skills/discord-bot"), "A bot.");
    skill(
        &root.join("a/lanista.done/.codex/skills/discord"),
        "wt copy",
    );
    git(
        &a,
        &[
            "remote",
            "set-url",
            "origin",
            "git@github.com:openclaw/lanista.git",
        ],
    );
    git(
        &b,
        &[
            "remote",
            "set-url",
            "origin",
            "https://github.com/me/lanista",
        ],
    );

    let roots = vec![root.clone()];
    let mut idx = index::rescan(&roots, &cache).unwrap();
    let p = |rel: &str| root.join(rel).display().to_string();
    let a_e = idx
        .entries
        .iter()
        .find(|e| e.path == p("a/lanista"))
        .unwrap();
    let names: Vec<&str> = a_e.skills.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(
        names,
        ["discord", "ghost", "lanista-own"],
        "dot and SKILL.md-less dirs skipped"
    );
    let d = &a_e.skills[0];
    assert_eq!(d.dirs(), ".agents .claude");
    assert_eq!(d.kinds(), "dir | link -> ../../.agents/skills/discord");
    assert_eq!(d.source(), "o/discord");
    assert_eq!(d.description, "Talks to Discord channels.");
    assert_eq!(a_e.skills[1].kinds(), "broken");
    assert_eq!(a_e.skills[2].source(), "own");

    let all = Scope {
        mine: None,
        worktrees: false,
    };
    let hits = views::skill_hits(&idx.entries, &all, Some("discord"), false);
    assert_eq!(hits.len(), 1, "exact tier only, worktree left out");
    let out = views::skill_rows(&hits, 50);
    assert!(
        out.starts_with("skills[1]{skill,checkout,dirs,kind,source,remote}:"),
        "{out}"
    );
    assert!(out.contains("o/discord,openclaw/lanista"), "{out}");
    let wt = Scope {
        mine: None,
        worktrees: true,
    };
    assert_eq!(
        views::skill_hits(&idx.entries, &wt, Some("discord"), false).len(),
        2
    );
    // --mine hides the upstream repo: the substring tier finds the user's own.
    let mine = Scope {
        mine: Some("ME"),
        worktrees: false,
    };
    let hits = views::skill_hits(&idx.entries, &mine, Some("discord"), false);
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].0.name, "discord-bot");
    assert_eq!(
        views::skill_hits(&idx.entries, &all, Some("dscbt"), false)[0]
            .0
            .name,
        "discord-bot"
    );
    let by_desc = views::skill_hits(&idx.entries, &all, Some("own skill"), true);
    assert_eq!(by_desc[0].0.name, "lanista-own");
    assert!(views::skill_hits(&idx.entries, &all, Some("own skill"), false).is_empty());

    let counts = views::skill_counts(&views::skill_hits(&idx.entries, &wt, None, false), 50);
    assert!(
        counts.starts_with("skills[4]{skill,checkouts,description}:\n  discord,2,"),
        "{counts}"
    );

    // A skill added without a git command waits for the next query of its checkout.
    skill(&b.join(".agents/skills/late"), "late");
    index::refresh(&mut idx.entries);
    assert!(views::skill_hits(&idx.entries, &all, Some("late"), false).is_empty());
    commit(&b, "touch");
    assert!(index::refresh(&mut idx.entries));
    assert_eq!(
        views::skill_hits(&idx.entries, &all, Some("late"), false).len(),
        1
    );
}

#[test]
fn store_round_trips_and_imports_the_json_cache() {
    let (_tmp, root, cache) = tree();
    let roots = vec![root.clone()];
    let mut idx = index::rescan(&roots, &cache).unwrap();
    let back = index::load_fresh(&roots, &cache).unwrap();
    assert_eq!(back.entries, idx.entries);
    assert_eq!(back.walked_at, idx.walked_at);

    // A row update, not a rewrite: one row goes, the rest stays.
    let gone = idx.entries.remove(0).path;
    index::save(&cache, &idx).unwrap();
    let back = index::load_fresh(&roots, &cache).unwrap();
    assert!(back.entries.iter().all(|e| e.path != gone));
    assert_eq!(back.entries.len(), idx.entries.len());

    // The v1 JSON cache is imported instead of rescanned.
    idx.version = 3;
    idx.scan_ms = 4242;
    fs::write(
        cache.with_file_name("index.json"),
        serde_json::to_vec(&idx).unwrap(),
    )
    .unwrap();
    fs::remove_file(&cache).unwrap();
    let imported = index::load_fresh(&roots, &cache).unwrap();
    assert_eq!(imported.scan_ms, 4242);
    assert_eq!(imported.entries.len(), idx.entries.len());
}

#[test]
fn other_hosts_rows_come_from_peer_files() {
    let (_tmp, root, cache) = tree();
    let roots = vec![root.clone()];
    let dir = cache.parent().unwrap().to_path_buf();
    index::rescan(&roots, &cache).unwrap();
    let summary: crate::peers::Summary =
        serde_json::from_slice(&fs::read(dir.join("summary.json")).unwrap()).unwrap();
    assert!(summary.rows.iter().any(|r| r.0.ends_with("a/lanista")));

    // vegad drops ganymede's summary; the next call imports it.
    fs::create_dir_all(dir.join("peers")).unwrap();
    let ganymede = crate::peers::Summary {
        at: index::now() - 3 * 86400,
        rows: vec![(
            "~/src/durandom/lanista".into(),
            "clone".into(),
            Some("github.com/b4arena/lanista".into()),
            Some("old".into()),
            -1,
            0,
            2,
            index::now() - 86400,
        )],
    };
    fs::write(
        dir.join("peers/ganymede.json"),
        serde_json::to_vec(&ganymede).unwrap(),
    )
    .unwrap();
    let idx = index::load_fresh(&roots, &cache).unwrap();
    assert_eq!(idx.hosts.len(), 1);
    let mut hits = views::find(&idx.entries, "lanista");
    let local = hits.len();
    hits.extend(views::find(&idx.peers, "lanista"));
    assert_eq!(hits.len(), local + 1, "ganymede's checkout too");
    let out = views::found(
        &idx.entries,
        &hits,
        &Live::default(),
        index::now(),
        "here",
        &idx.hosts,
    );
    assert!(
        out.contains("ganymede,~/src/durandom/lanista,clone,old,?,0,2,1d,"),
        "{out}"
    );
    assert!(
        out.contains("other hosts as of: ganymede 3d ago stale"),
        "{out}"
    );
    assert!(out.contains("\n  here,"), "{out}");
    let list = views::list(&idx, &Live::default(), Filter::All, 0, index::now(), "here");
    assert!(list.contains("ganymede,~/src/durandom/lanista"), "{list}");
    let report = views::report(&idx, &Live::default(), 6, 0, index::now());
    assert!(!report.contains("ganymede"), "report stays local");

    // The file is gone (the host's key left the registry): so are its rows.
    fs::remove_file(dir.join("peers/ganymede.json")).unwrap();
    let idx = index::load_fresh(&roots, &cache).unwrap();
    assert!(idx.peers.is_empty() && idx.hosts.is_empty());
}

#[test]
fn readmes_are_searched_by_words_and_by_meaning() {
    use crate::search;
    let (_tmp, root, cache) = tree();
    let roots = vec![root.clone()];
    index::rescan(&roots, &cache).unwrap();
    let db = index::open(&cache).unwrap();
    assert!(search::words(&db, "gladiator", 10).unwrap().is_empty());

    // A commit moves the fingerprint: the README is read again.
    let a = root.join("a/lanista");
    fs::write(
        a.join("README.md"),
        "# lanista\n\n[![ci](x)](y)\nTrains gladiator agents for the arena.\n",
    )
    .unwrap();
    git(&a, &["add", "README.md"]);
    git(&a, &["commit", "-q", "-m", "readme"]);
    index::load_fresh(&roots, &cache).unwrap();
    let a = a.display().to_string();
    assert_eq!(
        search::words(&db, "arena gladiators gladiator", 10).unwrap(),
        vec![a.clone()]
    );
    assert_eq!(
        search::about(&db, &a),
        "Trains gladiator agents for the arena."
    );

    // A fake model: one dimension per word.
    let fake = |texts: &[String]| -> Result<Vec<Vec<f32>>, String> {
        Ok(texts
            .iter()
            .map(|t| {
                let n = |w: &str| t.matches(w).count() as f32;
                vec![n("gladiator"), n("readme"), 0.1]
            })
            .collect())
    };
    let mut db = index::open(&cache).unwrap();
    let first = search::embed_all(&mut db, "m1", fake).unwrap();
    assert_eq!(
        (first.new, first.total),
        (2, 3),
        "three clones, two share one README"
    );
    assert_eq!(
        search::embed_all(&mut db, "m1", fake).unwrap().new,
        0,
        "cached by key"
    );
    assert_eq!(search::vectors_of(&db, "m1"), 3);
    assert_eq!(
        search::vectors_of(&db, "m2"),
        0,
        "another model needs its own"
    );
    let hits = search::meaning(&db, &[1.0, 0.0, 0.0], 10).unwrap();
    assert_eq!(hits[0].0, a);
}

#[test]
fn history_records_changes_and_vanished_checkouts() {
    use crate::history;
    let (_tmp, root, cache) = tree();
    let roots = vec![root.clone()];
    index::rescan(&roots, &cache).unwrap();
    let a = root.join("a/lanista");
    commit(&a, "more");
    fs::write(a.join("wip.txt"), "x").unwrap();
    git(&a, &["add", "wip.txt"]);
    fs::remove_dir_all(root.join("a/lanista.done")).unwrap();
    let idx = index::load_fresh(&roots, &cache).unwrap();
    assert!(idx.last_dirty.contains_key(&a.display().to_string()));

    let db = index::open(&cache).unwrap();
    let paths = crate::history_paths(&idx, &db, "lanista.done").unwrap();
    assert_eq!(
        paths,
        vec![root.join("a/lanista.done").display().to_string()]
    );
    let events = history::events(&db, &paths, 0).unwrap();
    let what: Vec<&str> = events.iter().map(|e| e.what.as_str()).collect();
    assert_eq!(what, ["vanished", "appeared"]);

    let a = a.display().to_string();
    let events = history::events(&db, std::slice::from_ref(&a), 0).unwrap();
    let what: Vec<&str> = events.iter().map(|e| e.what.as_str()).collect();
    assert!(
        what.contains(&"commit") && what.contains(&"dirty"),
        "{what:?}"
    );
    let out = views::history(&events, index::now());
    assert!(out.contains(",dirty,1 files\n"), "{out}");

    // Stale rows say since when.
    let mut idx = idx;
    for e in &mut idx.entries {
        e.touched -= 400 * 86400;
    }
    let list = views::list(
        &idx,
        &Live::default(),
        Filter::Stale(6),
        0,
        index::now(),
        "here",
    );
    assert!(list.contains(",stale_since,last_dirty}"), "{list}");
}
