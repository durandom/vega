//! Headless board dump. `vega-board --selftest` prints the grouped board as
//! plain text - no TUI - so CI (and you, before launching herdr) can confirm
//! the bd bridge parses and groups correctly.

use crate::bd;
use crate::decisions::{
    bead_age, is_decision, is_lane, lane_summary, parse_rfc3339, unix_now, Tracker,
};
use crate::model::{status_glyph, status_label, status_rank, Scope};
use anyhow::Result;

pub fn run(scope: Scope) -> Result<()> {
    let beads = bd::load(scope, true)?;
    println!(
        "vega-board selftest - {} beads · scope {}\n",
        beads.len(),
        scope.label()
    );

    let tracker = Tracker::find(&beads);
    match tracker.id() {
        Some(t) => {
            let open: Vec<_> = beads.iter().filter(|b| is_decision(b, t)).collect();
            println!("decisions under {t} ({})", open.len());
            for b in open {
                println!("  P{} {:<14} {}", b.priority, b.id, b.title);
            }
        }
        None => println!("{}", tracker.problem().unwrap_or_default()),
    }
    let now = unix_now();
    println!("lane beads: {}", lane_summary(&beads, now));
    let mut lane: Vec<_> = beads.iter().filter(|b| is_lane(b)).collect();
    lane.sort_by_key(|b| b.created_at.as_deref().and_then(parse_rfc3339));
    for b in lane {
        println!(
            "  P{} {:<14} {} {} {}",
            b.priority,
            b.id,
            b.created_by.as_deref().unwrap_or("?"),
            bead_age(b, now),
            b.title.chars().take(50).collect::<String>()
        );
    }
    println!();

    let mut statuses: Vec<String> = beads.iter().map(|b| b.status.clone()).collect();
    statuses.sort();
    statuses.dedup();
    statuses.sort_by_key(|s| (status_rank(s), s.clone()));

    for status in statuses {
        let mut group: Vec<&_> = beads.iter().filter(|b| b.status == status).collect();
        group.sort_by_key(|a| a.priority);
        println!(
            "{}{} ({})",
            status_glyph(&status),
            status_label(&status),
            group.len()
        );
        for b in group {
            println!(
                "  P{} {:<14} {}",
                b.priority,
                b.id,
                b.title.chars().take(70).collect::<String>()
            );
        }
        println!();
    }
    Ok(())
}
