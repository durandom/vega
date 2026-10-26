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
                host: v["host"].as_str()?.to_lowercase(),
                pane: v["pane"].as_str()?.to_string(),
                project: v["project"].as_str()?.to_string(),
            })
        })
        .collect();
    t.sort_by(|a, b| a.project.cmp(&b.project).then(a.host.cmp(&b.host)));
    t.dedup();
    t
}

/// The thread id from `vega post ask`'s first line `ask: <thread> to ...`.
pub(crate) fn thread_of(stdout: &str) -> Option<String> {
    let rest = stdout.lines().next()?.strip_prefix("ask: ")?;
    let t = rest.split_whitespace().next()?;
    t.chars()
        .all(|c| c.is_ascii_alphanumeric())
        .then(|| t.to_string())
}

/// Sends `uber.brief` to `targets` on one gather thread; its id.
fn fan_out(targets: &[Target], body: &str) -> Result<String, Failure> {
    let mut cmd = Command::new(crate::ask::post_bin());
    cmd.args([
        "ask",
        "--kind",
        Kind::Brief.as_str(),
        "--within",
        brief::WITHIN,
    ]);
    for t in targets {
        cmd.args(["--to", &format!("{}@{}", t.pane, t.host)]);
    }
    cmd.arg(body);
    let failed = |m: String| Failure::new("ask_failed", m, "retry: vega uber brief");
    match vega_herdr::output_within(&mut cmd, ASK_TIMEOUT) {
        Ok(Some(o)) if o.status.success() => thread_of(&String::from_utf8_lossy(&o.stdout))
            .ok_or_else(|| failed("vega post ask printed no thread".into())),
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

/// Prints the brief over the last request.
fn print(dir: &Path, now: i64) {
    let last = brief::last(dir);
    let replies = last
        .as_ref()
        .map(|r| brief::replies(dir, r))
        .unwrap_or_default();
    let lines = brief::render(
        &brief::open_decisions(dir),
        last.as_ref().map(|r| (r, replies.as_slice())),
        now,
    );
    for l in lines {
        println!("{l}");
    }
}

pub(crate) fn brief(a: BriefArgs) -> Result<(), Failure> {
    let here = here()?;
    let dir = uber_dir(&here)?;
    let now = (now_ms() / 1000) as i64;
    if a.show {
        print(&dir, now);
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
    let thread = if targets.is_empty() {
        // Nothing to ask; the record still marks this brief's time.
        format!("none{now}")
    } else {
        let body = serde_json::to_string(&Brief {
            since: since.clone(),
        })
        .expect("a Brief serializes");
        fan_out(&targets, &body)?
    };
    let request = Request {
        thread,
        asker: here.pane.clone(),
        since,
        at: now,
        targets,
    };
    brief::save_request(&dir, &request)
        .map_err(|e| Failure::new("state_io", e.to_string(), "check the state dir"))?;
    brief::prune(&dir, brief::KEEP);
    print(&dir, now);
    if !request.targets.is_empty() {
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
            "titan",
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
        assert_eq!(got, ["w2:p1@titan", "w3V:p1@proteus"]);
    }

    #[test]
    fn the_thread_comes_from_the_first_line_of_post_ask() {
        let out = "ask: phFIPgeIpAQf05sPo6ugsU to 3 agents, answers of 3 held until all answered or 2m\nsent[3]{host,pane,result}:";
        assert_eq!(thread_of(out).as_deref(), Some("phFIPgeIpAQf05sPo6ugsU"));
        assert_eq!(thread_of("ask: ../x to 1"), None);
        assert_eq!(thread_of("sent: w1"), None);
    }
}
