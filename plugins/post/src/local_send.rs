//! Inbox-only hosts may talk to their own panes without broker rights.
//! Local v2 UAC claims and herdr are the only discovery sources here.
use crate::{h, reach, st, Failure, HeaderMap, Outcome, To};
use serde_json::Value;
use vega_herdr::{herdr_local, Me};

const LOCAL_HINT: &str = "run `vega mux` to list local agents; send locally with vega post send <pane> \"<text>\"; for another host copy its explicit pane and --host from the reply command (quarantine)";

pub fn resolve(to: To<'_>, host: &str) -> Result<(String, String), Failure> {
    match to {
        To::Pane(pane, target_host) => Ok((
            pane.into(),
            target_host
                .map(crate::subject_token)
                .unwrap_or_else(|| host.into()),
        )),
        To::Uac(query) => {
            let here = if query == "." {
                crate::my_project()?
            } else {
                None
            };
            let mut view = vega_registry::View::default();
            for (project, _, _) in st::project_uac_records() {
                let Some((_, owner)) = st::project_uac_of(&project) else {
                    continue;
                };
                if crate::subject_token(&owner) != host {
                    continue;
                }
                let record: Option<Value> = st::project_file("uac", &project)
                    .and_then(|p| std::fs::read(p).ok())
                    .and_then(|bytes| serde_json::from_slice(&bytes).ok());
                if let Some(record) = record {
                    if let Ok(entry) = vega_registry::uac_entry(host, &record) {
                        view.entries
                            .insert(vega_registry::uac_key(host, &project), entry);
                    }
                }
            }
            let keys = reach::projects(&view, query, here.as_deref());
            let agents = herdr_local(&["agent", "list"])?;
            let mut found: Vec<String> = keys
                .iter()
                .filter_map(|key| {
                    let pane = view.entries.get(&vega_registry::uac_key(host, key))?["pane"]
                        .as_str()?
                        .to_owned();
                    agents["result"]["agents"]
                        .as_array()?
                        .iter()
                        .any(|a| a["pane_id"] == pane)
                        .then_some(pane)
                })
                .collect();
            found.sort();
            found.dedup();
            match found.as_slice() {
                [pane] => Ok((pane.clone(), host.into())),
                [] => Err(Failure::new("no_uac", format!("no open local UAC for {query}; this inbox-only host cannot enumerate the remote registry"), LOCAL_HINT)),
                _ => Err(Failure::new("ambiguous_uac", format!("{query} matches several local UAC panes: {}", found.join(", ")), "pass the full project ID to --uac, or run `vega mux` and send to an explicit local pane")),
            }
        }
        To::Uber => Err(Failure::new(
            "inbox_only",
            "this inbox-only host cannot read the registry to find the Uber-UAC",
            LOCAL_HINT,
        )),
    }
}

pub fn deliver(
    target: &str,
    me: &Me,
    headers: &HeaderMap,
    text: &str,
    done: Option<Outcome>,
) -> Result<(), Failure> {
    // Canonicalize names as mux does; this also rejects closed panes before paste.
    let agent = herdr_local(&["agent", "get", target])?;
    let pane = agent["result"]["agent"]["pane_id"]
        .as_str()
        .ok_or_else(|| {
            Failure::new(
                "pane_gone",
                format!("{target} has no local agent pane"),
                LOCAL_HINT,
            )
        })?;
    if pane == me.pane {
        return Err(Failure::new(
            "self_send",
            format!("{pane} is your own pane"),
            "run `vega mux` to list other agents",
        ));
    }
    let hv = |key| headers.get(key).map(|v| v.as_str()).unwrap_or("");
    let host = crate::host();
    let stale = || crate::stale_brief(&st::children(), pane, &me.pane, &host, hv(h::SENT_MS));
    let stale_before = done.and_then(|_| stale());
    let stale_marker = |at| {
        format!("stale: report from {} sent at {} ms predates your newer brief at {at} ms; this reports on earlier work; your newer brief stands", me.pane, hv(h::SENT_MS))
    };
    let report = match (done, stale_before) {
        (_, Some(at)) => format!("{}\n{text}", stale_marker(at)),
        (Some(_), None) => format!(
            "Report sent at {} ms; a newer brief supersedes this report.\n{text}",
            hv(h::SENT_MS)
        ),
        _ => text.into(),
    };
    let reply = if me.pane.is_empty() {
        "reply: none (sent from outside an agent pane)".into()
    } else {
        format!(
            "reply only if it adds something: vega post send {} --re {}/{}@{} \"<reply>\"",
            me.pane,
            hv(h::THREAD),
            hv(h::HOP),
            hv(h::SENT_MS)
        )
    };
    let sent = vega_herdr::local_time(None)
        .map(|t| format!(", {t}"))
        .unwrap_or_default();
    let footer = format!(
        "\n\n---\n[vega-mux] from {} ({} in {}){sent} · {reply}",
        me.pane, me.agent, me.cwd
    );
    let body = format!("{report}{footer}");
    herdr_local(&["agent", "prompt", pane, &body])?;
    vega_herdr::prompt::confirm_taken(pane, &body)?;
    // History is queued locally even on inbox-only hosts, without a broker call.
    vega_herdr::history::emit(
        "mail.local",
        serde_json::json!({"pane":pane,"from_pane":me.pane,"body":text,"thread":hv(h::THREAD),"hop":hv(h::HOP)}),
    );
    let stale_after = done.and_then(|_| stale());
    // A brief can arrive while herdr is submitting the report. Its first
    // prompt already carries the sent time and supersession rule; follow it
    // with a concrete correction before acknowledging delivery to the sender.
    if let Some(at) = stale_after.filter(|_| stale_before.is_none()) {
        let correction = format!("{}{footer}", stale_marker(at));
        herdr_local(&["agent", "prompt", pane, &correction])?;
        vega_herdr::prompt::confirm_taken(pane, &correction)?;
    }
    // Preserve local parent/child supervision, including done records, without
    // publishing mail or pretending there is a broker archive entry.
    let children = st::children();
    if let Some(child) = crate::child_of(&children, &me.pane, pane, &crate::host()) {
        if let Some(outcome) = done.filter(|_| stale_after.is_none()) {
            let prefix = format!("done ({}): ", outcome.as_str());
            st::save_done(
                child.machine.as_deref(),
                &me.pane,
                &st::Done {
                    outcome,
                    summary: text.strip_prefix(&prefix).unwrap_or(text).into(),
                },
            );
        }
        st::set_replied(child.machine.as_deref(), &me.pane);
        st::set_silent(child.machine.as_deref(), &me.pane, false);
    } else if children
        .iter()
        .any(|c| c.machine.is_none() && c.pane == pane && c.parent.pane == me.pane)
    {
        st::settle(None, pane);
    }
    if done.is_none() {
        if let Some(child) = crate::child_of(&children, pane, &me.pane, &host) {
            st::save_brief(child.machine.as_deref(), pane);
        }
    }
    if let Some(outcome) = done {
        println!("done: {} to {pane} locally via herdr", outcome.as_str());
        println!("help: your parent decides what comes next; end your turn now");
    } else {
        println!("sent: {pane} locally via herdr");
        println!("thread: {}/{}", hv(h::THREAD), hv(h::HOP));
        println!("help: the reply arrives as a local prompt; do not poll");
    }
    Ok(())
}
