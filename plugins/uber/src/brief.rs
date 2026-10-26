//! `vega uber brief`: the Slayer's brief (docs/design/uber-uac.md §3 Brief,
//! §5; vega-lhc.209.8). Decisions are the Uber's own open asks; FYI and
//! Blocked come from each live UAC's reply to `uber.brief`, sent on one
//! `vega post ask --within 2m` thread, so one pointer brings every reply.
//! vegad on this host stores the replies; `--show` renders the last brief
//! (`vega_uber_msg::brief`) in at most 25 lines.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use clap::Args;
use vega_herdr::Failure;
use vega_registry::View;
use vega_uber_msg::brief::{self, Request, Target};
use vega_uber_msg::{Brief, Kind};

use crate::{here, no_broker, now_ms, with_local, with_registry, Here};

/// `vega post ask` connects patiently; give it a bit more.
const ASK_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Args, Debug)]
pub struct BriefArgs {
    /// Render the last brief and its replies; ask nothing
    #[arg(long)]
    pub show: bool,
}

fn uber_dir(here: &Here) -> Result<PathBuf, Failure> {
    here.file
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| Failure::new("no_home", "no state dir", "set HOME or XDG_STATE_HOME"))
}

/// The live UACs in `view`, but `me`: fresh host, pane alive.
pub(crate) fn targets(view: &View, me: &Here) -> Vec<Target> {
    let mut t: Vec<Target> = view
        .uac_entries()
        .into_iter()
        .filter(|(h, v)| {
            let pane = v["pane"].as_str().unwrap_or_default();
            let mine = *h == vega_registry::token(&me.host) && pane == me.pane;
            view.fresh(h) && matches!(view.status(h, pane), Some(Some(_))) && !mine
        })
        .filter_map(|(_, v)| {
            Some(Target {
                // As the UAC's mail carries it (`from_host`), so its reply is stored.
                host: vega_herdr::state::host_token(v["host"].as_str()?),
                pane: v["pane"].as_str()?.to_string(),
                project: v["project"].as_str()?.to_string(),
            })
        })
        .collect();
    t.sort_by(|a, b| a.project.cmp(&b.project).then(a.host.cmp(&b.host)));
    t.dedup();
    t
}

/// Sends `uber.brief` to the request's targets on its thread.
fn fan_out(r: &Request, body: &str) -> Result<(), Failure> {
    let mut cmd = Command::new(crate::ask::post_bin());
    cmd.args([
        "ask",
        "--kind",
        Kind::Brief.as_str(),
        "--within",
        brief::WITHIN,
        "--thread",
        &r.thread,
    ]);
    for t in &r.targets {
        cmd.args(["--to", &format!("{}@{}", t.pane, t.host)]);
    }
    cmd.arg(body);
    let failed = |m: String| Failure::new("ask_failed", m, "retry: vega uber brief");
    match vega_herdr::output_within(&mut cmd, ASK_TIMEOUT) {
        Ok(Some(o)) if o.status.success() => Ok(()),
        Ok(Some(o)) => {
            let text = String::from_utf8_lossy(&o.stdout).to_string()
                + &String::from_utf8_lossy(&o.stderr);
            Err(failed(format!(
                "vega post ask failed: {}",
                text.split_whitespace().collect::<Vec<_>>().join(" ")
            )))
        }
        Ok(None) => Err(failed("vega post ask timed out".into())),
        Err(e) => Err(failed(e.to_string())),
    }
}

/// Prints the brief over the last request in at most `max` lines.
fn print(dir: &Path, now: i64, max: usize) {
    let last = brief::last(dir);
    let replies = last
        .as_ref()
        .map(|r| brief::replies(dir, r))
        .unwrap_or_default();
    let lines = brief::render(
        &brief::open_decisions(dir),
        last.as_ref().map(|r| (r, replies.as_slice())),
        now,
        max,
    );
    for l in lines {
        println!("{l}");
    }
}

/// Records the request, then sends it: a reply that comes back at once
/// finds the record. A failed send removes it again.
pub(crate) fn ask_brief(
    dir: &Path,
    r: &Request,
    send: impl FnOnce(&Request, &str) -> Result<(), Failure>,
) -> Result<(), Failure> {
    brief::save_request(dir, r)
        .map_err(|e| Failure::new("state_io", e.to_string(), "check the state dir"))?;
    if !r.targets.is_empty() {
        let body = serde_json::to_string(&Brief {
            since: r.since.clone(),
        })
        .expect("a Brief serializes");
        if let Err(f) = send(r, &body) {
            brief::remove(dir, &r.thread);
            return Err(f);
        }
    }
    brief::prune(dir, brief::KEEP);
    Ok(())
}

