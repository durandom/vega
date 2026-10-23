//! Deterministic stale-lookup races and storage-error recovery. Kept in one
//! test process because state/tool locations and the author are environment-owned.
mod common;

use std::fs;
use std::path::Path;

use vega_gate::{config, segments, start, state, verdict};

fn setup(tmp: &Path) -> (std::path::PathBuf, config::Config, state::Review) {
    std::env::set_var("XDG_STATE_HOME", tmp.join("state"));
    std::env::set_var("XDG_CACHE_HOME", tmp.join("cache"));
    std::env::set_var("HERDR_PANE_ID", "w1:p1");
    for (key, value) in common::fakes(tmp) {
        std::env::set_var(key, value);
    }
    let work = common::repo(tmp);
    common::git(&work, &["checkout", "-q", "-b", "feature"]);
    common::commit(&work, "src/a.rs", "fn a() { 1 }\n", "initial change");
    let cfg = config::parse(common::CONFIG).unwrap();
    let plan = segments::plan(&work, &cfg, common::CONFIG, "origin/main", "HEAD").unwrap();
    assert!(matches!(
        start::start(&work, &plan, &cfg, "feature").unwrap(),
        start::Outcome::Running(_)
    ));
    let r = state::all().pop().unwrap();
    (work, cfg, r)
}

fn verdict_first(pass: bool) {
    let tmp = tempfile::tempdir().unwrap();
    let (work, cfg, r) = setup(tmp.path());
    let head = common::commit(&work, "src/a.rs", "fn a() { 2 }\n", "new head");
    let plan = segments::plan(&work, &cfg, common::CONFIG, "origin/main", "HEAD").unwrap();
    // start has read the old reviewer while planning the new missing segment.
    let lookup = start::lookup(&work, &plan).unwrap();
    assert_eq!(lookup.reviews, vec![r.clone()]);
    assert!(!lookup.missing.is_empty());
    // The verdict wins before start can acquire the lock. Its tab still owns
    // the name: the fake spawn fails with name_taken until herdr closes it.
    fs::write(tmp.path().join("old-pane-alive"), "occupied").unwrap();
    verdict::give(pass, &[], &r).unwrap();
    let result = start::start_from(&work, &plan, &cfg, "feature", lookup).unwrap();
    assert!(matches!(result, start::Outcome::Running(_)), "{result:?}");
    assert!(!tmp.path().join("old-pane-alive").exists());
    assert!(common::log(tmp.path(), "herdr").contains("pane close w9:p5"));
    let bd = common::log(tmp.path(), "bd");
    assert!(bd.contains(if pass { "verdict=pass" } else { "verdict=fail" }));
    assert!(!bd.contains("superseded"), "{bd}");
    assert!(!bd.contains("abandoned"), "{bd}");
    let replacement = state::all().pop().unwrap();
    assert_eq!(replacement.head, head);
    assert_ne!(replacement.pane, r.pane);
}

fn storage_error_retains_the_original_reviewer() {
    let tmp = tempfile::tempdir().unwrap();
    let (work, cfg, r) = setup(tmp.path());
    common::commit(&work, "src/a.rs", "fn a() { 2 }\n", "new head");
    let plan = segments::plan(&work, &cfg, common::CONFIG, "origin/main", "HEAD").unwrap();
    fs::write(tmp.path().join("fail-supersede"), "fail").unwrap();
    let error = start::start(&work, &plan, &cfg, "feature").unwrap_err();
    assert!(error.contains("injected storage failure"), "{error}");
    assert_eq!(state::load(&r.pane), Some(r.clone()));
    assert_eq!(common::git(&r.clone, &["rev-parse", "HEAD"]), r.head);
    assert!(!common::log(tmp.path(), "herdr").contains("pane close"));
    // Once storage recovers, the original exact-head reviewer can still
    // submit its verdict; it was not orphaned by the failed replacement.
    fs::remove_file(tmp.path().join("fail-supersede")).unwrap();
    verdict::give(true, &[], &r).unwrap();
    assert!(common::log(tmp.path(), "bd").contains("verdict=pass"));
}

fn replacement_waits_for_verdict_mail() {
    use std::time::{Duration, Instant};

    let tmp = tempfile::tempdir().unwrap();
    let (work, cfg, r) = setup(tmp.path());
    common::commit(&work, "src/a.rs", "fn a() { 2 }\n", "new head");
    let plan = segments::plan(&work, &cfg, common::CONFIG, "origin/main", "HEAD").unwrap();
    let lookup = start::lookup(&work, &plan).unwrap();
    fs::write(tmp.path().join("old-pane-alive"), "occupied").unwrap();
    fs::write(tmp.path().join("block-mail"), "block").unwrap();
    std::thread::scope(|scope| {
        let giving = scope.spawn(|| verdict::give(true, &[], &r));
        let deadline = Instant::now() + Duration::from_secs(5);
        while !tmp.path().join("mail-entered").exists() {
            assert!(Instant::now() < deadline, "verdict never reached mail");
            std::thread::sleep(Duration::from_millis(10));
        }
        // The verdict's state is gone, but it still owns the lock while
        // mailing. A no-op lock lets start close its pane prematurely.
        assert!(state::load(&r.pane).is_none());
        let replacing = scope.spawn(|| start::start_from(&work, &plan, &cfg, "feature", lookup));
        std::thread::sleep(Duration::from_millis(500));
        let pane_survived_mail = tmp.path().join("old-pane-alive").exists();
        fs::write(tmp.path().join("release-mail"), "release").unwrap();
        giving.join().unwrap().unwrap();
        let result = replacing.join().unwrap().unwrap();
        assert!(
            pane_survived_mail,
            "start closed the reviewer before verdict mail completed"
        );
        assert!(matches!(result, start::Outcome::Running(_)), "{result:?}");
    });
    assert!(!tmp.path().join("old-pane-alive").exists());
    assert!(!common::log(tmp.path(), "bd").contains("abandoned"));
}

#[test]
fn verdict_first_releases_the_tab_and_storage_errors_remain_retryable() {
    verdict_first(true);
    verdict_first(false);
    storage_error_retains_the_original_reviewer();
    replacement_waits_for_verdict_mail();
}
