//! The TOON views: pure functions of the index and the live facts, so tests
//! can render them from a scratch tree.

use std::collections::BTreeMap;

use vega_herdr::{cell, collapse_home};

use crate::history;
use crate::index::{count, Index};
use crate::live;
use crate::peers::{self, Host};
use crate::remote;
use crate::scan::{Entry, Kind};
use crate::skills::{self, Skill};

/// Live facts joined at view time.
#[derive(Default)]
pub struct Live {
    pub panes: Vec<(String, String)>,
    pub installs: Vec<(String, String)>,
}

impl Live {
    /// Panes working in `e` (and not in a deeper checkout inside it).
    pub fn panes_of(&self, entries: &[Entry], e: &Entry) -> Vec<String> {
        self.panes
            .iter()
            .filter(|(cwd, _)| live::owner(entries, cwd).is_some_and(|o| o.path == e.path))
            .map(|(_, p)| p.clone())
            .collect()
    }

    pub fn installs_of(&self, entries: &[Entry], e: &Entry) -> Vec<String> {
        self.installs
            .iter()
            .filter(|(src, _)| live::owner(entries, src).is_some_and(|o| o.path == e.path))
            .map(|(_, l)| l.clone())
            .collect()
    }
}

/// `45s`, `3h`, `12d`, `5mo`, `2y`.
pub fn age(secs: i64) -> String {
    let s = secs.max(0);
    match s {
        _ if s < 60 => format!("{s}s"),
        _ if s < 3600 => format!("{}m", s / 60),
        _ if s < 86400 => format!("{}h", s / 3600),
        _ if s < 60 * 86400 => format!("{}d", s / 86400),
        _ if s < 730 * 86400 => format!("{}mo", s / (30 * 86400)),
        _ => format!("{}y", s / (365 * 86400)),
    }
}

const MONTH: i64 = 30 * 86400;

pub fn branch(e: &Entry) -> String {
    let b = e.branch.as_deref().unwrap_or("(detached)");
    let mut s = b.to_string();
    if e.gone {
        s.push_str(" (gone)");
    } else if e.merged {
        s.push_str(" (merged)");
    }
    s
}

fn remote_of(e: &Entry) -> &str {
    e.remote.as_deref().map_or("-", remote::display)
}

/// Clones (and bare repos) that share a remote: remote -> entries, most recently touched first.
pub fn dups(entries: &[Entry]) -> BTreeMap<&str, Vec<&Entry>> {
    let mut by: BTreeMap<&str, Vec<&Entry>> = BTreeMap::new();
    for e in entries.iter().filter(|e| e.kind != Kind::Worktree) {
        if let Some(r) = &e.remote {
            by.entry(r.as_str()).or_default().push(e);
        }
    }
    by.retain(|_, v| v.len() > 1);
    for v in by.values_mut() {
        v.sort_by_key(|e| -e.touched);
    }
    by
}

/// Clean clones untouched for `months`, nothing ahead, no worktree attached.
pub fn stale(entries: &[Entry], now: i64, months: i64) -> Vec<&Entry> {
    entries
        .iter()
        .filter(|e| {
            e.kind == Kind::Clone
                && now - e.touched > months * MONTH
                && e.dirty == 0
                && !e.slow
                && e.ahead == 0
                && !entries
                    .iter()
                    .any(|w| w.main.as_deref() == Some(e.path.as_str()))
        })
        .collect()
}

/// Worktrees whose branch is merged or gone, clean, nothing ahead, no pane in them.
pub fn done_worktrees<'a>(idx: &'a Index, live: &Live) -> Vec<&'a Entry> {
    idx.entries
        .iter()
        .filter(|e| {
            e.kind == Kind::Worktree
                && !e.prunable
                && (e.merged || e.gone)
                && e.dirty == 0
                && !e.slow
                && e.ahead == 0
                && live.panes_of(&idx.entries, e).is_empty()
        })
        .collect()
}

