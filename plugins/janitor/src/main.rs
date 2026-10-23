//! `vega janitor`: a map of every git checkout on this machine
//! (docs/design/janitor.md).
//!
//! Walks the roots (`VEGA_JANITOR_ROOTS`, default `~/src`) for clones,
//! worktrees and bare repos, caches what read-only git says about each, and
//! answers: where is repo X, what needs attention, what could be cleaned up.
//! It never changes, fetches or deletes anything in the scanned repos.

mod history;
mod index;
mod live;
mod peers;
mod remote;
mod scan;
mod search;
mod skills;
mod views;

use std::env;
use std::process::ExitCode;

use clap::error::ErrorKind;
use clap::{CommandFactory, Parser, Subcommand};
use vega_herdr::state::post_host;
use vega_herdr::{cell, collapse_home, local_host, print_failure, Failure};

use crate::index::Index;
use crate::views::{Filter, Live, Scope};

const SKILL: &str = include_str!("../SKILL.md");

#[derive(Parser)]
#[command(
    name = "vega janitor",
    bin_name = "vega janitor",
    disable_version_flag = true,
    about = "Map of every git checkout on this machine: find by name, overview, cleanup report (read-only)",
    after_help = "Roots: VEGA_JANITOR_ROOTS (colon-separated), default ~/src.\nCache: $XDG_STATE_HOME/vega/janitor/janitor.db.\n\nExamples:\n  vega janitor                  # counts and what needs attention\n  vega janitor find lanista     # name -> path(s)\n  vega janitor list --dups\n  vega janitor report\n  vega janitor skills discord"
)]
struct Cli {
    /// Print the agent guide for this plugin, SKILL.md form (read by `vega skills`)
    #[arg(long)]
    skill: bool,
    /// Print the bare version
    #[arg(short = 'V', short_alias = 'v', long)]
    version: bool,
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Resolve a repo name, owner/repo or fuzzy part to its checkout(s)
    #[command(
        after_help = "Examples:\n  vega janitor find lanista\n  vega janitor find durandom/vega\n  vega janitor find vega --paths"
    )]
    Find {
        name: String,
        /// Print bare paths, one per line
        #[arg(long)]
        paths: bool,
    },
    /// One row per checkout, optionally filtered
    #[command(
        after_help = "Examples:\n  vega janitor list --dirty\n  vega janitor list --stale --months 12\n  vega janitor list --dups --limit 0"
    )]
    List {
        /// Only checkouts with uncommitted or untracked files
        #[arg(long, group = "filter")]
        dirty: bool,
        /// Only clean clones untouched for --months, nothing ahead, no worktree
        #[arg(long, group = "filter")]
        stale: bool,
        /// Only clones that share a remote with another clone
        #[arg(long, group = "filter")]
        dups: bool,
        /// Months for --stale
        #[arg(long, default_value_t = 6)]
        months: i64,
        /// Rows to show; 0 for all
        #[arg(long, default_value_t = 50)]
        limit: usize,
    },
    /// Cleanup candidates, each with the command a human would run
    #[command(
        after_help = "Examples:\n  vega janitor report\n  vega janitor report --months 12 --limit 0"
    )]
    Report {
        /// Months without a commit, checkout or index change for a stale clone
        #[arg(long, default_value_t = 6)]
        months: i64,
        /// Rows per section; 0 for all
        #[arg(long, default_value_t = 30)]
        limit: usize,
    },
    /// Which checkouts install or own a skill; no query: counts per skill
    #[command(
        after_help = "Matches the skill name: exact, then substring, then fuzzy; --desc adds the description to the\nsubstring tier. Skills refresh with the git state: after adding one without a git command,\nrun `vega janitor scan`. --mine: checkouts with no hosted remote or one owned by\nVEGA_JANITOR_OWNER, else `git config github.user`, else gh's login.\n\nExamples:\n  vega janitor skills discord\n  vega janitor skills chat --desc\n  vega janitor skills --mine --limit 0"
    )]
    Skills {
        query: Option<String>,
        /// Also match the description (substring tier)
        #[arg(long)]
        desc: bool,
        /// Only checkouts with no hosted remote or one owned by you
        #[arg(long)]
        mine: bool,
        /// Include worktrees, not only clones
        #[arg(long)]
        worktrees: bool,
        /// Rows to show; 0 for all
        #[arg(long, default_value_t = 50)]
        limit: usize,
    },
    /// Find repos by what their README says: words always, meaning after `embed`
    #[command(
        after_help = "Ranks this host's clones by their README (first 8 KB, with the repo name). Words: SQLite\nFTS5, offline. Meaning: after `vega janitor embed`, the query is embedded too and both\nrankings are fused; when the endpoint does not answer, words only, said on stderr.\n\nExamples:\n  vega janitor search \"map of git checkouts\"\n  vega janitor search kubernetes operator --words"
    )]
    Search {
        #[arg(required = true)]
        query: Vec<String>,
        /// Words only: no embedding call
        #[arg(long)]
        words: bool,
        /// Rows to show
        #[arg(long, default_value_t = 10)]
        limit: usize,
    },
    /// Embed READMEs that have no vector yet (network: the embedding endpoint)
    #[command(
        after_help = "Endpoint: VEGA_JANITOR_EMBED_ENDPOINT or ~/.config/vega/janitor-embed.url (required),\nmodel: VEGA_JANITOR_EMBED_MODEL (default qwen3-embedding-0.6b-q8-0), key: the vega secret\ntriton-embedding for tool janitor. Vectors are keyed by model and text: unchanged READMEs\ncost nothing, an interrupted run keeps what it embedded.\n\nExamples:\n  vega janitor embed"
    )]
    Embed,
    /// What happened to a repo's checkouts on this host, also vanished ones
    #[command(
        after_help = "Events, newest first: appeared, vanished, dirty, clean, branch, commit. Recorded when a\ncall sees the change (kept a year); matches the name as `find` does, plus vanished paths.\n\nExamples:\n  vega janitor history lanista\n  vega janitor history vega --limit 0"
    )]
    History {
        name: String,
        /// Events to show; 0 for all
        #[arg(long, default_value_t = 30)]
        limit: usize,
    },
    /// Re-walk the roots and re-query every checkout
    Scan,
}

