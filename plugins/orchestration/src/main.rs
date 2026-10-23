mod home;

use std::{collections::BTreeMap, env, process::ExitCode};
use vega_herdr::{
    cell, print_failure,
    waits::{Change, Request},
    Failure,
};
use vega_orchestration::update;
const HELP: &str = include_str!("../help/orchestration.md");
const WAITING_HELP: &str = include_str!("../help/waiting.md");
const CLEAR_HELP: &str = include_str!("../help/clear.md");
const SKILL: &str = include_str!("../SKILL.md");

fn usage(message: &str, help: &str) -> Failure {
    Failure::new("usage", message, help)
}
fn options(
    args: &[String],
    allowed: &[&str],
    help: &str,
) -> Result<BTreeMap<String, String>, Failure> {
    let mut out = BTreeMap::new();
    let mut i = 0;
    while i < args.len() {
        let flag = &args[i];
        if !allowed.contains(&flag.as_str()) {
            return Err(usage(&format!("unknown flag or argument {flag}"), help));
        }
        let value = args
            .get(i + 1)
            .filter(|v| !v.starts_with('-') && !v.trim().is_empty())
            .ok_or_else(|| usage(&format!("missing value for {flag}"), help))?;
        if out.insert(flag.clone(), value.clone()).is_some() {
            return Err(usage(&format!("duplicate flag {flag}"), help));
        }
        i += 2;
    }
    Ok(out)
}
fn required<'a>(
    opts: &'a BTreeMap<String, String>,
    name: &str,
    help: &str,
) -> Result<&'a str, Failure> {
    opts.get(name)
        .map(String::as_str)
        .ok_or_else(|| usage(&format!("{name} is required"), help))
}
fn waiting(args: &[String]) -> Result<(), Failure> {
    let help="valid flags: --on <owner>, --for <reason>, --bead <id>, --thread <id>, --coordinator <pane@host>, --help; vega orchestration waiting --help";
    let opts = options(
        args,
        &["--on", "--for", "--bead", "--thread", "--coordinator"],
        help,
    )?;
    let on = required(&opts, "--on", help)?;
    let reason = required(&opts, "--for", help)?;
    update::validate_owner_shape(on)?;
    let actor = update::caller()?;
    update::validate_on(on, &actor)?;
    let coordinator = update::coordinator(&actor, opts.get("--coordinator").map(String::as_str))?;
    let to = coordinator.host.clone();
    let req = Request {
        version: 1,
        actor,
        change: Change::Waiting {
            coordinator,
            on: on.into(),
            reason: reason.into(),
            bead: opts.get("--bead").cloned(),
            thread: opts.get("--thread").cloned(),
        },
    };
    let receipt = update::submit(&to, &req)?;
    println!("wait: {}", cell(&receipt.id));
    println!("status: {}", cell(&receipt.status));
    println!(
        "help: vega orchestration clear {} --evidence \"<inspected resolution>\"",
        receipt.id
    );
    Ok(())
}
fn clear(id: &str, args: &[String]) -> Result<(), Failure> {
    let help = "valid flags: --evidence <text>, --help; vega orchestration clear --help";
    let opts = options(args, &["--evidence"], help)?;
    let evidence = required(&opts, "--evidence", help)?;
    let to = vega_herdr::waits::id_host(id)
        .ok_or_else(|| usage("invalid wait ID; use the ID returned by waiting", help))?;
    let req = Request {
        version: 1,
        actor: update::caller()?,
        change: Change::Clear {
            id: id.into(),
            evidence: evidence.into(),
        },
    };
    let receipt = update::submit(to, &req)?;
    println!("wait: {}", cell(&receipt.id));
    println!("status: {}", cell(&receipt.status));
    Ok(())
}
fn run(args: &[String]) -> Result<(), Failure> {
    match args {
        [a] if matches!(a.as_str(), "--help" | "-h") => { print!("{HELP}"); Ok(()) }
        [a] if matches!(a.as_str(), "--version" | "-V" | "-v") => { println!("{}", vega_build::version_line!()); Ok(()) }
        [a] if a=="--skill" => { print!("{SKILL}"); Ok(()) }
        [] => home::run(None),
        [a,parent] if a=="--parent" && !parent.starts_with('-') => home::run(Some(parent)),
        [cmd,h] if cmd=="waiting" && matches!(h.as_str(),"--help"|"-h") => {print!("{WAITING_HELP}"); Ok(())},
        [cmd,h] if cmd=="clear" && matches!(h.as_str(),"--help"|"-h") => {print!("{CLEAR_HELP}"); Ok(())},
        [cmd,rest @ ..] if cmd=="waiting" => waiting(rest),
        [cmd,id,rest @ ..] if cmd=="clear" && !id.starts_with('-') => clear(id,rest),
        [cmd,flag,from] if cmd=="remote-update" && flag=="--from" && !from.is_empty() && from==&vega_herdr::hosts::key(from) => update::remote(from),
        _=>Err(usage("unknown command, flag or missing value","valid commands: waiting, clear; home flags: --parent <bead>, --help; vega orchestration --help")),
    }
}
fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            print_failure(&e);
            ExitCode::from(if e.code == "usage" { 2 } else { 1 })
        }
    }
}
#[cfg(test)]
mod tests {
    #[test]
    fn guide() {
        vega_plugin::assert_guide(super::SKILL, "orchestration");
    }
}