pub fn home(idx: &Index, live: &Live, host: &str, now: i64) -> String {
    let es = &idx.entries;
    let live_es: Vec<&Entry> = es.iter().filter(|e| !e.prunable).collect();
    let dirty = live_es.iter().filter(|e| e.dirty > 0).count();
    let ahead = live_es.iter().filter(|e| e.ahead > 0).count();
    let behind = live_es.iter().filter(|e| e.behind > 0).count();
    let prunable = es.iter().filter(|e| e.prunable).count();
    let slow = es.iter().filter(|e| e.slow).count();
    let dups = dups(es);
    let dup_clones: usize = dups.values().map(Vec::len).sum();
    let done = done_worktrees(idx, live).len();
    let stale = stale(es, now, 6).len();
    let merged_branches: usize = es.iter().map(|e| e.merged_branches.len()).sum();
    let roots: Vec<String> = idx.roots.iter().map(|r| collapse_home(r)).collect();
    let mut o = String::new();
    o += &format!("host: {host}\n");
    o += &format!("roots: {}\n", cell(&roots.join(":")));
    o += &format!(
        "scanned: {} ago in {} ms, walked {} ago; changed checkouts refresh on every call\n",
        age(now - idx.scanned_at),
        idx.scan_ms,
        age(now - idx.walked_at)
    );
    o += &format!(
        "checkouts: {} ({} clones, {} worktrees, {} bare)\n",
        live_es.len(),
        count(es, Kind::Clone),
        count(es, Kind::Worktree),
        count(es, Kind::Bare)
    );
    o += &format!("state: {dirty} dirty, {ahead} ahead, {behind} behind (as of the last fetch)\n");
    if idx.hosts.is_empty() {
        o += "other hosts: none (local only; vegad brings them through the registry)\n";
    } else {
        o += &format!(
            "other_hosts[{}]{{host,checkouts,as_of}}:\n",
            idx.hosts.len()
        );
        for h in &idx.hosts {
            let stale = if now - h.at > peers::STALE {
                " stale"
            } else {
                ""
            };
            o += &format!(
                "  {},{},{} ago{stale}\n",
                cell(&h.name),
                h.rows,
                age(now - h.at)
            );
        }
    }
    let attention: Vec<(String, usize, &str)> = [
        (
            format!("duplicate clones of {} remotes", dups.len()),
            dup_clones,
            "vega janitor list --dups",
        ),
        (
            "merged or gone worktrees".into(),
            done,
            "vega janitor report",
        ),
        ("prunable worktrees".into(), prunable, "vega janitor report"),
        (
            "clones untouched 6 months".into(),
            stale,
            "vega janitor list --stale",
        ),
        (
            "merged local branches".into(),
            merged_branches,
            "vega janitor report",
        ),
        ("dirty checkouts".into(), dirty, "vega janitor list --dirty"),
        (
            "dirty unknown (git status > 3 s)".into(),
            slow,
            "vega janitor report",
        ),
    ]
    .into_iter()
    .filter(|(_, n, _)| *n > 0)
    .collect();
    if attention.is_empty() {
        o += "attention: none\n";
    } else {
        o += &format!("attention[{}]{{what,count,see}}:\n", attention.len());
        for (what, n, see) in attention {
            o += &format!("  {},{n},{}\n", cell(&what), see);
        }
    }
    o += "help:\n  vega janitor find <name>    # repo name or owner/repo -> path(s)\n  vega janitor list [--dirty|--stale|--dups]\n  vega janitor report         # cleanup candidates with the command to run\n  vega janitor skills <name>  # which checkouts have a skill\n  vega janitor scan           # re-walk the roots (new clones)\n";
    o
}

type Tier<'a> = Box<dyn Fn(&Entry) -> bool + 'a>;
type SkillTier<'a> = Box<dyn Fn(&Skill) -> bool + 'a>;

