//! `vega gate`: an LLM review before an agent's push. The git shim calls the
//! same library on `git push`; this binary is the agent's and the reviewer's
//! view: home, `check`, `start`, `verdict`, `log`, `gc`. Design:
//! docs/design/gate.md.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use vega_gate::git::{config_at, current_branch, default_target, is_beads_repo, toplevel};
use vega_gate::record::{self, Status};
use vega_gate::segments::{plan, short, Plan};
use vega_gate::start::{self, Outcome};
use vega_gate::{age, cache_dir, state, verdict};
use vega_herdr::{cell, print_failure, Failure};

const HELP: &str = include_str!("../help/gate.md");
const SKILL: &str = include_str!("../SKILL.md");

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    if args.iter().any(|a| a == "-h" || a == "--help") {
        print!("{HELP}");
        return ExitCode::SUCCESS;
    }
    let strs: Vec<&str> = args.iter().map(String::as_str).collect();
    let result = match strs.as_slice() {
        ["-V" | "--version"] => {
            println!("{}", vega_build::version_line!());
            Ok(())
        }
        ["--skill"] => {
            print!("{SKILL}");
            Ok(())
        }
        [] => home(),
        ["check", rest @ ..] => target_opt(rest).and_then(|t| check(t.as_deref())),
        ["start", rest @ ..] => target_opt(rest).and_then(|t| start_cmd(t.as_deref())),
        ["verdict", v @ ("pass" | "fail"), rest @ ..] => verdict_cmd(*v == "pass", rest),
        ["log"] => log(None),
        ["log", "--since", d] => log(Some(d)),
        ["gc"] => gc(),
        _ => Err(Failure::new(
            "usage",
            format!("unknown arguments: {}", args.join(" ")),
            "vega gate --help",
        )),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(f) => {
            print_failure(&f);
            ExitCode::from(if f.code == "usage" { 2 } else { 1 })
        }
    }
}

fn target_opt(rest: &[&str]) -> Result<Option<String>, Failure> {
    match rest {
        [] => Ok(None),
        ["--target", t] => Ok(Some(t.to_string())),
        _ => Err(Failure::new(
            "usage",
            format!("unknown arguments: {}", rest.join(" ")),
            "vega gate check|start [--target <ref>]",
        )),
    }
}

fn fail<'a>(code: &'a str, hint: &'a str) -> impl Fn(String) -> Failure + 'a {
    move |m| Failure::new(code, m, hint)
}

/// The checkout, its branch, target, config and plan.
struct Ctx {
    dir: PathBuf,
    branch: String,
    target: String,
    found: Option<(vega_gate::config::Config, Plan)>,
}

fn ctx(target: Option<&str>) -> Result<Ctx, Failure> {
    let cwd = env::current_dir().map_err(|e| Failure::new("cwd", e.to_string(), ""))?;
    let dir = toplevel(&cwd).map_err(fail("not_a_repo", "run it in a git checkout"))?;
    let branch = current_branch(&dir).unwrap_or_else(|| "HEAD".into());
    let target = match target {
        Some(t) => t.to_string(),
        None => default_target(&dir)
            .map_err(fail("no_target", "pass --target <ref>, e.g. origin/main"))?,
    };
    let found = match config_at(&dir, &target)
        .map_err(fail("config", "fix .vega/gate.toml on the target"))?
    {
        None => None,
        Some((blob, cfg)) => {
            let p = plan(&dir, &cfg, &blob, &target, "HEAD")
                .map_err(fail("plan", "vega gate check --target <ref>"))?;
            Some((cfg, p))
        }
    };
    Ok(Ctx {
        dir,
        branch,
        target,
        found,
    })
}

