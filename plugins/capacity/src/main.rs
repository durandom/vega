//! `vega capacity`: subscription capacity from the capacity source (token-burn,
//! the Slayer's quota monitor, behind an adapter): every window per provider
//! with its scope and the binding one marked, the Slayer's resets ledger, and
//! token-burn's health.
//!
//! The reading stays in `vega_herdr::capacity` and `vega_herdr::resets`,
//! shared with `vega uac status` and `vega mux spawn`; this plugin owns the
//! full view and the ledger commands. Design: docs/design/capacity.md.

mod health;
mod ledger;

use std::env;
use std::process::ExitCode;

use vega_herdr::capacity::{self, duration, percent, Sample, Window};
use vega_herdr::resets::{self, Ledger};
use vega_herdr::{print_failure, Failure};

const HELP: &str = include_str!("../help/capacity.md");

const SKILL: &str = include_str!("../SKILL.md");

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    if args.iter().any(|a| a == "-h" || a == "--help") {
        print!("{HELP}");
        return ExitCode::SUCCESS;
    }
    let result = match args.as_slice() {
        [a] if a == "-V" || a == "--version" => {
            println!("{}", vega_build::version_line!());
            Ok(())
        }
        [a] if a == "--skill" => {
            print!("{SKILL}");
            Ok(())
        }
        [] => home(),
        _ => change(&args),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(f) => {
            print_failure(&f);
            ExitCode::from(if f.code == "usage" { 2 } else { 1 })
        }
    }
}

fn home() -> Result<(), Failure> {
    let file = ledger::file()?;
    let now = capacity::now();
    let health = std::thread::spawn(health::read);
    let sample = capacity::read();
    let health = health.join().unwrap_or(health::Health {
        version: Err("token-burn check panicked".into()),
        service: health::Service::Unknown,
    });
    for line in view(sample, &resets::load(&file), &health, now) {
        println!("{line}");
    }
    Ok(())
}

fn change(args: &[String]) -> Result<(), Failure> {
    let file = ledger::file()?;
    // An unknown source checks no provider: the Slayer can still record.
    let known: Option<Vec<String>> = capacity::read().ok().map(|s| {
        capacity::providers(&s)
            .into_iter()
            .map(String::from)
            .collect()
    });
    for line in ledger::apply_at(&file, args, known.as_deref(), capacity::now())? {
        println!("{line}");
    }
    Ok(())
}

/// The home view: windows, ledger, health.
fn view(
    sample: Result<Sample, String>,
    ledger: &Result<Ledger, String>,
    health: &health::Health,
    now: i64,
) -> Vec<String> {
    let empty = Ledger::default();
    let mut out = Vec::new();
    let mut age = None;
    match sample {
        Err(why) => out.push(format!("capacity: unknown ({why})")),
        Ok(s) => {
            let rows = capacity::windows(&s, now);
            age = rows.iter().map(|w| w.headroom.age).min();
            if rows.is_empty() {
                out.push("capacity: unknown (token-burn has no sample)".into());
            } else {
                out.extend(table(rows, ledger.as_ref().unwrap_or(&empty), now));
            }
        }
    }
    match ledger {
        Ok(l) => out.extend(ledger::table(l, now)),
        Err(why) => {
            out.push(format!("resets: unreadable ({why})"));
            out.push("help: fix the JSON or remove the file; vega does not overwrite it".into());
        }
    }
    out.extend(health::lines(health, age));
    out
}

