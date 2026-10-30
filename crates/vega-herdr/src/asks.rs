//! Origin-owned blocking asks, stored as notes. No delivery policy lives here.
use crate::notes::{Note, Store};
use std::fs::File;
use std::io;
use std::os::fd::AsRawFd;

pub fn rank(level: &str) -> Option<u8> {
    match level {
        "info" => Some(0),
        "action" => Some(1),
        "urgent" => Some(2),
        _ => None,
    }
}

/// Serialize read/modify/write across CLI calls and vegad on this host.
struct Lock(File);
impl Drop for Lock {
    fn drop(&mut self) {
        unsafe { libc::flock(self.0.as_raw_fd(), libc::LOCK_UN) };
    }
}
fn lock(store: &Store) -> io::Result<Lock> {
    std::fs::create_dir_all(&store.dir)?;
    let file = File::options()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(store.dir.join("ask.lock"))?;
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(Lock(file))
}

/// Re-raise preserves the original id/clock and ack. Lower levels change nothing.
/// Returns (note, created, level increased).
pub fn raise(store: &Store, mut ask: Note, now_ms: u64) -> io::Result<(Note, bool, bool)> {
    let _lock = lock(store)?;
    if ask.kind != "ask"
        || ask.key.as_deref().is_none_or(str::is_empty)
        || ask.level.as_deref().and_then(rank).is_none()
    {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "invalid ask"));
    }
    if let Some(old) = store
        .list()
        .into_iter()
        .find(|n| n.kind == "ask" && n.host == ask.host && n.key == ask.key && n.resolved.is_none())
    {
        let new_level = ask.level.as_deref().and_then(rank).unwrap();
        let old_level = old.level.as_deref().and_then(rank).unwrap_or(0);
        if new_level < old_level {
            return Ok((old, false, false));
        }
        ask.id = old.id;
        ask.acked = old.acked;
        store.save(&ask)?;
        return Ok((ask, false, new_level > old_level));
    }
    Ok((store.add(ask, now_ms)?, true, false))
}

/// Only the origin's local store is writable. Resolved asks are idempotent no-ops.
pub fn change(
    store: &Store,
    host: &str,
    now: i64,
    ack: bool,
    matches: impl Fn(&Note) -> bool,
) -> io::Result<usize> {
    let _lock = lock(store)?;
    let mut count = 0;
    for mut n in store
        .list()
        .into_iter()
        .filter(|n| n.kind == "ask" && n.host == host && n.resolved.is_none() && matches(n))
    {
        if ack {
            if n.acked.is_some() {
                continue;
            }
            n.acked = Some(now);
        } else {
            n.resolved = Some(now);
        }
        store.save(&n)?;
        count += 1;
    }
    Ok(count)
}

/// Merge registry notes with local truth, ignoring read/clear watermarks.
pub fn open(local: Vec<Note>, remote: Vec<Note>, host: &str) -> Vec<Note> {
    let mut notes: Vec<_> = local
        .into_iter()
        .chain(remote.into_iter().filter(|n| n.host != host))
        .filter(|n| n.kind == "ask" && n.resolved.is_none())
        .collect();
    notes.sort_by(|a, b| (&a.host, a.id).cmp(&(&b.host, b.id)));
    notes.dedup_by(|a, b| a.host == b.host && a.id == b.id);
    notes
}

/// Unknown pane/bead status never resolves an ask.
pub fn cleared(ask: &Note, pane_alive: Option<bool>, bead_closed: Option<bool>) -> bool {
    ask.kind == "ask"
        && ask.resolved.is_none()
        && (pane_alive == Some(false) || bead_closed == Some(true))
}

#[cfg(test)]
mod tests {
    use super::*;
    const T: u64 = 1_800_000_000_000;
    fn ask(host: &str, level: &str) -> Note {
        Note {
            host: host.into(),
            kind: "ask".into(),
            level: Some(level.into()),
            key: Some("proj/w1:p1".into()),
            pane: Some("w1:p1".into()),
            cause: Some("ask:proj/w1:p1".into()),
            title: "proj: go -> w1:p1".into(),
            ..Default::default()
        }
    }
    #[test]
    fn raise_reraise_level_up_ack_resolve_and_two_hosts() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        let sa = Store {
            dir: a.path().into(),
        };
        let sb = Store {
            dir: b.path().into(),
        };
        let (first, created, _) = raise(&sa, ask("a", "action"), T).unwrap();
        assert!(created);
        let mut update = ask("a", "action");
        update.title = "new line".into();
        let (again, created, up) = raise(&sa, update, T + 50).unwrap();
        assert!(!created && !up);
        assert_eq!(again.id, first.id);
        assert_eq!(again.title, "new line");
        let (lower, _, _) = raise(&sa, ask("a", "info"), T + 60).unwrap();
        assert_eq!(lower, again);
        let (higher, _, up) = raise(&sa, ask("a", "urgent"), T + 70).unwrap();
        assert!(up);
        assert_eq!(higher.id, first.id);
        raise(&sb, ask("b", "action"), T).unwrap();
        assert_eq!(
            open(sa.list(), [sa.list(), sb.list()].concat(), "a").len(),
            2
        );
        assert_eq!(change(&sa, "a", 10, true, |n| n.id == first.id).unwrap(), 1);
        assert_eq!(change(&sa, "a", 11, true, |_| true).unwrap(), 0);
        let seen = sa.mark(&crate::notes::newest(&sa.list()), true).unwrap();
        let acked = sa.list()[0].clone();
        assert!(!seen.is_unread(&acked));
        assert!(!seen.is_cleared(&acked), "an open ask stays on the board");
        let (reraised, _, _) = raise(&sa, ask("a", "urgent"), T + 80).unwrap();
        assert_eq!(reraised.acked, Some(10));
        assert_eq!(reraised.id, first.id);
        assert_eq!(open(sa.list(), sb.list(), "a")[0].acked, Some(10));
        assert_eq!(change(&sa, "a", 12, false, |_| true).unwrap(), 1);
        assert_eq!(change(&sa, "a", 13, false, |_| true).unwrap(), 0);
        assert_eq!(open(sa.list(), sb.list(), "a").len(), 1);
        let (fresh, created, _) = raise(&sa, ask("a", "info"), T + 100).unwrap();
        assert!(created);
        assert!(fresh.id > first.id);
        sa.prune((T / 1000) as i64 + crate::notes::KEEP_SECS + 1);
        assert_eq!(sa.list(), vec![fresh]);
    }
    #[test]
    fn lifecycle_requires_evidence() {
        let n = ask("a", "action");
        assert!(!cleared(&n, None, None));
        assert!(!cleared(&n, Some(true), Some(false)));
        assert!(cleared(&n, Some(false), None));
        assert!(cleared(&n, None, Some(true)));
    }
    #[test]
    fn concurrent_raises_dedupe() {
        let d = tempfile::tempdir().unwrap();
        std::thread::scope(|scope| {
            for _ in 0..8 {
                let dir = d.path();
                scope.spawn(move || {
                    raise(&Store { dir: dir.into() }, ask("a", "action"), T).unwrap();
                });
            }
        });
        assert_eq!(
            Store {
                dir: d.path().into()
            }
            .list()
            .len(),
            1
        );
    }
}