fn home() -> Result<(), Failure> {
    let c = ctx(None)?;
    println!("repo: {}", cell(&c.dir.display().to_string()));
    match &c.found {
        None => println!(
            "config: none at {} (no .vega/gate.toml there: pushes are not gated)",
            c.target
        ),
        Some((cfg, _)) => println!(
            "config: .vega/gate.toml at {}; reviews {}; reviewers {}; tier {}; timeout {} s",
            c.target,
            cfg.reviews
                .iter()
                .map(|r| r.name.as_str())
                .collect::<Vec<_>>()
                .join(" "),
            cfg.reviewers.join(" "),
            cfg.tier,
            cfg.timeout_secs
        ),
    }
    if c.found.is_some() {
        segments_view(&c)?;
    }
    let running = state::all();
    println!(
        "reviews_running[{}]{{tab,pane,reviewer,branch,since,state}}:",
        running.len()
    );
    for r in &running {
        println!(
            "  {},{},{}/{},{},{},{}",
            r.tab,
            r.pane,
            r.kind,
            r.model,
            cell(&r.branch),
            age(vega_shim::unix_now() - r.started_at),
            cell(&start::reviewer_state(r))
        );
    }
    println!("help: vega gate check | start | log | --help");
    Ok(())
}

/// The segment table and a result line; returns the missing count.
fn segments_view(c: &Ctx) -> Result<usize, Failure> {
    let Some((_, p)) = &c.found else {
        println!("result: not gated (no .vega/gate.toml at {})", c.target);
        return Ok(0);
    };
    println!(
        "branch: {} @ {}, target {}, base {}",
        c.branch,
        short(&p.head),
        p.target,
        short(&p.base)
    );
    if !is_beads_repo(&c.dir) {
        println!("result: not gated (no .beads: beads repos only)");
        return Ok(0);
    }
    println!(
        "segments[{}]{{segment,key,reviews,status}}: ({} without relevant diff)",
        p.segments.len(),
        p.empty
    );
    let mut missing = 0;
    let mut running = 0;
    for s in &p.segments {
        let st = record::lookup(&c.dir, &s.key).map_err(fail("bd", "bd list --label gate"))?;
        let status = match record::status(st) {
            Status::Pass(b) => format!("pass {}", b.id),
            Status::Running(b) => {
                running += 1;
                format!("running {}", b.id)
            }
            Status::Missing { .. } => {
                missing += 1;
                "missing".into()
            }
        };
        println!(
            "  {},{},{},{}",
            cell(&s.label()),
            &s.key[..12],
            s.reviews.join(" "),
            status
        );
    }
    println!(
        "result: {}",
        match (missing, running) {
            (0, 0) => "pass (git push goes through)".to_string(),
            (0, r) => format!("review running ({r} segments)"),
            (m, _) => format!("review needed ({m} segments): vega gate start"),
        }
    );
    Ok(missing)
}

fn check(target: Option<&str>) -> Result<(), Failure> {
    let c = ctx(target)?;
    segments_view(&c).map(|_| ())
}

fn start_cmd(target: Option<&str>) -> Result<(), Failure> {
    let c = ctx(target)?;
    let Some((cfg, p)) = &c.found else {
        println!("result: not gated (no .vega/gate.toml at {})", c.target);
        return Ok(());
    };
    if !is_beads_repo(&c.dir) {
        println!("result: not gated (no .beads: beads repos only)");
        return Ok(());
    }
    if p.segments.is_empty() {
        println!("result: pass (no relevant diff against {})", c.target);
        return Ok(());
    }
    match start::start(&c.dir, p, cfg, &c.branch).map_err(fail("start", "vega gate check"))? {
        Outcome::Passed => println!("result: pass (every segment has one)"),
        Outcome::Running(m) => println!("result: {m}"),
        Outcome::FailOpen(m) => println!("result: not reviewed: {m}"),
        Outcome::Failed(m) => return Err(Failure::new("spawn", m, "vega mux spawn --help")),
    }
    Ok(())
}