/// Matches for `name`, best tier only: exact repo or dir name / owner/repo,
/// then substring of path or remote, then subsequence of the dir name.
pub fn find<'a>(entries: &'a [Entry], name: &str) -> Vec<&'a Entry> {
    let q = name.trim().trim_end_matches('/').to_lowercase();
    let q = q.strip_suffix(".git").unwrap_or(&q).to_string();
    let live: Vec<&Entry> = entries.iter().filter(|e| !e.prunable).collect();
    let base = |e: &Entry| e.path.rsplit('/').next().unwrap_or_default().to_lowercase();
    let remote = |e: &Entry| e.remote.as_deref().unwrap_or_default().to_lowercase();
    let tiers: [Tier; 3] = [
        Box::new(|e| {
            let r = remote(e);
            base(e) == q
                || (!r.is_empty() && remote::repo_name(&r) == q)
                || (q.contains('/') && (r == q || r.ends_with(&format!("/{q}"))))
        }),
        Box::new(|e| collapse_home(&e.path).to_lowercase().contains(&q) || remote(e).contains(&q)),
        Box::new(|e| subsequence(&q, &base(e))),
    ];
    for tier in tiers {
        let mut hits: Vec<&Entry> = live.iter().copied().filter(|e| tier(e)).collect();
        if !hits.is_empty() {
            // Clones first, then bare repos, then worktrees; newest first within each.
            let rank = |k: &Kind| match k {
                Kind::Clone => 0,
                Kind::Bare => 1,
                Kind::Worktree => 2,
            };
            hits.sort_by_key(|e| (rank(&e.kind), -e.touched));
            return hits;
        }
    }
    Vec::new()
}

fn subsequence(q: &str, s: &str) -> bool {
    let mut it = s.chars();
    q.chars().all(|c| it.any(|x| x == c))
}

/// `found` and `list` rows: this host's first, then the other hosts'.
pub fn found(
    entries: &[Entry],
    hits: &[&Entry],
    live: &Live,
    now: i64,
    host: &str,
    hosts: &[Host],
) -> String {
    let mut o = format!(
        "checkouts[{}]{{host,path,kind,branch,dirty,ahead,behind,touched,remote,pane,installed}}:\n",
        hits.len()
    );
    for e in hits {
        let (panes, inst) = if e.host.is_some() {
            (Vec::new(), Vec::new())
        } else {
            (live.panes_of(entries, e), live.installs_of(entries, e))
        };
        o += &format!(
            "  {},{},{},{},{},{},{},{},{},{},{}\n",
            cell(e.host.as_deref().unwrap_or(host)),
            cell(&collapse_home(&e.path)),
            kind_label(e),
            cell(&branch(e)),
            dirty(e),
            e.ahead,
            e.behind,
            age(now - e.touched),
            cell(remote_of(e)),
            cell(&dash(&panes.join(" "))),
            cell(&dash(&inst.join(" ")))
        );
    }
    o += &as_of(hits, hosts, now);
    o
}

/// When the other hosts in `rows` last changed, `stale` past a day.
fn as_of(rows: &[&Entry], hosts: &[Host], now: i64) -> String {
    let shown: Vec<String> = hosts
        .iter()
        .filter(|h| rows.iter().any(|e| e.host.as_deref() == Some(&h.name)))
        .map(|h| host_age(h, now))
        .collect();
    if shown.is_empty() {
        String::new()
    } else {
        format!("other hosts as of: {}\n", cell(&shown.join(", ")))
    }
}

fn host_age(h: &Host, now: i64) -> String {
    let stale = if now - h.at > peers::STALE {
        " stale"
    } else {
        ""
    };
    format!("{} {} ago{stale}", h.name, age(now - h.at))
}

/// The dirty count, `?` when `git status` was cut off.
fn dirty(e: &Entry) -> String {
    if e.slow {
        "?".into()
    } else {
        e.dirty.to_string()
    }
}

fn dash(s: &str) -> String {
    if s.is_empty() {
        "-".into()
    } else {
        s.into()
    }
}

fn kind_label(e: &Entry) -> String {
    let k = e.kind.as_str();
    if e.fork {
        format!("{k} fork")
    } else {
        k.to_string()
    }
}

#[derive(Clone, Copy, PartialEq)]
pub enum Filter {
    All,
    Dirty,
    Stale(i64),
    Dups,
}

