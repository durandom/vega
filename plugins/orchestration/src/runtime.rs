//! Runtime declarations are associated by pane@host, bead and thread only.
use super::{age, all_beads, asks, Row};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};
use vega_herdr::{
    cell,
    waits::{Store, Wait},
};
use vega_registry::View;

pub fn rows(
    out: &mut Vec<Row>,
    view: Option<&View>,
    panes: &BTreeSet<(String, String)>,
    cwd: &Path,
    host: &str,
    now: i64,
) {
    let mut merged = BTreeMap::new();
    if let Some(view) = view {
        match view.explicit_waits() {
            Ok(waits) => {
                println!(
                    "explicit_waits_registry: snapshot; stale coordinator records remain unknown"
                );
                for (w, fresh) in waits {
                    merged.insert(
                        w.id.clone(),
                        (
                            w,
                            if fresh {
                                "registry snapshot"
                            } else {
                                "unknown (stale coordinator host)"
                            },
                        ),
                    );
                }
            }
            Err(_) => println!("explicit_waits_registry: unknown (invalid projection)"),
        }
        let hosts: BTreeSet<_> = panes
            .iter()
            .map(|(h, _)| h.clone())
            .chain(std::iter::once(host.into()))
            .collect();
        println!("wait_sources[{}]{{host,status}}:", hosts.len());
        for h in hosts {
            let status = match view.wait_source(&h) {
                Some(s) if s["waits_available"] == true => "snapshot",
                _ => "unknown",
            };
            println!("  {},{}", cell(&h), status);
        }
    } else {
        println!("explicit_waits_registry: unknown (registry unavailable)");
    }
    let local = vega_herdr::waits::path()
        .ok_or(())
        .and_then(|p| Store::read(&p).map_err(|_| ()));
    match local {
        Ok(waits) => {
            println!("explicit_waits_local: snapshot");
            // Resolved local records fence the publisher's lagging active projection.
            for w in waits {
                merged.insert(w.id.clone(), (w, "local transactional record"));
            }
        }
        Err(_) => println!("explicit_waits_local: unknown (state absent or unreadable)"),
    }
    let beads = all_beads(cwd).ok().map(|xs| {
        xs.into_iter()
            .filter_map(|b| b["id"].as_str().map(String::from))
            .collect::<BTreeSet<_>>()
    });
    let (rows, unscoped) = record_rows(merged.into_values(), panes, beads.as_ref(), now);
    out.extend(rows);
    if unscoped > 0 {
        println!("explicit_waits_scope: unknown for {unscoped} records without current pane or bead association");
    }
}

fn record_rows<'a>(
    records: impl IntoIterator<Item = (Wait, &'a str)>,
    panes: &BTreeSet<(String, String)>,
    beads: Option<&BTreeSet<String>>,
    now: i64,
) -> (Vec<Row>, usize) {
    let mut rows = Vec::new();
    let mut unscoped = 0;
    for (w, freshness) in records {
        if w.resolved.is_some() {
            continue;
        }
        let ep = &w.waiter.endpoint;
        let co = &w.coordinator;
        let member = panes.contains(&(ep.host.clone(), ep.pane.clone()))
            || panes.contains(&(co.host.clone(), co.pane.clone()));
        let bead = w
            .bead
            .as_ref()
            .is_some_and(|b| beads.is_some_and(|ids| ids.contains(b)));
        if !member && !bead {
            unscoped += 1;
            continue;
        }
        rows.push(Row {
            id: w.id.clone(),
            bead: w.bead.unwrap_or_else(|| "unknown".into()),
            waiter: format!("{}@{}", ep.pane, ep.host),
            owner: w.on,
            reason: w.reason,
            age: age(w.created, now),
            source: w.thread.map_or("explicit-wait".into(), |t| {
                format!("explicit-wait; thread {t}")
            }),
            freshness: format!("{freshness}; waiter session recorded"),
            next: format!(
                "vega orchestration clear {} --evidence \"<inspected resolution>\"",
                w.id
            ),
        });
    }
    (rows, unscoped)
}

pub fn questions(
    rows: &mut Vec<Row>,
    view: Option<&View>,
    panes: &BTreeSet<(String, String)>,
    host: &str,
    now: i64,
) {
    match vega_herdr::gather::open() {
        Ok(questions) => {
            for (thread, ask) in questions {
                let files = vec![(
                    format!("local_{thread}"),
                    serde_json::from_str(&ask.to_json()).unwrap_or(Value::Null),
                    0,
                )];
                asks(rows, &files, panes, host, now);
            }
            println!("post_asks_local: snapshot (includes unanswered expired threads)");
        }
        Err(_) => println!("post_asks_local: unknown (source absent or unreadable)"),
    }
    if let Some(view) = view {
        match view.open_asks() {
            Ok(questions) => {
                let mut unknown = 0;
                for (h, thread, ask, fresh) in questions {
                    if h == host {
                        continue;
                    }
                    if !fresh || view.agent(&h, &ask.asker).is_none() {
                        unknown += 1;
                        continue;
                    }
                    let mut added = Vec::new();
                    asks(
                        &mut added,
                        &[(
                            format!("local_{thread}"),
                            serde_json::from_str(&ask.to_json()).unwrap_or(Value::Null),
                            0,
                        )],
                        panes,
                        &h,
                        now,
                    );
                    for r in &mut added {
                        r.source = "post-ask (registry)".into();
                        r.freshness = "fresh publisher; unresolved thread".into();
                    }
                    rows.extend(added);
                }
                println!("post_asks_registry: snapshot; {unknown} unresolved sources unknown; missing publishers unknown");
            }
            Err(_) => println!("post_asks_registry: unknown (invalid projection)"),
        }
    } else {
        println!("post_asks_registry: unknown (registry unavailable)");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vega_herdr::waits::{Actor, Endpoint};
    fn wait() -> Wait {
        Wait {
            version: 1,
            id: format!("owner:{}", "a".repeat(32)),
            waiter: Actor {
                endpoint: Endpoint {
                    host: "worker".into(),
                    pane: "w1:p1".into(),
                    name: "child".into(),
                },
                session: "s".into(),
            },
            coordinator: Endpoint {
                host: "owner".into(),
                pane: "w2:p1".into(),
                name: "parent".into(),
            },
            on: "Slayer".into(),
            reason: "decision".into(),
            bead: Some("b-1".into()),
            thread: Some("thread".into()),
            created: 100,
            resolved: None,
            evidence: None,
        }
    }
    #[test]
    fn scope_uses_declared_bead_or_current_coordinator_and_never_treats_resolution_as_approval() {
        let w = wait();
        let (rows, unknown) = record_rows(
            [(w.clone(), "unknown (stale)")],
            &BTreeSet::new(),
            Some(&BTreeSet::from(["b-1".into()])),
            150,
        );
        assert_eq!(rows.len(), 1);
        assert_eq!(unknown, 0);
        assert_eq!(rows[0].age, "50s");
        assert!(rows[0].freshness.contains("unknown"));
        assert!(rows[0].source.contains("thread"));
        let (_, unknown) = record_rows([(w.clone(), "local")], &BTreeSet::new(), None, 150);
        assert_eq!(unknown, 1);
        let panes = BTreeSet::from([("owner".into(), "w2:p1".into())]);
        let (rows, _) = record_rows([(w.clone(), "local")], &panes, None, 150);
        assert_eq!(rows.len(), 1);
        let mut cleared = w;
        cleared.resolved = Some(120);
        cleared.evidence = Some("inspected".into());
        let (rows, _) = record_rows([(cleared, "local")], &panes, None, 150);
        assert!(rows.is_empty());
    }
}