/// `capacity[n]{provider,window,scope,left,resets_in,runs_out,binding}:`;
/// the binding row carries the Slayer's cover and the `(soon)` mark.
fn table(rows: Vec<Window>, ledger: &Ledger, now: i64) -> Vec<String> {
    let mut out = vec![format!(
        "capacity[{}]{{provider,window,scope,left,resets_in,runs_out,binding}}:",
        rows.len()
    )];
    for w in rows {
        let h = if w.binding {
            w.headroom.covered_by(ledger, now)
        } else {
            w.headroom
        };
        let mut runs_out: Vec<String> = Vec::new();
        match h.out_in {
            Some(s) if w.binding && h.warns() => {
                runs_out.push(format!("in ~{} (soon)", duration(s)))
            }
            Some(s) => runs_out.push(format!("in ~{}", duration(s))),
            None => {}
        }
        runs_out.extend(h.cover.clone());
        out.push(format!(
            "  {},{},{},{}%,{},{},{}",
            h.provider,
            h.window,
            h.scope.text(),
            percent(h.left),
            h.resets_in.map(duration).unwrap_or_default(),
            vega_herdr::cell(&runs_out.join("; ")),
            if w.binding { "yes" } else { "" }
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use health::{Health, Service};

    /// Recorded on 2026-10-10 from `token-burn status --json` / `forecast --json`.
    const FIXTURE: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../crates/vega-herdr/tests/fixtures/token-burn"
    );

    fn recorded() -> Sample {
        let tb = capacity::token_burn::TokenBurn {
            bin: "/nonexistent/token-burn".into(),
            fixture: Some(FIXTURE.into()),
        };
        let scopes = capacity::scopes::Scopes::parse(capacity::scopes::DEFAULT).unwrap();
        capacity::read_from(&tb, &scopes).unwrap()
    }

    /// 2026-10-10T07:40:00Z, 15 s after the recording.
    const AT: i64 = 1_791_618_000;

    fn ok() -> Health {
        Health {
            version: Ok(Some("v0.1.37".into())),
            service: Service::Running,
        }
    }

    #[test]
    fn skill_has_valid_frontmatter() {
        vega_plugin::assert_guide(SKILL, "capacity");
    }

    #[test]
    fn home_view_lists_every_window_ledger_and_health() {
        let mut l = Ledger::default();
        l.set_owned("claude", 1);
        let v = view(Ok(recorded()), &Ok(l), &ok(), AT);
        assert_eq!(
            v,
            [
                "capacity[14]{provider,window,scope,left,resets_in,runs_out,binding}:",
                "  antigravity,claude_and_gpt,account,100%,4h57m,,yes",
                "  antigravity,claude_and_gpt_weekly,account,100%,6d23h,,",
                "  antigravity,gemini,account,100%,4h57m,,",
                "  antigravity,gemini_weekly,account,100%,6d9h,,",
                "  claude,five_hour,account,86%,3h59m,,",
                "  claude,seven_day,account,75%,5d17h,in ~3d8h,yes",
                "  claude,seven_day_fable,model:fable,96%,5d17h,,",
                "  codex,primary,account,80%,3d22h,,yes",
                "  copilot,chat,account,100%,21d16h,,",
                "  copilot,completions,account,100%,21d16h,,",
                "  copilot,premium_interactions,account,99.7%,21d16h,,yes",
                "  xai,weekly,account,97%,4d6h,,yes",
                "  zai,5h,account,100%,,,",
                "  zai,weekly,account,100%,2d3h,,yes",
                "resets[1]{provider,owned,plan,override_until,last_used}:",
                "  claude,1,,,",
                "token_burn: ok (v0.1.37, service running, sample 0m old)",
            ]
        );
    }

    #[test]
    fn a_planned_reset_shows_on_the_binding_row_only() {
        let mut l = Ledger::default();
        l.set_owned("claude", 1);
        l.plan("claude", "2026-10-11").unwrap();
        let v = view(Ok(recorded()), &Ok(l), &ok(), AT);
        assert_eq!(
            v[6],
            "  claude,seven_day,account,75%,5d17h,in ~3d8h; reset planned this week,yes"
        );
        assert_eq!(v[5], "  claude,five_hour,account,86%,3h59m,,");
    }

    #[test]
    fn unknown_sample_and_unreadable_ledger_still_show_health() {
        let missing = Health {
            version: Err("token-burn not found".into()),
            service: Service::Unknown,
        };
        let v = view(
            Err("token-burn not found".into()),
            &Err("bad json".into()),
            &missing,
            AT,
        );
        assert_eq!(v[0], "capacity: unknown (token-burn not found)");
        assert_eq!(v[1], "resets: unreadable (bad json)");
        assert_eq!(v[3], "token_burn: missing (token-burn not found)");
    }
}