/// One row per checkout; `All` and `Dirty` add the other hosts' rows.
pub fn list(
    idx: &Index,
    live: &Live,
    filter: Filter,
    limit: usize,
    now: i64,
    host: &str,
) -> String {
    let es = &idx.entries;
    let all = es.iter().chain(&idx.peers);
    let rows: Vec<&Entry> = match filter {
        Filter::All => all.filter(|e| !e.prunable).collect(),
        Filter::Dirty => all.filter(|e| !e.prunable && e.dirty > 0).collect(),
        Filter::Stale(m) => stale(es, now, m),
        Filter::Dups => dups(es).into_values().flatten().collect(),
    };
    let what = match filter {
        Filter::All => "checkouts".to_string(),
        Filter::Dirty => "dirty checkouts".to_string(),
        Filter::Stale(m) => format!("clean clones untouched {m} months"),
        Filter::Dups => "clones sharing a remote".to_string(),
    };
    if rows.is_empty() {
        return format!("{what}: none\n");
    }
    let shown = if limit == 0 {
        rows.len()
    } else {
        limit.min(rows.len())
    };
    // Stale rows also say since when (History): stale since touched + months,
    // last turned dirty.
    let stale_months = match filter {
        Filter::Stale(m) => Some(m),
        _ => None,
    };
    let mut o = format!(
        "{}[{}]{{host,path,kind,branch,dirty,ahead,behind,touched,remote,pane{}}}:\n",
        what.replace(' ', "_"),
        rows.len(),
        if stale_months.is_some() {
            ",stale_since,last_dirty"
        } else {
            ""
        }
    );
    for e in &rows[..shown] {
        let panes = if e.host.is_some() {
            Vec::new()
        } else {
            live.panes_of(es, e)
        };
        let since = stale_months.map_or(String::new(), |m| {
            let dirty = idx
                .last_dirty
                .get(&e.path)
                .map_or("-".into(), |t| format!("{} ago", age(now - t)));
            format!(",{} ago,{dirty}", age(now - e.touched - m * MONTH))
        });
        o += &format!(
            "  {},{},{},{},{},{},{},{},{},{}{since}\n",
            cell(e.host.as_deref().unwrap_or(host)),
            cell(&collapse_home(&e.path)),
            kind_label(e),
            cell(&branch(e)),
            dirty(e),
            e.ahead,
            e.behind,
            age(now - e.touched),
            cell(remote_of(e)),
            cell(&dash(&panes.join(" ")))
        );
    }
    if shown < rows.len() {
        o += &format!(
            "{} more; --limit 0 shows all, `vega janitor find <name>` one repo\n",
            rows.len() - shown
        );
    }
    o += &as_of(&rows[..shown], &idx.hosts, now);
    o
}

/// `history`'s events, newest first.
pub fn history(events: &[history::Event], now: i64) -> String {
    if events.is_empty() {
        return "events: none yet (history starts with the SQLite store)\n".into();
    }
    let mut o = format!("events[{}]{{when,path,what,detail}}:\n", events.len());
    for e in events {
        o += &format!(
            "  {} ago,{},{},{}\n",
            age(now - e.at),
            cell(&collapse_home(&e.path)),
            e.what,
            cell(&dash(&e.detail))
        );
    }
    o
}

