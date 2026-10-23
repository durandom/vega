//! The resets ledger commands of `vega capacity`: the Slayer's provider
//! quota resets (resets owned, a planned reset, resets used, an override of
//! the spawn warning), recorded in vega's ledger (`vega_herdr::resets`).
//! vega never contacts a provider.

use std::path::Path;

use vega_herdr::resets::{self, Ledger};
use vega_herdr::{cell, Failure};

pub const USAGE: &str = "vega capacity [resets <provider> <n> | plan <provider> --this-week|--at <YYYY-MM-DD>|--clear | used <provider> | override <provider> --until <YYYY-MM-DD>|--clear]";

/// The ledger file, or why there is none.
pub fn file() -> Result<std::path::PathBuf, Failure> {
    resets::path().ok_or_else(|| {
        Failure::new(
            "no_state_dir",
            "neither XDG_STATE_HOME nor HOME is set",
            "set XDG_STATE_HOME",
        )
    })
}

/// Applies a command to the ledger at `file` (none: reads it) and returns
/// the ledger table. `known`: the providers the capacity source reports
/// (`None`: the source is unknown, any provider goes).
pub fn apply_at(
    file: &Path,
    args: &[String],
    known: Option<&[String]>,
    now: i64,
) -> Result<Vec<String>, Failure> {
    let mut ledger = resets::load(file).map_err(|why| {
        Failure::new(
            "ledger_unreadable",
            why,
            "fix the JSON or remove the file; vega does not overwrite it",
        )
    })?;
    if !args.is_empty() {
        change(&mut ledger, args, known, now)?;
        resets::save(file, &ledger).map_err(|e| {
            Failure::new(
                "ledger_write_failed",
                format!("{}: {e}", file.display()),
                "check the state dir",
            )
        })?;
    }
    Ok(table(&ledger, now))
}

/// A provider the source reports, or one already in the ledger (its
/// entry stays editable when the source drops it).
fn change(
    ledger: &mut Ledger,
    args: &[String],
    known: Option<&[String]>,
    now: i64,
) -> Result<(), Failure> {
    let a: Vec<&str> = args.iter().map(String::as_str).collect();
    if let (Some(p), Some(known)) = (a.get(1), known) {
        if !known.iter().any(|k| k == p) && !ledger.providers.contains_key(*p) {
            return Err(Failure::new(
                "unknown_provider",
                format!("{p} is not in the capacity sample"),
                &format!("use one of {}", known.join(", ")),
            ));
        }
    }
    let refused = |why: String| {
        Failure::new(
            "refused",
            why,
            "record the resets owned first: vega capacity resets <provider> <n>",
        )
    };
    let bad_day = |why: String| Failure::new("usage", why, USAGE);
    match a.as_slice() {
        ["resets", p, n] => {
            let n = n.parse().map_err(|_| {
                Failure::new(
                    "usage",
                    format!("{n} is no count"),
                    "pass a whole number, e.g. 1",
                )
            })?;
            ledger.set_owned(p, n);
        }
        ["plan", p, "--this-week"] => ledger.plan(p, &resets::this_week(now)).map_err(refused)?,
        ["plan", p, "--at", d] => {
            resets::check_day(d).map_err(bad_day)?;
            ledger.plan(p, d).map_err(refused)?
        }
        ["plan", p, "--clear"] => ledger.clear_plan(p),
        ["used", p] => ledger.used(p, now).map_err(refused)?,
        ["override", p, "--until", d] => ledger.override_until(p, d).map_err(bad_day)?,
        ["override", p, "--clear"] => ledger.clear_override(p),
        _ => {
            return Err(Failure::new(
                "usage",
                format!("unknown capacity command {}", args.join(" ")),
                USAGE,
            ))
        }
    }
    Ok(())
}

