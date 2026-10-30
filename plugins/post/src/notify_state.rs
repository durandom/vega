//! Origin-only lifecycle cleanup; delivery belongs to later notify beads.
use crate::{bead_closed, Supervisor};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use vega_herdr::{
    asks,
    notes::{Note, Store},
};

static WRITE_FAILED: AtomicBool = AtomicBool::new(false);

pub fn refresh(sup: &Supervisor) {
    let Some(store) = Store::local() else {
        return;
    };
    let host = vega_herdr::state::post_host();
    let projects = vega_bd_sync::store::projects();
    match refresh_with(
        &store,
        &host,
        |p| sup.alive(p),
        |n| closed_bead(n, &projects),
    ) {
        Ok(_) => {
            WRITE_FAILED.store(false, Ordering::Relaxed);
        }
        Err(e) => {
            if !WRITE_FAILED.swap(true, Ordering::Relaxed) {
                crate::log(&format!("notify: cannot resolve asks: {e}"));
            }
        }
    }
}

fn refresh_with(
    store: &Store,
    host: &str,
    pane_alive: impl Fn(&str) -> Option<bool>,
    bead_closed: impl Fn(&Note) -> Option<bool>,
) -> std::io::Result<usize> {
    let mut count = 0;
    // Probe outside the ask lock; recheck the exact note snapshot on write so
    // a concurrent re-raise cannot resolve a newly retargeted ask.
    for n in store
        .list()
        .into_iter()
        .filter(|n| n.kind == "ask" && n.resolved.is_none() && n.host == host)
    {
        let alive = n.pane.as_deref().and_then(&pane_alive);
        let closed = if alive == Some(false) {
            None
        } else {
            bead_closed(&n)
        };
        if asks::cleared(&n, alive, closed) {
            count += asks::change(
                store,
                host,
                (vega_herdr::notes::now_ms() / 1000) as i64,
                false,
                |current| current == &n,
            )?;
        }
    }
    Ok(count)
}

fn closed_bead(n: &Note, projects: &[vega_bd_sync::Project]) -> Option<bool> {
    let bead = n.bead.as_deref()?;
    let key = n.project.as_deref()?;
    let candidates: Vec<_> = projects.iter().filter(|p| p.key == key).collect();
    let location =
        vega_herdr::state::project_uac_location(key).or_else(|| match candidates.as_slice() {
            [p] => Some(vega_herdr::project::RepoLocation {
                common_dir: p.common.clone(),
                checkout: p.dir.clone(),
            }),
            _ => None,
        })?;
    // Local paths only, and only while they still identify this project.
    let current =
        vega_herdr::project::resolve(&location.checkout, &vega_herdr::state::post_host()).ok()?;
    if current.id.as_str() != key || current.location.common_dir != location.common_dir {
        return None;
    }
    let bd = projects
        .iter()
        .find(|p| p.key == key && p.common == location.common_dir)
        .or_else(|| projects.first())?
        .bd
        .clone();
    let out = vega_herdr::output_within(
        std::process::Command::new(bd)
            .args(["show", bead, "--json"])
            .current_dir(location.checkout),
        Duration::from_secs(5),
    )
    .ok()??;
    if !out.status.success() {
        return None;
    }
    bead_closed(&String::from_utf8_lossy(&out.stdout))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn lifecycle_resolves_local_pane_and_bead_but_unknown_stays_open() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store {
            dir: dir.path().into(),
        };
        let now = vega_herdr::notes::now_ms();
        for (host, key, pane, bead) in [
            ("a", "gone", "closed", None),
            ("a", "bead", "live", Some("done")),
            ("a", "unknown", "unknown", Some("unknown")),
            ("b", "remote", "closed", None),
        ] {
            asks::raise(
                &store,
                Note {
                    host: host.into(),
                    kind: "ask".into(),
                    level: Some("action".into()),
                    key: Some(key.into()),
                    cause: Some(format!("ask:{key}")),
                    pane: Some(pane.into()),
                    bead: bead.map(String::from),
                    ..Default::default()
                },
                now,
            )
            .unwrap();
        }
        let alive = |p: &str| match p {
            "closed" => Some(false),
            "live" => Some(true),
            _ => None,
        };
        let closed = |n: &Note| match n.bead.as_deref() {
            Some("done") => Some(true),
            _ => None,
        };
        assert_eq!(refresh_with(&store, "a", alive, closed).unwrap(), 2);
        assert_eq!(refresh_with(&store, "a", alive, closed).unwrap(), 0);
        let remaining = asks::open(store.list(), vec![], "a");
        assert_eq!(remaining.len(), 2);
        assert!(remaining
            .iter()
            .any(|n| n.key.as_deref() == Some("unknown")));
        assert!(remaining.iter().any(|n| n.host == "b"));
    }
    #[test]
    fn concurrent_retarget_does_not_resolve_new_question() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store {
            dir: dir.path().into(),
        };
        let now = vega_herdr::notes::now_ms();
        let note = Note {
            host: "a".into(),
            kind: "ask".into(),
            level: Some("action".into()),
            key: Some("key".into()),
            cause: Some("ask:key".into()),
            bead: Some("old".into()),
            ..Default::default()
        };
        asks::raise(&store, note.clone(), now).unwrap();
        assert_eq!(
            refresh_with(
                &store,
                "a",
                |_| None,
                |_| {
                    let mut next = note.clone();
                    next.bead = Some("new".into());
                    asks::raise(&store, next, now + 1).unwrap();
                    Some(true)
                }
            )
            .unwrap(),
            0
        );
        assert_eq!(
            asks::open(store.list(), vec![], "a")[0].bead.as_deref(),
            Some("new")
        );
    }
}