pub fn report(idx: &Index, live: &Live, months: i64, limit: usize, now: i64) -> String {
    let es = &idx.entries;
    let take = |n: usize| if limit == 0 { n } else { limit.min(n) };
    let more = |o: &mut String, n: usize, shown: usize| {
        if shown < n {
            *o += &format!("  … {} more; --limit 0 shows all\n", n - shown);
        }
    };
    let mut o = String::from(
        "report: read-only; janitor changes nothing. Review each command before running it.\n",
    );

    let prunable: Vec<&Entry> = es.iter().filter(|e| e.prunable).collect();
    let mut repos: Vec<&str> = prunable.iter().filter_map(|e| e.main.as_deref()).collect();
    repos.sort();
    repos.dedup();
    if repos.is_empty() {
        o += "prunable_worktrees: none\n";
    } else {
        o += &format!(
            "prunable_worktrees[{}]{{repo,worktrees,cmd}}:\n",
            repos.len()
        );
        for r in repos {
            let n = prunable
                .iter()
                .filter(|e| e.main.as_deref() == Some(r))
                .count();
            o += &format!(
                "  {},{n},{}\n",
                cell(&collapse_home(r)),
                cell(&format!("git -C {} worktree prune", collapse_home(r)))
            );
        }
    }

    let done = done_worktrees(idx, live);
    if done.is_empty() {
        o += "merged_worktrees: none\n";
    } else {
        o += &format!(
            "merged_worktrees[{}]{{path,branch,touched,cmd}}:\n",
            done.len()
        );
        let n = take(done.len());
        for e in &done[..n] {
            let main = collapse_home(e.main.as_deref().unwrap_or("?"));
            o += &format!(
                "  {},{},{},{}\n",
                cell(&collapse_home(&e.path)),
                cell(&branch(e)),
                age(now - e.touched),
                cell(&format!(
                    "git -C {main} worktree remove {}",
                    collapse_home(&e.path)
                ))
            );
        }
        more(&mut o, done.len(), n);
    }

    let branches: Vec<&Entry> = es
        .iter()
        .filter(|e| !e.merged_branches.is_empty())
        .collect();
    if branches.is_empty() {
        o += "merged_branches: none\n";
    } else {
        o += &format!(
            "merged_branches[{}]{{repo,count,into,cmd}}:\n",
            branches.len()
        );
        let n = take(branches.len());
        for e in &branches[..n] {
            o += &format!(
                "  {},{},{},{}\n",
                cell(&collapse_home(&e.path)),
                e.merged_branches.len(),
                e.default_branch.as_deref().unwrap_or("-"),
                cell(&format!(
                    "git -C {} branch -d {}",
                    collapse_home(&e.path),
                    e.merged_branches.join(" ")
                ))
            );
        }
        more(&mut o, branches.len(), n);
    }

    let dups = dups(es);
    if dups.is_empty() {
        o += "duplicate_clones: none\n";
    } else {
        o += &format!(
            "duplicate_clones[{}]{{remote,paths (most recently touched first),cmd}}:\n",
            dups.len()
        );
        let n = take(dups.len());
        for (r, v) in dups.iter().take(n) {
            let paths: Vec<String> = v
                .iter()
                .map(|e| {
                    format!(
                        "{} ({}{})",
                        collapse_home(&e.path),
                        branch(e),
                        if e.dirty > 0 { ", dirty" } else { "" }
                    )
                })
                .collect();
            o += &format!(
                "  {},{},{}\n",
                cell(remote::display(r)),
                cell(&paths.join(" | ")),
                "keep one; diff the others first"
            );
        }
        more(&mut o, dups.len(), n);
    }

    let stale = stale(es, now, months);
    if stale.is_empty() {
        o += &format!("stale_clones: none untouched {months} months\n");
    } else {
        o += &format!(
            "stale_clones[{}]{{path,touched,remote,cmd}}:\n",
            stale.len()
        );
        let n = take(stale.len());
        for e in &stale[..n] {
            o += &format!(
                "  {},{},{},{}\n",
                cell(&collapse_home(&e.path)),
                age(now - e.touched),
                cell(remote_of(e)),
                cell(&format!("trash {}", collapse_home(&e.path)))
            );
        }
        more(&mut o, stale.len(), n);
    }
    let slow: Vec<&Entry> = es.iter().filter(|e| e.slow).collect();
    if slow.is_empty() {
        o += "slow_status: none\n";
    } else {
        o += &format!(
            "slow_status[{}]{{path,cmd}}: git status took over 3 s, so dirty is unknown; usually stale index stat data that a read-only status rehashes on every call. One plain status rewrites the index.\n",
            slow.len()
        );
        let n = take(slow.len());
        for e in &slow[..n] {
            o += &format!(
                "  {},{}\n",
                cell(&collapse_home(&e.path)),
                cell(&format!("git -C {} status", collapse_home(&e.path)))
            );
        }
        more(&mut o, slow.len(), n);
    }
    o += "notes: merged = reachable from the default branch as of the last fetch; squash-merged branches show as gone only after a fetch --prune. Worktrees with a herdr pane in them are left out.\n";
    o
}

