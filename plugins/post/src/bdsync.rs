//! vegad's beads sync thread (docs/design/beads-sync.md): runs the sync
//! cycle of crates/vega-bd-sync every few seconds over the projects `vega bd
//! init` provisioned on this host. bd pull and push block for seconds, so
//! they run here, never on the relay loop. A project that stops (conflict,
//! rejected push) raises an attention: a herdr notification for the human,
//! and mail to the project's UAC when one is registered on this host. The
//! notification's note counts on that project's UAC badge until a later
//! sync succeeds, then it resolves (vega_herdr::notes).

use std::thread;
use std::time::Duration;

use tokio::sync::mpsc::UnboundedSender;
use vega_bd_sync::store;
use vega_bd_sync::sync::{self, Bd};
use vega_herdr::notes::{Note, Store};

/// How often the thread looks; each project pulls every `sync::PULL_EVERY`
/// and pushes within one tick of a local write.
const EVERY: Duration = Duration::from_secs(5);

/// The `Vega-From-Agent` of beads sync mail: a vega tool, no reply path.
pub const AGENT: &str = "tool vega-bd";

pub enum Event {
    /// (pane, text) for the relay loop to publish.
    Mail(String, String),
    Log(String),
}

pub fn spawn(tx: UnboundedSender<Event>) {
    thread::spawn(move || {
        // Claims use the registry host; bd actors retain their local host contract.
        let host = vega_herdr::state::post_host();
        let mut ops = Bd {
            actor: format!("{}/vegad", vega_herdr::local_host()),
        };
        loop {
            for e in sync::cycle(vega_bd_sync::unix_now(), &mut ops) {
                let out = match e {
                    sync::Event::Log(l) => vec![Event::Log(l)],
                    sync::Event::Stopped {
                        key,
                        common,
                        database,
                        reason,
                    } => stopped(&host, &key, &common, &database, &reason),
                };
                for ev in out {
                    if tx.send(ev).is_err() {
                        return;
                    }
                }
            }
            let n = resolve_synced(vega_bd_sync::unix_now());
            if n > 0
                && tx
                    .send(Event::Log(format!("bd sync: resolved {n} stop note(s)")))
                    .is_err()
            {
                return;
            }
            thread::sleep(EVERY);
        }
    });
}

/// The `cause` of a stop note for `database`.
fn cause(database: &str) -> String {
    format!("bd-sync:{database}")
}

/// Resolves the stop notes of projects that synced after the note: sync is
/// back to ok. How many.
fn resolve_synced(now: i64) -> usize {
    let Some(notes) = Store::local() else {
        return 0;
    };
    let synced: Vec<(String, String, i64)> = store::projects()
        .into_iter()
        .map(|p| {
            let s = store::state(&p.key, &p.common);
            let at = if s.is_stopped(&p) { 0 } else { s.synced };
            (p.key, cause(&p.database), at)
        })
        .collect();
    notes.resolve(now, |n| cleared(n, &synced))
}

/// A stop note whose project (cause, last full sync in Unix s) synced
/// after it was written.
fn cleared(n: &Note, synced: &[(String, String, i64)]) -> bool {
    // Keep migrated causes unchanged. If a legacy cause covers multiple clones,
    // every matching clone must recover before that shared attention clears.
    let matching: Vec<_> = synced
        .iter()
        .filter(|(project, cause, _)| {
            n.cause.as_deref() == Some(cause) && n.project.as_ref().is_none_or(|p| p == project)
        })
        .collect();
    !matching.is_empty() && matching.iter().all(|(_, _, at)| *at > n.at())
}

/// The attention for a stopped project: always a notification; mail to the
/// project's UAC if it is registered on this host.
fn stopped(
    host: &str,
    key: &str,
    common: &std::path::Path,
    database: &str,
    reason: &str,
) -> Vec<Event> {
    let title = format!("vegad: beads sync of {database} stopped");
    vega_herdr::notes::notify_project(
        "bd-sync",
        None,
        Some(key),
        Some(&cause(database)),
        &title,
        reason,
    );
    let mut out = Vec::new();
    match vega_herdr::state::project_uac_of(key) {
        Some((pane, h)) if h == host => out.push(Event::Mail(
            pane,
            mail(&common.display().to_string(), database, reason),
        )),
        _ => out.push(Event::Log(format!(
            "bd sync: {key} has no UAC on this host; notification only"
        ))),
    }
    out
}

fn mail(key: &str, database: &str, reason: &str) -> String {
    format!(
        "Beads sync of {database} ({key}) stopped: {reason}\n\
         Nothing pushes or pulls for this project until it is resolved and resumed.\n\
         Resolve in .beads/dolt/{database}: dolt pull origin main; dolt conflicts resolve --ours|--theirs issues; dolt commit -m <msg>.\n\
         Then: vega bd resume --project {key}   (vega bd status shows the state)"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mail_names_the_fix_and_resume() {
        let m = mail(
            "src/x/.git",
            "X",
            "pull conflict: merge conflicts in issues",
        );
        assert!(m.contains("vega bd resume --project src/x/.git"));
        assert!(m.contains(".beads/dolt/X"));
    }

    #[test]
    fn a_stop_note_clears_once_its_project_synced_after_it() {
        let stop = Note {
            id: 1_800_000_000_500,
            cause: Some(cause("X")),
            ..Default::default()
        };
        let at = stop.at();
        assert!(
            !cleared(&stop, &[("project".into(), cause("X"), at)]),
            "the sync before the stop"
        );
        assert!(!cleared(&stop, &[("project".into(), cause("Y"), at + 9)]));
        assert!(!cleared(&stop, &[]), "still stopped");
        assert!(cleared(&stop, &[("project".into(), cause("X"), at + 1)]));
    }
    #[test]
    fn a_healthy_clone_cannot_clear_another_clones_stop() {
        let n = Note {
            id: 1_800_000_000_500,
            project: Some("project".into()),
            cause: Some(cause("DB")),
            ..Default::default()
        };
        let statuses = vec![
            ("project".into(), cause("DB"), n.at() + 1),
            ("project".into(), cause("DB"), 0),
        ];
        assert!(!cleared(&n, &statuses));
        assert!(!cleared(
            &n,
            &[("other-project".into(), cause("DB"), n.at() + 1)]
        ));
    }
}