fn main() -> ExitCode {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(e) if matches!(e.kind(), ErrorKind::DisplayHelp) => {
            let _ = e.print();
            return ExitCode::SUCCESS;
        }
        Err(e) => {
            let msg = e.to_string();
            let message = msg
                .lines()
                .take_while(|l| !l.trim().is_empty())
                .map(str::trim)
                .collect::<Vec<_>>()
                .join(" ");
            print_failure(&Failure::new(
                "usage",
                message.trim_start_matches("error: "),
                "valid usage below",
            ));
            let mut cmd = Cli::command();
            cmd.build();
            let sub = env::args().nth(1).filter(|a| !a.starts_with('-'));
            let help = match sub.as_deref().and_then(|s| cmd.find_subcommand_mut(s)) {
                Some(sc) => sc.render_help(),
                None => cmd.render_help(),
            };
            print!("{help}");
            return ExitCode::from(2);
        }
    };
    if cli.version {
        println!("{}", vega_build::version_line!());
        return ExitCode::SUCCESS;
    }
    if cli.skill {
        print!("{SKILL}");
        return ExitCode::SUCCESS;
    }
    match run(cli.cmd) {
        Ok(out) => {
            print!("{out}");
            ExitCode::SUCCESS
        }
        Err(f) => {
            print_failure(&f);
            ExitCode::from(1)
        }
    }
}