/// Which checkouts `skills` looks at.
pub struct Scope<'a> {
    /// `--mine`: only checkouts with no hosted remote or one owned by this user.
    pub mine: Option<&'a str>,
    pub worktrees: bool,
}

impl Scope<'_> {
    fn admits(&self, e: &Entry) -> bool {
        !e.prunable
            && (self.worktrees || e.kind != Kind::Worktree)
            && self.mine.is_none_or(|me| {
                e.remote
                    .as_deref()
                    .and_then(skills::remote_owner)
                    .is_none_or(|o| o.eq_ignore_ascii_case(me))
            })
    }
}

/// (skill, checkout) pairs in scope matching `query`, best tier only: exact
/// name, then a substring of the name (with `desc`, also of the description),
/// then a subsequence of the name. No query: all. Sorted by skill, then path.
pub fn skill_hits<'a>(
    entries: &'a [Entry],
    scope: &Scope,
    query: Option<&str>,
    desc: bool,
) -> Vec<(&'a Skill, &'a Entry)> {
    let all: Vec<(&Skill, &Entry)> = entries
        .iter()
        .filter(|e| scope.admits(e))
        .flat_map(|e| e.skills.iter().map(move |s| (s, e)))
        .collect();
    let mut hits = match query {
        None => all,
        Some(q) => {
            let q = q.trim().to_lowercase();
            let name = |s: &Skill| s.name.to_lowercase();
            let tiers: [SkillTier; 3] = [
                Box::new(|s| name(s) == q),
                Box::new(|s| {
                    name(s).contains(&q) || (desc && s.description.to_lowercase().contains(&q))
                }),
                Box::new(|s| subsequence(&q, &name(s))),
            ];
            tiers
                .iter()
                .map(|t| {
                    all.iter()
                        .copied()
                        .filter(|(s, _)| t(s))
                        .collect::<Vec<_>>()
                })
                .find(|h| !h.is_empty())
                .unwrap_or_default()
        }
    };
    hits.sort_by(|a, b| (&a.0.name, &a.1.path).cmp(&(&b.0.name, &b.1.path)));
    hits
}

/// One row per (skill, checkout).
pub fn skill_rows(hits: &[(&Skill, &Entry)], limit: usize) -> String {
    let shown = if limit == 0 {
        hits.len()
    } else {
        limit.min(hits.len())
    };
    let mut o = format!(
        "skills[{}]{{skill,checkout,dirs,kind,source,remote}}:\n",
        hits.len()
    );
    for (s, e) in &hits[..shown] {
        o += &format!(
            "  {},{},{},{},{},{}\n",
            cell(&s.name),
            cell(&collapse_home(&e.path)),
            cell(&s.dirs()),
            cell(&s.kinds()),
            cell(s.source()),
            cell(remote_of(e))
        );
    }
    if shown < hits.len() {
        o += &format!("{} more; --limit 0 shows all\n", hits.len() - shown);
    }
    o
}

/// Checkouts per skill name, most used first.
pub fn skill_counts(hits: &[(&Skill, &Entry)], limit: usize) -> String {
    let mut by: BTreeMap<&str, (usize, &str)> = BTreeMap::new();
    for (s, _) in hits {
        let c = by.entry(&s.name).or_insert((0, &s.description));
        c.0 += 1;
        if c.1.is_empty() {
            c.1 = &s.description;
        }
    }
    let mut rows: Vec<(&str, usize, &str)> = by.into_iter().map(|(n, (c, d))| (n, c, d)).collect();
    rows.sort_by_key(|(n, c, _)| (std::cmp::Reverse(*c), *n));
    let shown = if limit == 0 {
        rows.len()
    } else {
        limit.min(rows.len())
    };
    let mut o = format!("skills[{}]{{skill,checkouts,description}}:\n", rows.len());
    for (n, c, d) in &rows[..shown] {
        o += &format!("  {},{c},{}\n", cell(n), cell(&dash(d)));
    }
    if shown < rows.len() {
        o += &format!(
            "{} more; --limit 0 shows all, `vega janitor skills <name>` one skill\n",
            rows.len() - shown
        );
    }
    o
}