pub(crate) fn brief(a: BriefArgs) -> Result<(), Failure> {
    let here = here()?;
    let dir = uber_dir(&here)?;
    let now = (now_ms() / 1000) as i64;
    if a.show {
        print(&dir, now, brief::MAX_LINES);
        return Ok(());
    }
    let view = with_registry(async |client, kv| {
        let mut v = vega_registry::read_kinds(client, kv, Some(&["beat", "agent", "uber", "uac"]))
            .await
            .map_err(no_broker)?;
        with_local(&mut v, &here);
        Ok(v)
    })?;
    match view.live_uber() {
        Some(w) if here.mine(&w) => {}
        _ => {
            return Err(Failure::new(
                "not_uber",
                "only the live Uber-UAC asks for a brief",
                "vega uber status",
            ))
        }
    }
    let targets = targets(&view, &here);
    let since = brief::last(&dir)
        .map(|r| r.at)
        .unwrap_or(now - brief::FIRST_SINCE);
    let since = brief::rfc3339(since);
    let request = Request {
        thread: brief::new_thread(),
        asker: here.pane.clone(),
        since,
        at: now,
        targets,
    };
    ask_brief(&dir, &request, fan_out)?;
    // The help line counts toward the phone's 25.
    let help = !request.targets.is_empty();
    print(&dir, now, brief::MAX_LINES - usize::from(help));
    if help {
        println!(
            "help: {} UACs asked; their replies come as one pointer within {}, then run vega uber brief --show",
            request.targets.len(),
            brief::WITHIN
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn uac(view: &mut View, host: &str, pane: &str, project: &str, alive: bool) {
        view.entries.insert(
            vega_registry::beat_key(host),
            json!({"at": now_ms() / 1000}),
        );
        view.entries.insert(
            vega_registry::uac_key(host, project),
            json!({"schema": 2, "pane": pane, "host": host, "project": project, "at": 0}),
        );
        if alive {
            view.entries.insert(
                vega_registry::agent_key(host, pane),
                json!({"pane": pane, "status": "idle"}),
            );
        }
    }

    #[test]
    fn the_brief_asks_every_live_uac_but_the_uber_itself() {
        let mut view = View::default();
        uac(
            &mut view,
            "proteus",
            "w3V:p1",
            "github.com/durandom/vega",
            true,
        );
        uac(
            &mut view,
            // The registry keys a dotted host as titan_local; mail says titan_local.
            "titan.local",
            "w2:p1",
            "github.com/b4arena/arena",
            true,
        );
        uac(
            &mut view,
            "titan",
            "w5:p1",
            "github.com/durandom/gone",
            false,
        );
        uac(
            &mut view,
            "ganymede",
            "w1:p1",
            "github.com/durandom/self",
            true,
        );
        // A stale host's UAC is not live.
        uac(&mut view, "io", "w1:p2", "github.com/durandom/stale", true);
        view.entries
            .insert(vega_registry::beat_key("io"), json!({"at": 0}));
        let me = Here {
            host: "ganymede".into(),
            pane: "w1:p1".into(),
            panes: vec![],
            file: PathBuf::from("/x/claim.json"),
        };
        let t = targets(&view, &me);
        let got: Vec<_> = t.iter().map(|t| format!("{}@{}", t.pane, t.host)).collect();
        assert_eq!(got, ["w2:p1@titan_local", "w3V:p1@proteus"]);
    }

    fn request() -> Request {
        Request {
            thread: brief::new_thread(),
            asker: "w1:p1".into(),
            since: brief::rfc3339(0),
            at: 1,
            targets: vec![Target {
                host: "titan".into(),
                pane: "w2:p1".into(),
                project: "arena".into(),
            }],
        }
    }

    #[test]
    fn a_reply_that_comes_back_during_the_send_is_stored() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        let r = request();
        ask_brief(d, &r, |r, body| {
            assert!(vega_uber_msg::validate(Kind::Brief, body.as_bytes()).is_ok());
            // The UAC answers before `vega post ask` has returned.
            assert!(
                brief::store_reply(d, &r.thread, "w1:p1", "titan", "w2:p1", b"FYI: x").unwrap()
            );
            Ok(())
        })
        .unwrap();
        let replies = brief::replies(d, &r);
        assert_eq!(replies[0].1.as_deref(), Some("FYI: x"));
    }

    #[test]
    fn a_failed_send_leaves_no_brief() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        let r = request();
        let f = ask_brief(d, &r, |_, _| Err(Failure::new("ask_failed", "x", ""))).unwrap_err();
        assert_eq!(f.code, "ask_failed");
        assert_eq!(brief::last(d), None);
    }
}