fn run(cmd: Option<Cmd>) -> Result<String, Failure> {
    let roots = index::roots();
    if roots.is_empty() {
        return Err(Failure::new(
            "no_roots",
            "no root directory exists",
            "set VEGA_JANITOR_ROOTS to existing dirs (colon-separated); the default is ~/src",
        ));
    }
    let cache = index::cache_path();
    let fail = |e: String| Failure::new("cache", e, "check $XDG_STATE_HOME/vega/janitor");
    let now = index::now();
    let live = || Live {
        panes: live::panes(),
        installs: live::installs(),
    };
    match cmd {
        None => {
            let idx = index::load_fresh(&roots, &cache).map_err(fail)?;
            Ok(views::home(&idx, &live(), &local_host(), now))
        }
        Some(Cmd::Scan) => {
            let idx = index::rescan(&roots, &cache).map_err(fail)?;
            Ok(format!(
                "scanned: {} checkouts in {} ms\nnext: vega janitor\n",
                idx.entries.iter().filter(|e| !e.prunable).count(),
                idx.scan_ms
            ))
        }
        Some(Cmd::Find { name, paths }) => {
            let mut idx = index::load_fresh(&roots, &cache).map_err(fail)?;
            if views::find(&idx.entries, &name).is_empty() && index::now() - idx.walked_at > 60 {
                // A fresh clone is the usual cause of a miss; a walk finds it.
                index::rewalk(&roots, &mut idx);
                index::save(&cache, &idx).map_err(fail)?;
            }
            find(&idx, &name, paths, now)
        }
        Some(Cmd::List {
            dirty,
            stale,
            dups,
            months,
            limit,
        }) => {
            let idx = index::load_fresh(&roots, &cache).map_err(fail)?;
            let filter = if dirty {
                Filter::Dirty
            } else if stale {
                Filter::Stale(months)
            } else if dups {
                Filter::Dups
            } else {
                Filter::All
            };
            Ok(views::list(&idx, &live(), filter, limit, now, &post_host()))
        }
        Some(Cmd::Skills {
            query,
            desc,
            mine,
            worktrees,
            limit,
        }) => {
            let me = if mine {
                Some(skills::me().ok_or_else(|| {
                    Failure::new(
                        "no_owner",
                        "--mine needs your login",
                        "set VEGA_JANITOR_OWNER or `git config --global github.user <login>`",
                    )
                })?)
            } else {
                None
            };
            let idx = index::load_fresh(&roots, &cache).map_err(fail)?;
            let scope = Scope {
                mine: me.as_deref(),
                worktrees,
            };
            let hits = views::skill_hits(&idx.entries, &scope, query.as_deref(), desc);
            match query {
                None => Ok(views::skill_counts(&hits, limit)),
                Some(q) if hits.is_empty() => Err(Failure::new(
                    "not_found",
                    format!("no skill matches {}", cell(&q)),
                    "try a shorter part, --desc or --worktrees; added without a git command: vega janitor scan",
                )),
                Some(_) => Ok(views::skill_rows(&hits, limit)),
            }
        }
        Some(Cmd::Search {
            query,
            words,
            limit,
        }) => {
            let idx = index::load_fresh(&roots, &cache).map_err(fail)?;
            search(&idx, &cache, &query.join(" "), words, limit)
        }
        Some(Cmd::Embed) => {
            index::load_fresh(&roots, &cache).map_err(fail)?;
            let ep = search::Endpoint::from_env()
                .map_err(|e| Failure::new(e.code, e.message, &e.hint))?;
            let mut db = index::open(&cache).map_err(fail)?;
            let done = search::embed_all(&mut db, &ep.model, |t| {
                ep.embed(t, std::time::Duration::from_secs(120))
            })
            .map_err(|e| {
                Failure::new(
                    "embed",
                    e,
                    "check VEGA_JANITOR_EMBED_ENDPOINT / ~/.config/vega/janitor-embed.url and the network; vectors made so far are kept",
                )
            })?;
            Ok(format!(
                "embedded: {} new, {} reused, {} READMEs ({})\nnext: vega janitor search <what the repo does>\n",
                done.new, done.reused, done.total, ep.model
            ))
        }
        Some(Cmd::History { name, limit }) => {
            let idx = index::load_fresh(&roots, &cache).map_err(fail)?;
            let db = index::open(&cache).map_err(fail)?;
            let sql = |e: rusqlite::Error| fail(e.to_string());
            let paths = history_paths(&idx, &db, &name).map_err(sql)?;
            if paths.is_empty() {
                return Err(Failure::new(
                    "not_found",
                    format!("no checkout, now or vanished, matches {}", cell(&name)),
                    "try a shorter part; history covers this host since the SQLite store",
                ));
            }
            let events = history::events(&db, &paths, limit).map_err(sql)?;
            Ok(views::history(&events, now))
        }
        Some(Cmd::Report { months, limit }) => {
            let idx = index::load_fresh(&roots, &cache).map_err(fail)?;
            Ok(views::report(&idx, &live(), months, limit, now))
        }
    }
}