/// `resets[n]{provider,owned,plan,override_until,last_used}:` and its rows,
/// or `resets: none recorded` with the command that records some.
pub fn table(ledger: &Ledger, now: i64) -> Vec<String> {
    if ledger.providers.is_empty() {
        return vec![
            "resets: none recorded".into(),
            "help: vega capacity resets <provider> <n>".into(),
        ];
    }
    let mut out = vec![format!(
        "resets[{}]{{provider,owned,plan,override_until,last_used}}:",
        ledger.providers.len()
    )];
    for (p, e) in &ledger.providers {
        let plan = match (&e.plan, ledger.cover(p, now)) {
            (Some(d), Some(c @ resets::Cover::Plan(_))) => format!("{d} ({})", c.text(now)),
            (Some(d), _) if e.plan_passed(now).is_some() => format!("{d} (passed)"),
            (Some(d), _) => d.clone(),
            (None, _) => String::new(),
        };
        let over = e
            .override_until
            .as_deref()
            .map(|d| {
                if resets::day_end(d).is_some_and(|t| t < now) {
                    format!("{d} (passed)")
                } else {
                    d.to_string()
                }
            })
            .unwrap_or_default();
        let last = e
            .used
            .last()
            .map(|&t| resets::day_of(t))
            .unwrap_or_default();
        out.push(format!(
            "  {},{},{},{},{}",
            p,
            e.owned,
            cell(&plan),
            cell(&over),
            last
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Sunday 2026-10-11 12:00 UTC.
    const SUN: i64 = 1_791_720_000;

    fn run(file: &Path, args: &[&str]) -> Result<Vec<String>, Failure> {
        let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        let known: Vec<String> = ["claude", "codex", "acme"].map(String::from).to_vec();
        apply_at(file, &args, Some(&known), SUN)
    }

    #[test]
    fn records_owned_plan_used_and_override_in_a_temp_ledger() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("vega/capacity.json");
        assert_eq!(run(&file, &[]).unwrap()[0], "resets: none recorded");
        assert!(!file.exists(), "reading writes nothing");

        let e = run(&file, &["plan", "claude", "--this-week"]).unwrap_err();
        assert_eq!(e.code, "refused");

        run(&file, &["resets", "claude", "1"]).unwrap();
        let t = run(&file, &["plan", "claude", "--this-week"]).unwrap();
        assert_eq!(
            t,
            [
                "resets[1]{provider,owned,plan,override_until,last_used}:",
                "  claude,1,2026-10-17 (reset planned this week),,",
            ]
        );
        let t = run(&file, &["override", "codex", "--until", "2026-10-14"]).unwrap();
        assert_eq!(t[1], "  claude,1,2026-10-17 (reset planned this week),,");
        assert_eq!(t[2], "  codex,0,,2026-10-14,");
        let t = run(&file, &["used", "claude"]).unwrap();
        assert_eq!(t[1], "  claude,0,,,2026-10-11");
        assert_eq!(run(&file, &["used", "claude"]).unwrap_err().code, "refused");
        let t = run(&file, &["override", "codex", "--clear"]).unwrap();
        assert_eq!(t[2], "  codex,0,,,");
    }

    #[test]
    fn rejects_bad_input_without_writing() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("capacity.json");
        let code = |args: &[&str]| run(&file, args).unwrap_err().code;
        assert_eq!(code(&["resets", "nosuch", "1"]), "unknown_provider");
        assert_eq!(code(&["resets", "claude", "one"]), "usage");
        assert_eq!(code(&["plan", "claude", "--at", "tomorrow"]), "usage");
        assert_eq!(
            code(&["override", "claude", "--until", "2026-1-1"]),
            "usage"
        );
        assert_eq!(code(&["reset", "claude"]), "usage");
        assert!(!file.exists());
        std::fs::write(&file, "not json").unwrap();
        assert_eq!(code(&["resets", "claude", "1"]), "ledger_unreadable");
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "not json");
    }

    #[test]
    fn any_provider_the_source_reports_goes_and_an_unknown_source_checks_none() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("capacity.json");
        let t = run(&file, &["resets", "acme", "1"]).unwrap();
        assert_eq!(t[1], "  acme,1,,,");
        let args: Vec<String> = ["resets", "nosuch", "2"].map(String::from).to_vec();
        let t = apply_at(&file, &args, None, SUN).unwrap();
        assert_eq!(t[2], "  nosuch,2,,,");
        // In the ledger: still editable when the source no longer has it.
        let t = run(&file, &["resets", "nosuch", "0"]).unwrap();
        assert_eq!(t[2], "  nosuch,0,,,");
    }
}
