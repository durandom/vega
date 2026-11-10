//! Fake herdr for `reopen::tests`, installed as `herdr` on the test's PATH.
//!
//! Behaviour is driven by the `VEGA_TEST_*` variables the test sets; state lives under
//! `$XDG_STATE_HOME` (the `running` file is the one live agent). Replaces the Perl fixture of
//! 7946286: the test makes ~260 herdr calls, and a native binary starts faster than any
//! interpreter and needs no JSON module on the host.

use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::{env, fs, process};

fn var(name: &str) -> String {
    env::var(name).unwrap_or_default()
}

fn die(message: &str) -> ! {
    eprintln!("{message}");
    process::exit(255)
}

fn emit(result: Value) -> ! {
    println!("{}", json!({ "result": result }));
    process::exit(0)
}

fn fail(code: &str) -> ! {
    println!(
        "{}",
        json!({ "error": { "code": code, "message": "fixture failure" } })
    );
    process::exit(1)
}

fn read_json(path: &Path) -> Value {
    let text = fs::read_to_string(path).unwrap_or_else(|e| die(&format!("read {path:?}: {e}")));
    serde_json::from_str(&text).unwrap_or_else(|e| die(&format!("parse {path:?}: {e}")))
}

fn write(path: &Path, text: &str) {
    fs::write(path, text).unwrap_or_else(|e| die(&format!("write {path:?}: {e}")));
}

fn key_of(pane: &str) -> String {
    format!("local_{}", pane.replace(':', "_"))
}

fn arg_after<'a>(args: &'a [String], flag: &str) -> &'a str {
    args.windows(2)
        .find(|w| w[0] == flag)
        .map(|w| w[1].as_str())
        .unwrap_or_else(|| die(&format!("no {flag} in {args:?}")))
}

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    let arg = |i: usize| args.get(i).map(String::as_str).unwrap_or("");
    let root = PathBuf::from(var("XDG_STATE_HOME"));
    let kind = var("VEGA_TEST_KIND");
    let live = root.join("running");
    let mut agent = live.exists().then(|| read_json(&live));
    let is_agent = |agent: &Option<Value>, id: &str, fields: &[&str]| {
        agent
            .as_ref()
            .is_some_and(|a| fields.iter().any(|f| a[f].as_str() == Some(id)))
    };

    match format!("{} {}", arg(0), arg(1)).as_str() {
        "agent get" => {
            if !is_agent(&agent, arg(2), &["name", "pane_id"]) {
                fail("agent_not_found");
            }
            emit(json!({ "agent": agent }));
        }
        "agent list" => emit(json!({ "agents": agent.into_iter().collect::<Vec<_>>() })),
        "pane get" => {
            if !is_agent(&agent, arg(2), &["pane_id"]) {
                fail("pane_not_found");
            }
            if var("VEGA_TEST_RESUME") == "0"
                && var("VEGA_TEST_DIALOG") == "0"
                && !root.join("prompted").exists()
            {
                if let Some(a) = agent.as_mut().and_then(Value::as_object_mut) {
                    a.remove("agent_session");
                }
            }
            emit(json!({ "pane": agent }));
        }
        "pane list" => emit(
            json!({ "panes": [{ "pane_id": "w0:p1", "workspace_id": "w0", "agent": "codex" }] }),
        ),
        "pane process-info" => {
            // The caller's pane runs the test. A spawned pane has no shell: spawn renices its
            // shell, and reniced, the test and all its fixture calls would crawl on a loaded host.
            let pid: u64 = if arg(3) == "w0:p1" {
                var("VEGA_TEST_PID").parse().unwrap_or(0)
            } else {
                0
            };
            emit(json!({ "process_info": { "shell_pid": pid } }));
        }
        "pane read" => {
            println!("Ready");
        }
        "workspace get" => emit(json!({ "workspace": {} })),
        "pane close" => {
            let key = key_of(arg(2));
            if root.join("vega/parents").join(&key).exists() {
                die(&format!("parent of {} left", arg(2)));
            }
            if root.join("vega/modes").join(&key).exists() {
                die(&format!("mode of {} left", arg(2)));
            }
            let _ = fs::remove_file(&live);
            emit(json!({}));
        }
        "agent prompt" => {
            write(&root.join("prompted"), "");
            emit(json!({}));
        }
        "agent wait" => emit(json!({})),
        "workspace create" => {
            let cwd = fs::canonicalize(arg_after(&args, "--cwd")).ok();
            if cwd.is_none() || cwd != fs::canonicalize(&root).ok() {
                die("workspace cwd is not the test dir");
            }
            let pane = var("VEGA_TEST_NEXT_PANE");
            let ws = pane.split(':').next().unwrap_or("");
            emit(json!({ "workspace": { "workspace_id": ws }, "root_pane": { "pane_id": pane } }));
        }
        "agent start" => {
            let pane = arg_after(&args, "--pane");
            let key = key_of(pane);
            let expected: Value =
                serde_json::from_str(&var("VEGA_TEST_PARENT")).unwrap_or_default();
            let parent_file = root.join("vega/parents").join(&key);
            if expected.is_object() {
                let parent = read_json(&parent_file);
                if parent["parent"] != expected["pane"] || parent["host"] != expected["host"] {
                    die("wrong parent");
                }
            } else if parent_file.exists() {
                die("unexpected parent");
            }
            let mode = read_json(&root.join("vega/modes").join(&key));
            if mode["mode"].as_str() != Some(var("VEGA_TEST_MODE").as_str()) {
                die(&format!("wrong mode {}", mode["mode"]));
            }
            let dashes = args
                .iter()
                .position(|a| a == "--")
                .unwrap_or(args.len() - 1);
            let tail = &args[dashes + 1..];
            if var("VEGA_TEST_RESUME") == "1" {
                let flag = if kind == "codex" {
                    "resume"
                } else {
                    "--resume"
                };
                let ok = tail.len() >= 2
                    && tail[tail.len() - 2] == flag
                    && tail[tail.len() - 1] == "saved-session";
                if !ok {
                    die(&format!("no resume in {tail:?}"));
                }
            } else if tail.iter().any(|a| a == "resume" || a == "--resume") {
                die(&format!("unexpected resume in {tail:?}"));
            }
            let ws = pane.split(':').next().unwrap_or("");
            write(
                &live,
                &json!({
                    "name": "fix", "pane_id": pane, "workspace_id": ws, "agent": kind,
                    "agent_session": { "value": "saved-session" },
                })
                .to_string(),
            );
            if var("VEGA_TEST_DIALOG") == "1" {
                fail("timeout");
            }
            emit(json!({}));
        }
        _ => die(&format!("unexpected call: {args:?}")),
    }
}