/// Paths `history` covers: `find`'s hits, plus vanished paths whose name,
/// repo or path matches.
fn history_paths(
    idx: &Index,
    db: &rusqlite::Connection,
    name: &str,
) -> rusqlite::Result<Vec<String>> {
    let mut paths: Vec<String> = views::find(&idx.entries, name)
        .into_iter()
        .map(|e| e.path.clone())
        .collect();
    let current = idx.entries.iter().map(|e| e.path.as_str()).collect();
    let q = name.trim().trim_end_matches('/').to_lowercase();
    for (p, remote) in history::vanished(db, &current)? {
        let base = p.rsplit('/').next().unwrap_or_default().to_lowercase();
        if base == q
            || remote::repo_name(&remote.to_lowercase()) == q
            || p.to_lowercase().contains(&q)
        {
            paths.push(p);
        }
    }
    Ok(paths)
}

/// README search: words, fused with meaning when this host has vectors of
/// the model and the endpoint answers.
fn search(
    idx: &Index,
    cache: &std::path::Path,
    q: &str,
    words_only: bool,
    limit: usize,
) -> Result<String, Failure> {
    let fail = |e: String| Failure::new("cache", e, "check $XDG_STATE_HOME/vega/janitor");
    let db = index::open(cache).map_err(fail)?;
    let words = search::words(&db, q, 50).map_err(|e| fail(e.to_string()))?;
    let mut legs = vec![words];
    let mut mode = "words";
    if !words_only {
        let model = search::model();
        let note = if search::vectors_of(&db, &model) == 0 {
            Some("no README vectors yet (vega janitor embed)".to_string())
        } else {
            match search::Endpoint::from_env().map_err(|e| e.message) {
                Ok(ep) => match ep.query(q) {
                    Ok(v) => {
                        let hits = search::meaning(&db, &v, 50).map_err(|e| fail(e.to_string()))?;
                        legs.push(hits.into_iter().map(|(p, _)| p).collect());
                        mode = "meaning+words";
                        None
                    }
                    Err(e) => Some(e),
                },
                Err(e) => Some(e),
            }
        };
        if let Some(n) = note {
            eprintln!("janitor: words only: {n}");
        }
    }
    let ranked = search::fuse(&legs);
    if ranked.is_empty() {
        return Err(Failure::new(
            "not_found",
            format!("no README matches {}", cell(q)),
            "other words; READMEs of new clones are read on the next call after `vega janitor scan`",
        ));
    }
    let remote: std::collections::BTreeMap<&str, &str> = idx
        .entries
        .iter()
        .filter_map(|e| Some((e.path.as_str(), e.remote.as_deref()?)))
        .collect();
    let shown = &ranked[..limit.min(ranked.len())];
    let mut o = format!("search: {mode}\n");
    o += &format!("readmes[{}]{{path,remote,score,about}}:\n", shown.len());
    for (p, score) in shown {
        o += &format!(
            "  {},{},{score:.2},{}\n",
            cell(&collapse_home(p)),
            cell(remote.get(p.as_str()).map_or("-", |r| remote::display(r))),
            cell(&search::about(&db, p))
        );
    }
    if ranked.len() > shown.len() {
        o += &format!(
            "{} more; --limit N shows more\n",
            ranked.len() - shown.len()
        );
    }
    Ok(o)
}

fn find(idx: &Index, name: &str, paths: bool, now: i64) -> Result<String, Failure> {
    let mut hits = views::find(&idx.entries, name);
    let local = hits.len();
    hits.extend(views::find(&idx.peers, name));
    if hits.is_empty() || (paths && local == 0) {
        let roots: Vec<String> = idx.roots.iter().map(|r| collapse_home(r)).collect();
        let elsewhere: Vec<&str> = hits.iter().filter_map(|e| e.host.as_deref()).collect();
        let hint = if elsewhere.is_empty() {
            "try owner/repo or a shorter part; other roots via VEGA_JANITOR_ROOTS".to_string()
        } else {
            format!(
                "checked out on {}: vega janitor find {} (without --paths)",
                elsewhere.join(" "),
                cell(name)
            )
        };
        return Err(Failure::new(
            "not_found",
            format!(
                "no checkout matches {} under {}",
                cell(name),
                roots.join(":")
            ),
            &hint,
        ));
    }
    if paths {
        // Paths for cd: this host's only.
        return Ok(hits[..local]
            .iter()
            .map(|e| format!("{}\n", e.path))
            .collect());
    }
    let live = Live {
        panes: live::panes(),
        installs: live::installs(),
    };
    Ok(views::found(
        &idx.entries,
        &hits,
        &live,
        now,
        &post_host(),
        &idx.hosts,
    ))
}

#[cfg(test)]
mod tests;
