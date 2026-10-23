//! The ID and owner-local location views read the same v2 claim. No legacy
//! files participate, and filesystem consumers never receive a remote ID.
use std::path::Path;
use std::process::Command;

use serde_json::json;
use vega_herdr::{lanes, project, state};

fn git(path: &Path, args: &[&str]) {
    let out = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn v2_claim_remains_a_local_git_repo_and_records_lanes_with_registry_host_alias() {
    const CHILD: &str = "VEGA_LOCATION_VIEW_FIXTURE";
    if std::env::var_os(CHILD).is_none() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path().canonicalize().unwrap();
        let out = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "project_location_test::v2_claim_remains_a_local_git_repo_and_records_lanes_with_registry_host_alias"])
            .env(CHILD, "1").env("HOME", &root)
            .env("XDG_STATE_HOME", root.join("state"))
            .env("VEGA_POST_HOST", format!("{}-registry-fixture", vega_herdr::local_host()))
            .env("GIT_CONFIG_GLOBAL", "/dev/null").env("GIT_CONFIG_NOSYSTEM", "1")
            .output().unwrap();
        assert!(
            out.status.success(),
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        return;
    }
    let root = std::path::PathBuf::from(std::env::var_os("HOME").unwrap());
    let repo = root.join("repo");
    std::fs::create_dir(&repo).unwrap();
    git(&repo, &["init", "-q"]);
    git(
        &repo,
        &[
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "commit",
            "--no-gpg-sign",
            "--allow-empty",
            "-qm",
            "fixture",
        ],
    );
    git(
        &repo,
        &["remote", "add", "origin", "git@github.com:team/repo.git"],
    );
    let lane = root.join("lane");
    git(
        &repo,
        &["worktree", "add", "-qb", "lane", lane.to_str().unwrap()],
    );
    let project = project::resolve(&repo, &state::post_host()).unwrap();
    let key = vega_herdr::home_relative(project.location.common_dir.to_str().unwrap());
    let file = state::project_file("uac", &project.id).unwrap();
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, json!({"schema":2,"project":project.id,"common_dir":project.location.common_dir,"checkout":project.location.checkout,"pane":"w1:p1","host":state::post_host(),"at":10}).to_string()).unwrap();
    let legacy = state::state_file_on("uac", None, &key).unwrap();
    std::fs::write(
        &legacy,
        json!({"project":key,"pane":"legacy","host":"old","at":1}).to_string(),
    )
    .unwrap();
    assert_eq!(
        state::project_uac_records(),
        vec![(project.id.to_string(), "w1:p1".into(), 10)]
    );
    assert_eq!(
        state::uac_records(),
        vec![(key.clone(), "w1:p1".into(), 10)]
    );
    assert_eq!(
        state::uac_of(&key),
        Some(("w1:p1".into(), state::post_host()))
    );
    let repos = vega_bd_sync::git::repos(vega_bd_sync::git::claimed());
    assert_eq!(repos.len(), 1);
    assert_eq!(repos[0].common, project.location.common_dir);
    assert!(lanes::record_if_uac(&lane, &repo, "wrong-pane")
        .unwrap()
        .is_none());
    assert!(lanes::record_if_uac(&lane, &repo, "w1:p1")
        .unwrap()
        .is_some());
    assert_eq!(lanes::lanes(project.id.as_str()).len(), 1);
    assert_ne!(state::post_host(), vega_herdr::local_host());
}

fn isolated(name: &str) -> bool {
    if std::env::var("VEGA_ID_CONSUMER_FIXTURE").as_deref() == Ok(name) {
        return true;
    }
    let root = tempfile::tempdir().unwrap();
    let root = root.path().canonicalize().unwrap();
    let out = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", &format!("project_location_test::{name}")])
        .env("VEGA_ID_CONSUMER_FIXTURE", name)
        .env("HOME", &root)
        .env("XDG_STATE_HOME", root.join("state"))
        .env("VEGA_POST_HOST", "fixture")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    false
}

