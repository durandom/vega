//! Live check against github.com through the real `gh` (ignored by default:
//! it needs gh auth and costs a few core requests the first time).
//!
//! VEGA_CI_LIVE_PR=cli/cli#14640 cargo test -p vega-ci-watch --test live -- --ignored --nocapture

use std::cell::Cell;

use vega_ci_watch::poll::cycle;
use vega_ci_watch::store::{self, Watch};
use vega_ci_watch::Target;
use vega_ci_watch::{forge, gh};

#[test]
#[ignore = "requires live GitHub API and authenticated gh; excluded from hermetic CI"]
fn second_poll_is_all_304_and_free() {
    let label = std::env::var("VEGA_CI_LIVE_PR").unwrap_or_else(|_| "cli/cli#14640".into());
    let tmp = tempfile::tempdir().unwrap();
    std::env::set_var("XDG_STATE_HOME", tmp.path());
    let t = Target::parse_label(&label, &mut forge::http_probe).expect("owner/repo#n");
    for pane in ["live:p1", "live:p2"] {
        store::add_watch(&Watch {
            target: t.clone(),
            pane: pane.into(),
            since: 0,
        });
    }
    let (requests, fresh) = (Cell::new(0), Cell::new(0));
    let mut fetch = |_: vega_ci_watch::forge::Forge, host: &str, path: &str, etag: Option<&str>| {
        let r = gh::get(host, path, etag)?;
        requests.set(requests.get() + 1);
        if r.status != 304 {
            fresh.set(fresh.get() + 1);
        }
        println!("{} {path} remaining={:?}", r.status, r.remaining);
        store::note_rate(&r, 0);
        Ok(r)
    };
    let live = |_: &str| true;
    let first = cycle(0, &live, &mut fetch);
    println!("first: {:?}", first.log);
    let before = store::load_rate().unwrap().remaining;
    let (r1, f1) = (requests.get(), fresh.get());
    // Due again (next poll is at most 120 s later); nothing changed meanwhile.
    cycle(10_000, &live, &mut fetch);
    let after = store::load_rate().unwrap().remaining;
    println!("first cycle: {r1} requests ({f1} 200s); second: {} requests ({} 200s); remaining {before} -> {after}",
        requests.get() - r1, fresh.get() - f1);
    assert_eq!(
        requests.get() - r1,
        r1,
        "one request per endpoint, not per watcher"
    );
    assert_eq!(fresh.get() - f1, 0, "unchanged PR: every request is a 304");
    // Other processes of the same user may spend meanwhile; ours spent none.
    assert!(after <= before);
}

/// Forgejo through the real `tea` (needs `tea login add` for the host):
/// VEGA_CI_LIVE_FORGEJO=git.b4mad.industries/goern/rs.cms#103 cargo test -p vega-ci-watch --test live forgejo -- --ignored --nocapture
#[test]
#[ignore = "requires live Forgejo API and authenticated tea; excluded from hermetic CI"]
fn forgejo_polls_through_tea() {
    let label = std::env::var("VEGA_CI_LIVE_FORGEJO")
        .unwrap_or_else(|_| "git.b4mad.industries/goern/rs.cms#103".into());
    let tmp = tempfile::tempdir().unwrap();
    std::env::set_var("XDG_STATE_HOME", tmp.path());
    let t = Target::parse_label(&label, &mut forge::http_probe).expect("host/owner/repo#n");
    assert_eq!(t.forge, forge::Forge::Forgejo);
    store::add_watch(&Watch {
        target: t.clone(),
        pane: "live:p1".into(),
        since: 0,
    });
    let mut fetch = |f: forge::Forge, host: &str, path: &str, _: Option<&str>| {
        assert_eq!(f, forge::Forge::Forgejo);
        let r = vega_ci_watch::tea::get(host, path)?;
        println!("{} {path}", r.status);
        Ok(r)
    };
    let out = cycle(0, &|_: &str| true, &mut fetch);
    println!("log: {:?}\nmails: {:?}", out.log, out.mails);
    let s = store::load_state(&t).expect("state cached");
    println!(
        "head {:?} verdict {:?} {:?}",
        s.head,
        s.summary().verdict,
        s.statuses
    );
    assert!(s.head.is_some() && s.error.is_none(), "{:?}", s.error);
}
