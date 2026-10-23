//! vegad's git sync thread (docs/design/git-sync.md): fetches the projects
//! vegad knows (the bd registry and the UAC claims on this host) every few
//! minutes, fast-forwards their clean main checkout, and records how far
//! each worktree is behind. git fetch blocks on the network, so it runs
//! here, never on the relay loop. It only logs; `vega bd status` shows the
//! state and the bd shim tells the agents in the checkouts.

use std::thread;
use std::time::Duration;

use vega_bd_sync::git::{self, Git};

/// How often the thread looks; each project fetches every
/// `git::FETCH_EVERY` and fast-forwards at most every `git::FF_EVERY`.
const EVERY: Duration = Duration::from_secs(30);

pub fn spawn() {
    thread::spawn(|| loop {
        let claimed = git::claimed();
        let repos = git::repos(claimed);
        for line in git::cycle(vega_bd_sync::unix_now(), &repos, &mut Git) {
            crate::log(&line);
        }
        thread::sleep(EVERY);
    });
}