#[test]
fn decisions_use_the_claimed_bare_checkout_and_refresh_its_own_journal() {
    if !isolated("decisions_use_the_claimed_bare_checkout_and_refresh_its_own_journal") {
        return;
    }
    use std::os::unix::fs::PermissionsExt;
    use vega_bd_sync::Project;
    let root = std::path::PathBuf::from(std::env::var_os("HOME").unwrap());
    let seed = root.join("seed");
    std::fs::create_dir(&seed).unwrap();
    git(&seed, &["init", "-q"]);
    git(
        &seed,
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "--allow-empty",
            "-qm",
            "fixture",
        ],
    );
    let common = root.join("a/.bare");
    std::fs::create_dir_all(&common).unwrap();
    git(&common, &["init", "--bare", "-q"]);
    git(&common, &["fetch", "-q", seed.to_str().unwrap(), "HEAD"]);
    let checkout = root.join("a/main");
    git(
        &common,
        &[
            "worktree",
            "add",
            "-qb",
            "main",
            checkout.to_str().unwrap(),
            "FETCH_HEAD",
        ],
    );
    git(
        &common,
        &[
            "remote",
            "add",
            "origin",
            "https://github.com/team/repo.git",
        ],
    );
    let other = root.join("b");
    std::fs::create_dir(&other).unwrap();
    git(&other, &["init", "-q"]);
    git(
        &other,
        &["remote", "add", "origin", "git@github.com:team/repo.git"],
    );
    let bd = root.join("fake-bd");
    std::fs::write(
        &bd,
        "#!/bin/sh\npwd >> \"$HOME/calls\"\ncat decisions.json\n",
    )
    .unwrap();
    std::fs::set_permissions(&bd, std::fs::Permissions::from_mode(0o755)).unwrap();
    let registration = |dir: &Path, common: &Path| Project {
        key: "github.com/team/repo".into(),
        common: common.into(),
        dir: dir.into(),
        database: "DB".into(),
        remote: "http://remote/DB".into(),
        bd: bd.clone(),
        data: dir.join(".beads/dolt"),
        since: 1,
        resumed: 0,
    };
    let a = registration(&checkout, &common);
    let b = registration(&other, &other.join(".git"));
    let decisions = |id: &str| {
        json!([{"id":"tracker","issue_type":"epic","status":"open","labels":["uac-tracker"]},{"id":id,"parent":"tracker","status":"open","labels":["slayer-decision"],"title":id}]).to_string()
    };
    std::fs::write(checkout.join("decisions.json"), decisions("a1")).unwrap();
    std::fs::write(other.join("decisions.json"), decisions("b1")).unwrap();
    let claim = state::project_file("uac", &a.key).unwrap();
    std::fs::create_dir_all(claim.parent().unwrap()).unwrap();
    std::fs::write(&claim, json!({"schema":2,"project":a.key,"host":"fixture","pane":"p1","at":1,"common_dir":common,"checkout":checkout}).to_string()).unwrap();
    let mut cache = crate::DecisionCache::default();
    let projects = vec![b, a.clone()]; // the other clone must not win by order
    assert_eq!(cache.list(&a.key, &projects).unwrap()[0].id, "a1");
    std::fs::write(checkout.join("decisions.json"), decisions("a2")).unwrap();
    assert_eq!(
        cache.list(&a.key, &projects).unwrap()[0].id,
        "a1",
        "cache before journal change"
    );
    std::fs::create_dir_all(a.noms()).unwrap();
    std::fs::write(a.noms().join("journal"), "write").unwrap();
    assert_eq!(
        cache.list(&a.key, &projects).unwrap()[0].id,
        "a2",
        "its journal refreshes before the minute expires"
    );
    let calls = std::fs::read_to_string(root.join("calls")).unwrap();
    assert_eq!(
        calls.lines().collect::<Vec<_>>(),
        vec![checkout.to_str().unwrap(), checkout.to_str().unwrap()]
    );
    assert!(cache.0.contains_key(&(a.key.clone(), common.clone())));
    let known = cache.list(&a.key, &projects).unwrap();
    crate::save_project_badges(&a.key, 0, &[], Some(&known), 10);
    let config = std::fs::read(common.join("config")).unwrap();
    std::fs::write(common.join("config"), "[broken configuration").unwrap();
    let retained = cache.list(&a.key, &projects).unwrap();
    assert_eq!(retained[0].id, "a2");
    crate::save_project_badges(&a.key, 0, &[], Some(&retained), 11);
    assert_eq!(vega_herdr::badges::load(&a.key, 11).unwrap().0.decisions, 1);
    let unknown = crate::DecisionCache::default().list(&a.key, &projects);
    assert!(unknown.is_none());
    let after_expiry = 11 + vega_herdr::badges::STALE_SECS + 1;
    assert!(vega_herdr::badges::load(&a.key, after_expiry).is_none());
    crate::save_project_badges(&a.key, 0, &[], unknown.as_deref(), after_expiry);
    assert_eq!(
        vega_herdr::badges::load(&a.key, after_expiry)
            .unwrap()
            .0
            .decisions,
        1
    );
    std::fs::write(common.join("config"), config).unwrap();

    // The ID joins notes to the owning UAC, independently of the vega home badge.
    let note = vega_herdr::notes::Note {
        project: Some(a.key.clone()),
        cause: Some("bd-sync:DB".into()),
        ..Default::default()
    };
    let uacs = vec![a.key.clone(), "github.com/team/vega".into()];
    assert!(note.counts_on(&a.key, Some("github.com/team/vega"), &uacs));
    assert!(!note.counts_on("github.com/team/vega", Some("github.com/team/vega"), &uacs));
    let count = vega_herdr::badges::Badges {
        notes: 1,
        ..Default::default()
    };
    vega_herdr::badges::save(&a.key, &count, &[], 10);
    assert_eq!(vega_herdr::badges::load(&a.key, 10).unwrap().0.notes, 1);
    let wrong = state::project_file("badges", "github.com/team/fork").unwrap();
    std::fs::copy(state::project_file("badges", &a.key).unwrap(), wrong).unwrap();
    assert!(vega_herdr::badges::load("github.com/team/fork", 10).is_none());
}

