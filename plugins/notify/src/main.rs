use serde_json::{json, Value};
use std::{collections::BTreeMap, env, io::Read, process::ExitCode, time::Duration};
use vega_herdr::{
    asks, cell,
    notes::{self, Note, Store},
    Failure,
};
const HELP: &str = include_str!("../help/notify.md");
const RAISE_HELP: &str = include_str!("../help/raise.md");
const RESOLVE_HELP: &str = include_str!("../help/resolve.md");
const ACK_HELP: &str = include_str!("../help/ack.md");
const LIST_HELP: &str = include_str!("../help/list.md");
const TEST_HELP: &str = include_str!("../help/test.md");
const SKILL: &str = include_str!("../SKILL.md");
const TIMEOUT: Duration = Duration::from_secs(4);
fn usage(message: impl Into<String>) -> Failure {
    Failure::new("usage", message, "vega notify --help")
}
fn io_error(e: std::io::Error) -> Failure {
    Failure::new(
        "state_failed",
        e.to_string(),
        "check the vega notes state directory",
    )
}
fn help(cmd: &str) -> &'static str {
    match cmd {
        "info" | "action" | "urgent" => RAISE_HELP,
        "resolve" => RESOLVE_HELP,
        "ack" => ACK_HELP,
        "list" => LIST_HELP,
        "test" => TEST_HELP,
        _ => HELP,
    }
}
fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    if args.iter().any(|a| a == "--help" || a == "-h") {
        print!("{}", help(args.first().map(String::as_str).unwrap_or("")));
        return ExitCode::SUCCESS;
    }
    let result = match args.as_slice() {
        [a] if ["--version", "-V", "-v"].contains(&a.as_str()) => {
            println!("{}", vega_build::version_line!());
            Ok(())
        }
        [a] if a == "--skill" => {
            print!("{SKILL}");
            Ok(())
        }
        [a, b, _from] if a == "remote-update" && b == "--from" => remote_update(),
        [] => {
            let bin = env::current_exe()
                .map(|p| vega_herdr::collapse_home(&p.to_string_lossy()))
                .unwrap_or_else(|_| "vega-notify".into());
            println!("bin: {}", cell(&bin));
            println!("description: Raise and settle blocking asks for the operator");
            run(Cmd::List)
        }
        _ => parse(&args).and_then(run),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            vega_herdr::print_failure(&e);
            ExitCode::from(if e.code == "usage" { 2 } else { 1 })
        }
    }
}
#[derive(Debug, PartialEq)]
enum Cmd {
    List,
    Raise {
        level: String,
        line: String,
        flags: BTreeMap<String, String>,
    },
    Resolve(Selector),
    Ack(Selector),
    Test(String),
}
#[derive(Debug, PartialEq)]
enum Selector {
    Id(String, u64),
    Key(String),
    Pane(String),
    Default,
}
fn selector_id(s: &str) -> Result<Selector, Failure> {
    let (host, id) = s
        .split_once('=')
        .ok_or_else(|| usage("ask id must be <host>=<id>, as printed by vega notify"))?;
    if host.is_empty() || vega_herdr::state::host_token(host) != host {
        return Err(usage("invalid ask host"));
    }
    Ok(Selector::Id(
        host.into(),
        id.parse().map_err(|_| usage("invalid ask id"))?,
    ))
}
fn parse(args: &[String]) -> Result<Cmd, Failure> {
    let cmd = args.first().map(String::as_str).unwrap_or("list");
    match cmd {
        "list" if args.len() <= 1 => Ok(Cmd::List),
        "info" | "action" | "urgent" => {
            let line = args.get(1).filter(|s| !s.starts_with('-')).ok_or_else(|| {
                usage(
                    "a one-line ask is required; valid flags: --bead --cmd --key --pane --project",
                )
            })?;
            if line.is_empty()
                || line.chars().count() > 120
                || line.chars().any(char::is_control)
                || !line.contains(": ")
                || !line.contains(" -> ")
            {
                return Err(usage(
                    "line must be at most 120 characters: <project>: <what> -> <pane>",
                ));
            }
            let mut flags = BTreeMap::new();
            let mut i = 2;
            while i < args.len() {
                let flag = &args[i];
                if !["--bead", "--cmd", "--key", "--pane", "--project"].contains(&flag.as_str()) {
                    return Err(usage(format!(
                        "unknown argument {flag}; valid flags: --bead --cmd --key --pane --project"
                    )));
                }
                let value = args
                    .get(i + 1)
                    .filter(|s| !s.is_empty() && !s.starts_with("--"))
                    .ok_or_else(|| usage(format!("{flag} requires a value")))?;
                if flag != "--cmd" && value.chars().any(char::is_control) {
                    return Err(usage(format!("{flag} contains controls")));
                }
                if flags.insert(flag.clone(), value.clone()).is_some() {
                    return Err(usage(format!("duplicate {flag}")));
                }
                i += 2;
            }
            Ok(Cmd::Raise {
                level: cmd.into(),
                line: line.clone(),
                flags,
            })
        }
        "resolve" => match &args[1..] {
            [] => Ok(Cmd::Resolve(Selector::Default)),
            [id] if !id.starts_with('-') => Ok(Cmd::Resolve(selector_id(id)?)),
            [flag, value] if !value.is_empty() && !value.starts_with('-') => match flag.as_str() {
                "--key" if !value.chars().any(char::is_control) => {
                    Ok(Cmd::Resolve(Selector::Key(value.clone())))
                }
                "--pane" if !value.chars().any(char::is_control) => {
                    Ok(Cmd::Resolve(Selector::Pane(value.clone())))
                }
                _ => Err(usage(format!(
                    "unknown flag {flag}; valid flags: --key --pane"
                ))),
            },
            _ => Err(usage(
                "resolve accepts <host>=<id>, --key <key>, or --pane <pane>",
            )),
        },
        "ack" if args.len() == 2 => Ok(Cmd::Ack(selector_id(&args[1])?)),
        "test"
            if args.len() == 2
                && ["toast", "macos", "ntfy", "pushover", "moshi"].contains(&args[1].as_str()) =>
        {
            Ok(Cmd::Test(args[1].clone()))
        }
        _ => Err(usage(format!("invalid {cmd} arguments; {}", help(cmd)))),
    }
}
fn store() -> Result<Store, Failure> {
    Store::local().ok_or_else(|| {
        Failure::new(
            "no_state_dir",
            "neither XDG_STATE_HOME nor HOME is set",
            "set HOME",
        )
    })
}
fn context() -> Result<(String, String), Failure> {
    let pane = vega_herdr::me()?.pane;
    let cwd = env::current_dir().map_err(io_error)?;
    let project = vega_herdr::project::resolve(&cwd, &vega_herdr::state::post_host())?;
    Ok((pane, project.id.to_string()))
}
fn matches(s: &Selector, n: &Note) -> bool {
    match s {
        Selector::Id(host, id) => &n.host == host && n.id == *id,
        Selector::Key(k) => n.key.as_ref() == Some(k),
        Selector::Pane(p) => n.pane.as_ref() == Some(p),
        Selector::Default => false,
    }
}
fn gather(store: &Store, host: &str) -> Vec<Note> {
    let remote = match vega_registry::read_notes_blocking(TIMEOUT) {
        Ok(n) => n,
        Err(e) => {
            println!("registry: off ({}); this host's asks only", cell(&e));
            Vec::new()
        }
    };
    asks::open(store.list(), remote, host)
}
fn run(cmd: Cmd) -> Result<(), Failure> {
    if let Cmd::Test(channel) = cmd {
        return Err(Failure::new(
            "channel_unavailable",
            format!("{channel} delivery is not implemented yet"),
            "vega notify list; channel support lands in the delivery beads",
        ));
    }
    let store = store()?;
    let host = vega_herdr::state::post_host();
    let now_ms = notes::now_ms();
    let ack = matches!(cmd, Cmd::Ack(_));
    match cmd {
        Cmd::Raise {
            level,
            line,
            mut flags,
        } => {
            let (pane, project) = if flags.contains_key("--pane") && flags.contains_key("--project")
            {
                (
                    flags.remove("--pane").unwrap(),
                    flags.remove("--project").unwrap(),
                )
            } else {
                // Resolve only missing context: explicit pane works outside herdr.
                let pane = match flags.remove("--pane") {
                    Some(p) => p,
                    None => vega_herdr::me()?.pane,
                };
                let project = match flags.remove("--project") {
                    Some(p) => p,
                    None => {
                        vega_herdr::project::resolve(&env::current_dir().map_err(io_error)?, &host)?
                            .id
                            .to_string()
                    }
                };
                (pane, project)
            };
            let key = flags
                .remove("--key")
                .unwrap_or_else(|| format!("{project}/{pane}"));
            let note = Note {
                host,
                kind: "ask".into(),
                level: Some(level),
                cause: Some(format!("ask:{key}")),
                key: Some(key),
                pane: Some(pane),
                project: Some(project),
                bead: flags.remove("--bead"),
                cmd: flags.remove("--cmd").map(|s| strip_controls(&s)),
                title: line,
                ..Default::default()
            };
            let (n, new, up) = asks::raise(&store, note, now_ms).map_err(io_error)?;
            println!(
                "ask: {}={} ({}) {}; delivery pending",
                n.host,
                n.id,
                n.level.as_deref().unwrap_or("info"),
                if new {
                    "open"
                } else if up {
                    "level increased"
                } else {
                    "updated"
                }
            );
            Ok(())
        }
        Cmd::List => {
            print_list(&gather(&store, &host), now_ms);
            Ok(())
        }
        Cmd::Resolve(mut sel) | Cmd::Ack(mut sel) => {
            if sel == Selector::Default {
                let (pane, project) = context()?;
                sel = Selector::Key(format!("{project}/{pane}"));
            }
            // A key/pane selector is origin-local; qualified ids address another host.
            if let Selector::Id(ref origin, id) = sel {
                if origin != &host {
                    return remote_change(origin, id, ack);
                }
            }
            let count = asks::change(&store, &host, (now_ms / 1000) as i64, ack, |n| {
                matches(&sel, n)
            })
            .map_err(io_error)?;
            if count == 0 {
                println!(
                    "asks: 0 changed; no open un{} ask for {}",
                    if ack { "acknowledged" } else { "resolved" },
                    label(&sel)
                );
            } else {
                println!(
                    "asks: {count} {}",
                    if ack {
                        "acknowledged; kept open"
                    } else {
                        "resolved"
                    }
                );
            }
            Ok(())
        }
        Cmd::Test(_) => unreachable!(),
    }
}
fn label(s: &Selector) -> String {
    match s {
        Selector::Id(h, id) => format!("{h}={id}"),
        Selector::Key(k) | Selector::Pane(k) => k.clone(),
        Selector::Default => String::new(),
    }
}
fn print_list(notes: &[Note], now_ms: u64) {
    if notes.is_empty() {
        println!("asks: 0 open asks");
        return;
    }
    println!(
        "asks[{}]{{id,level,age,project,pane,line,next}}:",
        notes.len()
    );
    for n in notes {
        println!(
            "  {},{},{},{},{},{},{}",
            cell(&format!("{}={}", n.host, n.id)),
            cell(n.level.as_deref().unwrap_or("info")),
            cell(&format!("{}s", now_ms.saturating_sub(n.id) / 1000)),
            cell(n.project.as_deref().unwrap_or("")),
            cell(n.pane.as_deref().unwrap_or("")),
            cell(&n.title),
            cell(
                n.cmd
                    .as_deref()
                    .or(n.bead.as_deref())
                    .unwrap_or("answer in pane")
            )
        );
    }
    println!("help: vega notify resolve <host>=<id>; vega notify ack <host>=<id> keeps it open");
}
/// Strip CSI/OSC terminal escapes as well as individual controls.
fn strip_controls(s: &str) -> String {
    let mut out = String::new();
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            match chars.next() {
                Some('[') => {
                    for x in chars.by_ref() {
                        if ('@'..='~').contains(&x) {
                            break;
                        }
                    }
                }
                Some(']') => {
                    while let Some(x) = chars.next() {
                        if x == '\u{7}' || (x == '\u{1b}' && chars.peek() == Some(&'\\')) {
                            if x == '\u{1b}' {
                                chars.next();
                            }
                            break;
                        }
                    }
                }
                _ => {}
            }
        } else if !c.is_control() {
            out.push(c);
        }
    }
    out
}
fn remote_change(host: &str, id: u64, ack: bool) -> Result<(), Failure> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(io_error)?;
    let result: Result<String, String> = rt.block_on(async {
        tokio::time::timeout(TIMEOUT, async {
            let client = vega_registry::connect("notify").await?;
            let subject = format!("vega.cmd.{host}.notify.{}", vega_herdr::state::post_host());
            let payload = json!({"id":id,"ack":ack}).to_string().into_bytes();
            let reply = client
                .request(subject, payload.into())
                .await
                .map_err(|e| e.to_string())?;
            let v: Value = serde_json::from_slice(&reply.payload).map_err(|e| e.to_string())?;
            if v["ok"] != true {
                return Err(v["message"]
                    .as_str()
                    .unwrap_or("origin refused update")
                    .to_string());
            }
            Ok(v["message"].as_str().unwrap_or("updated").to_string())
        })
        .await
        .map_err(|_| "origin update timed out".to_string())?
    });
    let message = result.map_err(|e| {
        Failure::new(
            "origin_unavailable",
            e,
            "retry when the origin vegad is running; vega notify list",
        )
    })?;
    println!("ask: {}", cell(&message));
    Ok(())
}
fn remote_update() -> Result<(), Failure> {
    let mut input = Vec::new();
    std::io::stdin()
        .take(4097)
        .read_to_end(&mut input)
        .map_err(io_error)?;
    if input.len() > 4096 {
        return Err(usage("request too large"));
    }
    let v: Value = serde_json::from_slice(&input).map_err(|_| usage("invalid request JSON"))?;
    let id = v["id"]
        .as_u64()
        .ok_or_else(|| usage("request requires id"))?;
    let ack = v["ack"]
        .as_bool()
        .ok_or_else(|| usage("request requires ack boolean"))?;
    if v.as_object().is_none_or(|o| o.len() != 2) {
        return Err(usage("request accepts only id and ack"));
    }
    let host = vega_herdr::state::post_host();
    let count = asks::change(
        &store()?,
        &host,
        (notes::now_ms() / 1000) as i64,
        ack,
        |n| n.id == id,
    )
    .map_err(io_error)?;
    println!(
        "{host}={id}: {count} {}",
        if ack { "acknowledged" } else { "resolved" }
    );
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    fn p(s: &[&str]) -> Result<Cmd, Failure> {
        parse(&s.iter().map(|s| s.to_string()).collect::<Vec<_>>())
    }
    #[test]
    fn guide() {
        vega_plugin::assert_guide(SKILL, "notify");
    }
    #[test]
    fn reject_bad_input_before_state_or_context() {
        for args in [
            &["list", "--typo"][..],
            &["action", "hello"],
            &["action", "p: go -> w1:p1", "--typo", "x"],
            &["ack", "5"],
            &["resolve", "--key"],
            &["test", "unknown"],
        ] {
            assert!(p(args).is_err(), "{args:?}");
        }
        assert!(p(&[
            "action",
            "p: go -> w1:p1",
            "--pane",
            "w1:p1",
            "--project",
            "p"
        ])
        .is_ok());
        assert_eq!(p(&[]).unwrap(), Cmd::List);
        assert_eq!(
            p(&["resolve", "a=5"]).unwrap(),
            Cmd::Resolve(Selector::Id("a".into(), 5))
        );
    }
    #[test]
    fn terminal_controls() {
        assert_eq!(
            strip_controls("a\u{1b}[31mred\u{1b}[0m\n\u{1b}]0;title\u{7}b"),
            "aredb"
        );
    }
}