fn verdict_cmd(pass: bool, rest: &[&str]) -> Result<(), Failure> {
    let findings = match rest {
        [] => Vec::new(),
        ["--findings", f] => {
            let text = fs::read_to_string(f).map_err(|e| {
                Failure::new(
                    "findings",
                    format!("{f}: {e}"),
                    "write JSON lines to a file",
                )
            })?;
            verdict::parse_findings(&text).map_err(fail(
                "findings",
                "one JSON object per line: {file, line, rubric, principle, score, fix}",
            ))?
        }
        _ => {
            return Err(Failure::new(
                "usage",
                format!("unknown arguments: {}", rest.join(" ")),
                "vega gate verdict pass|fail [--findings <file.jsonl>]",
            ))
        }
    };
    let r = verdict::authorize().map_err(fail(
        "not_reviewer",
        "only the gate reviewer pane runs this",
    ))?;
    for l in verdict::give(pass, &findings, &r).map_err(fail("verdict", "vega skills gate"))? {
        println!("{l}");
    }
    Ok(())
}

fn log(since: Option<&str>) -> Result<(), Failure> {
    let cwd = env::current_dir().map_err(|e| Failure::new("cwd", e.to_string(), ""))?;
    let dir = toplevel(&cwd).map_err(fail("not_a_repo", "run it in a git checkout"))?;
    let min = match since {
        None => None,
        Some(d) => {
            let secs = vega_gate::config::duration(d)
                .map_err(fail("usage", "vega gate log --since 7d"))?;
            Some(vega_shim::unix_now() - secs)
        }
    };
    let mut beads = record::all(&dir).map_err(fail("bd", "bd list --label gate"))?;
    beads.retain(|b| {
        min.is_none_or(|m| vega_transcript::parse_rfc3339(&b.created_at).is_some_and(|t| t >= m))
    });
    beads.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    if beads.is_empty() {
        println!(
            "gate_beads: none{}",
            since.map_or(String::new(), |d| format!(" in the last {d}"))
        );
        return Ok(());
    }
    println!("gate_beads[{}]{{id,verdict,created,title}}:", beads.len());
    for b in &beads {
        println!(
            "  {},{},{},{}",
            b.id,
            b.verdict(),
            b.created_at.get(..16).unwrap_or(&b.created_at),
            cell(&b.title)
        );
    }
    Ok(())
}

/// Removes reviews whose beads are closed or that are older than a day, and
/// clone dirs no review owns that are older than a day.
fn gc() -> Result<(), Failure> {
    const DAY: i64 = 86400;
    let now = vega_shim::unix_now();
    let mut removed = 0;
    let mut owned: Vec<PathBuf> = Vec::new();
    for r in state::all() {
        let closed = || {
            r.beads.iter().all(|b| {
                vega_gate::run_ok(
                    &vega_gate::tool("bd"),
                    &["show", b, "--json"],
                    Some(&r.repo),
                    Some(record::BD_TIMEOUT),
                )
                .ok()
                .and_then(|o| {
                    serde_json::from_str::<serde_json::Value>(&vega_plugin::unwrap_bd_envelope(&o))
                        .ok()
                })
                .is_some_and(|v| {
                    let issue = v.get(0).unwrap_or(&v);
                    issue.get("status").and_then(|s| s.as_str()) == Some("closed")
                })
            })
        };
        if now - r.started_at > DAY || !r.repo.exists() || closed() {
            start::cleanup(&r, start::pane_alive(&r.pane));
            removed += 1;
        } else {
            owned.push(r.clone.clone());
        }
    }
    let clones = cache_dir().join("clones");
    for e in fs::read_dir(&clones).into_iter().flatten().flatten() {
        let p = e.path();
        if owned.contains(&p) {
            continue;
        }
        if older_than(&p, DAY) {
            let _ = fs::remove_dir_all(&p);
            let work = cache_dir().join(e.file_name());
            let _ = fs::remove_dir_all(work);
            removed += 1;
        }
    }
    println!("gc: removed {removed} review(s); {} running", owned.len());
    Ok(())
}

fn older_than(p: &Path, secs: i64) -> bool {
    p.metadata()
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.elapsed().ok())
        .is_some_and(|e| e.as_secs() as i64 > secs)
}

#[cfg(test)]
mod tests {
    #[test]
    fn guide_meets_the_skill_contract() {
        vega_plugin::assert_guide(super::SKILL, "gate");
    }
}