#[test]
fn layout_resolves_local_clones_and_forks_before_id_membership() {
    if !isolated("layout_resolves_local_clones_and_forks_before_id_membership") {
        return;
    }
    let root = std::path::PathBuf::from(std::env::var_os("HOME").unwrap());
    let mut panes = Vec::new();
    let mut workspaces = Vec::new();
    for (name, url) in [
        ("a", "https://github.com/team/repo.git"),
        ("b", "git@github.com:team/repo.git"),
        ("fork", "https://github.com/other/fork.git"),
    ] {
        let path = root.join(name);
        std::fs::create_dir(&path).unwrap();
        git(&path, &["init", "-q"]);
        git(&path, &["remote", "add", "origin", url]);
        panes.push(
            json!({"pane_id":name,"workspace_id":name,"cwd":path,"terminal_title":"vega-board"}),
        );
        workspaces.push(json!({"workspace_id":name,"worktree":{"repo_key":path.join(".git"),"checkout_path":path}}));
    }
    let layout = crate::Layout {
        panes,
        workspaces,
        tabs: Vec::new(),
    }
    .resolve_projects();
    assert!(layout.in_project("github.com/team/repo", "a"));
    assert!(layout.in_project("github.com/team/repo", "b"));
    assert!(!layout.in_project("github.com/team/repo", "fork"));
    assert_eq!(
        layout.project_workspaces("github.com/team/repo"),
        vec!["a", "b"]
    );
    let boards = layout.boards(&["github.com/team/repo".into()]);
    assert_eq!(boards[2].1.as_deref(), Some("github.com/other/fork"));
}
