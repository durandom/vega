//! hc owns terminal creation and input; vega retains launch policy and lineage.
use super::*;
use vega_herdr::hc::{hc_local, own_terminal, Terminal};
use vega_herdr::state::SpawnRecord;

#[allow(clippy::too_many_arguments)]
pub fn spawn(
    name: &str,
    kind: &str,
    cwd: Option<String>,
    place: Place,
    prompt: Option<&str>,
    user: &[String],
    no_parent: bool,
    mode: Mode,
    extra: &[String],
    tier: Option<&str>,
    old: Option<&SpawnRecord>,
) -> Result<String, Failure> {
    if !valid_agent_name(name) {
        return Err(Failure::new(
            "invalid_agent_name",
            "invalid agent name",
            "use [a-z][a-z0-9-], at most 24 characters",
        ));
    }
    if machine().is_some() && cwd.is_none() {
        return Err(Failure::new(
            "usage",
            "--cwd is required with --machine",
            "pass an absolute directory on that machine",
        ));
    }
    if matches!(place, Place::At(At::Split | At::Workspace)) && old.is_none() {
        return Err(Failure::new(
            "usage",
            "hc spawns open a tab in the caller's space",
            "use --at tab, or --worktree <branch>",
        ));
    }
    if let Some(m) = machine() {
        vega_herdr::hc::check_direction(m)?;
        let list = vega_herdr::hc::machines()?;
        if !list
            .as_array()
            .into_iter()
            .flatten()
            .any(|v| v["label"] == m)
        {
            return Err(Failure::new(
                "unknown_machine",
                "no saved machine matches --machine",
                "inspect the saved machine inventory",
            ));
        }
    }
    match backend::info(name) {
        Ok(v) if v["state"] == "live" => {
            return Err(Failure::new(
                "name_taken",
                format!("{name} is already running"),
                "choose another name or send to the running agent",
            ))
        }
        Ok(_) => (),
        Err(f) if f.code == "pane_not_found" => (),
        Err(f) => return Err(f),
    }
    let own = own_terminal().ok();
    let mut cwd =
        cwd.unwrap_or_else(|| env::current_dir().unwrap_or_default().display().to_string());
    if machine().is_none() {
        cwd = expand_home(&cwd, &env::var("HOME").unwrap_or_default());
    }
    let selection = tier::select(kind, tier, extra, Some(&cwd))?;
    // Space IDs belong to the local app; they do not identify remote spaces.
    let mut space = if let Some(Terminal::Hc(p)) = &own.as_ref().filter(|_| machine().is_none()) {
        hc_local(&["get", p])?["labels"]["herdr.space"]
            .as_str()
            .map(String::from)
    } else {
        None
    };
    let remote_prepared = if let Some(target) = machine() {
        let prepared = prepare_remote(target, kind, &cwd, &place)?;
        if matches!(place, Place::Worktree { .. }) {
            cwd = prepared["cwd"]
                .as_str()
                .filter(|s| Path::new(s).is_absolute())
                .ok_or_else(|| {
                    Failure::new(
                        "remote_prepare_failed",
                        "target returned no worktree path",
                        "deploy matching vega binaries to the target",
                    )
                })?
                .into();
        }
        Some(prepared)
    } else {
        None
    };
    let mut placeholder = None;
    let mut tab_fallback = false;
    if let Place::Worktree { branch, base } = place {
        let (path, how) = if let Some(prepared) = &remote_prepared {
            (
                cwd.clone(),
                prepared["how"]
                    .as_str()
                    .unwrap_or("prepared remotely")
                    .into(),
            )
        } else {
            worktree::hc_worktree(&cwd, &branch, base.as_deref())?
        };
        cwd = path;
        println!("worktree: {} ({how})", cell(&branch));
        // Remote IDs cannot be used in the local app. Headless local hosts use
        // exactly the ordinary tab path, retaining the caller's space label.
        let shown = if machine().is_some() {
            Err(Failure::new("app_unavailable", "remote spawn", "use a tab"))
        } else {
            local_shown_space()
        };
        match shown {
            Err(f) if f.code == "app_unavailable" => {
                tab_fallback = true;
            }
            shown => {
                if let Err(f) = &shown {
                    eprintln!("warning: cannot read the shown herdr-app space ({}); the new space may take the screen", f.message);
                }
                let v = app(&["do", "space.new", "--name", &branch, "--folder", &cwd])?;
                placeholder = v["terminal"].as_str().map(String::from);
                space = v["space"].as_str().or(v["id"].as_str()).map(String::from);
                if space.is_none() {
                    return Err(Failure::new(
                        "app_bad_output",
                        "new space has no id",
                        "inspect herdr-app ls before retrying",
                    ));
                }
                if let Some(prev) = shown.ok().flatten().filter(|p| Some(p) != space.as_ref()) {
                    // The space exists either way; a failed reselect only leaves it shown.
                    if let Err(f) = app(&["do", "space.select", "--id", &prev]) {
                        eprintln!(
                            "warning: cannot show space {prev} again ({}); the new space is shown",
                            f.message
                        );
                    }
                }
            }
        }
    }

    if machine().is_none() && !no_parent {
        vega_herdr::project::resolve_optional(Path::new(&cwd), &vega_herdr::state::post_host())?;
    }
    let trust_line = if let Some(prepared) = remote_prepared {
        prepared["trust"].as_str().map(String::from)
    } else {
        trust::apply(kind, &cwd)
    };
    if let Some(line) = trust_line {
        println!("{line}");
    }
    let actor = beads_actor(&cwd);
    let socket = env::var("SSH_AUTH_SOCK")
        .ok()
        .filter(|_| machine().is_none());
    let vars = spawn_env(kind, actor.as_deref(), socket.as_deref(), user);
    let mut labels = serde_json::json!({
        "vega.kind":kind, "vega.harness":kind, "vega.mode":mode.as_str(), "vega.role":"sentinel"
    });
    if let Some(s) = space {
        labels["herdr.space"] = s.into();
    }
    let mut launch_env = serde_json::Map::new();
    for k in ["TERM", "TERM_PROGRAM", "TERM_PROGRAM_VERSION", "COLORTERM"] {
        if let Ok(v) = env::var(k) {
            launch_env.insert(k.into(), v.into());
        }
    }
    // hc can set but not unset: mask inherited TUI identity; consumers must ignore empty values.
    launch_env.insert("HERDR_PANE_ID".into(), "".into());
    launch_env.insert("HERDR_SOCKET_PATH".into(), "".into());
    for var in vars {
        if let Some((k, v)) = var.split_once('=') {
            launch_env.insert(k.into(), v.into());
        }
    }
    let mut args = vec![kind.to_string()];
    let mut agent_args = spawn_args(kind, &[("SSH_AUTH_SOCK", socket.as_deref())], user, &[]);
    if kind == "codex" {
        // The child ID/socket are created by hc, not the parent's environment.
        agent_args.extend([
            "-c".into(),
            "shell_environment_policy.inherit=\"all\"".into(),
        ]);
    }
    agent_args.extend(selection.args.clone());
    args.extend(agent_args);
    // An in-place resume after a reboot repeats this launch (resume.rs).
    let launch_argv = args.clone();
    // Only what vega set: explicit --env values are never stored (reopen.md).
    let resume_env: Vec<String> = launch_env
        .iter()
        .filter(|(k, _)| !k.starts_with("TERM") && *k != "COLORTERM")
        .filter_map(|(k, v)| v.as_str().map(|v| format!("{k}={v}")))
        .filter(|kv| !user.contains(kv))
        .collect();
    if let Some(p) = prompt {
        args.push(if no_parent && own.is_none() {
            p.into()
        } else {
            backend::body(p, handoff(no_parent, mode).0, machine().is_some())?
        });
    }
    let v = vega_herdr::hc::rpc(
        "terminal.run",
        serde_json::json!({
            "argv":args, "env":launch_env, "labels":labels, "cwd":cwd,
            "name":name, "on":machine(), "outlive_parent":true, "die_with_parent":false,
            "no_parent":no_parent || !matches!(own, Some(Terminal::Hc(_))),
            "caller":own.as_ref().and_then(|t| match t { Terminal::Hc(p) => Some(p), _ => None }),
            "wait":false,
        }),
        std::time::Duration::from_secs(30),
    )?;
    // Keep the harness as argv[0]. A nice/env wrapper hides its identity.
    if machine().is_none() {
        if let Some(pid) = v["pid"].as_u64().filter(|p| *p > 1) {
            let _ = Command::new("renice")
                .args([AGENT_NICE, "-p", &pid.to_string()])
                .output();
        }
    }
    let pane = v["address"]
        .as_str()
        .filter(|p| vega_herdr::pane::parse(p).is_some())
        .ok_or_else(|| {
            Failure::new(
                "hc_bad_output",
                "spawn returned no terminal address",
                "inspect `vega mux --all` before retrying",
            )
        })?
        .to_string();
    // The agent is now in the space, so the shell that space.new made is
    // surplus: remove it so the agent is the space's only terminal. Removing
    // it earlier would close the space. Failure leaves a harmless extra shell.
    if let Some(p) = &placeholder {
        for verb in ["kill", "rm"] {
            if let Err(f) = hc_local(&[verb, p]) {
                eprintln!(
                    "warning: cannot {verb} the space's placeholder shell {p} ({})",
                    f.message
                );
                break;
            }
        }
    }
    let mut record = old.cloned().unwrap_or_else(|| SpawnRecord {
        backend: "hc".into(),
        pane: pane.clone(),
        name: name.into(),
        kind: kind.into(),
        cwd: reopen::recorded_cwd(&cwd),
        parent: own
            .as_ref()
            .filter(|_| !no_parent)
            .map(|o| vega_herdr::state::Parent {
                pane: o.pane().into(),
                host: vega_herdr::state::post_host(),
            }),
        mode: mode.as_str().into(),
        task: prompt.and_then(task_label),
        session: None,
        extra: selection.args.clone(),
        tier: tier.map(String::from),
        model: selection.model,
        ..Default::default()
    });
    record.pane = pane.clone();
    record.argv = launch_argv;
    record.env = resume_env;
    record.started_ms = Some(vega_herdr::notes::now_ms());
    let saved = reopen::save_best_effort(&record);
    if let Some(old) = old.filter(|o| saved && o.pane != pane) {
        if let Some(p) = state_file("spawns", &old.pane) {
            let _ = std::fs::remove_file(p);
        }
    }
    save_mode(machine(), &pane, name, mode);
    vega_herdr::state::restore_parent(&pane, name, record.parent.as_ref());
    save_task(&pane, record.task.as_deref());
    if let Some(parent) = own.filter(|_| !no_parent && old.is_none() && machine().is_none()) {
        lanes::record_if_uac(
            Path::new(&cwd),
            &env::current_dir().unwrap_or_default(),
            parent.pane(),
        )?;
    }
    vega_herdr::history::emit(
        "spawn",
        serde_json::json!({"pane":pane,"backend":"hc","kind":kind,"parent":record.parent,"tier":record.tier,"model":record.model,"cwd":cwd,"brief":prompt,"started":true}),
    );
    if v["first_message"]["sent"] == false {
        if let Some(text) = prompt {
            keep_brief(
                &pane,
                &KeptBrief {
                    name: name.into(),
                    kind: kind.into(),
                    cwd: cwd.clone(),
                    brief: text.into(),
                    no_parent,
                },
            );
            return Err(Failure::new("brief_not_delivered", format!("terminal {pane} started, but hc did not deliver its first message"), &format!("inspect `vega mux show {pane}`; after resolving startup, use `vega mux spawn --resume {pane}`")));
        }
    }
    if tab_fallback {
        println!("placement: no local herdr-app; started as a tab in the caller's space");
    }
    println!(
        "spawned: {name} ({kind})\npane: {pane}\nbackend: hc\ncwd: {}",
        cell(&cwd)
    );
    if prompt.is_some() {
        println!("prompt: passed to harness at launch; use show to inspect startup");
    }
    let ledger = vega_herdr::resets::read().unwrap_or_default();
    let (field, warning) = capacity::spawn_check(
        kind,
        &selection.args,
        capacity::read(),
        &ledger,
        capacity::now(),
    );
    println!("{field}");
    if let Some(w) = warning {
        eprintln!("{w}");
    }
    Ok(pane)
}

/// Prepare on the target: filesystem paths and worktrunk hooks belong there.
fn prepare_remote(target: &str, kind: &str, cwd: &str, place: &Place) -> Result<Value, Failure> {
    let mut args = vec![
        "exec",
        "--timeout",
        "120",
        "--",
        "vega-mux",
        "prepare",
        "--kind",
        kind,
        "--cwd",
        cwd,
    ];
    if let Place::Worktree { branch, base } = place {
        args.extend(["--worktree", branch]);
        if let Some(base) = base {
            args.extend(["--base", base]);
        }
    }
    let wrapped = vega_herdr::hc::hc_on(
        Some(target),
        &args,
        Some(std::time::Duration::from_secs(125)),
    )
    .map_err(|f| {
        Failure::new(
            "remote_prepare_failed",
            format!("cannot prepare {target}: {}", f.message),
            "deploy matching vega binaries to the target and check its cwd and hc connection",
        )
    })?;
    let prepared = exec_output(target, &wrapped)?;
    if prepared["schema"] != 1
        || !prepared
            .get("trust")
            .is_some_and(|v| v.is_null() || v.is_string())
    {
        return Err(Failure::new(
            "remote_prepare_failed",
            "target returned an incompatible preparation result",
            "deploy the matching vega version to the target before spawning",
        ));
    }
    Ok(prepared)
}

/// Check the configured socket before invoking a CLI that emits plain-text
/// transport errors. HERDR_APP_HOME follows the app CLI's home override.
fn local_shown_space() -> Result<Option<String>, Failure> {
    let home = env::var_os("HERDR_APP_HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            env::var_os("XDG_CONFIG_HOME")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| Path::new(&env::var("HOME").unwrap_or_default()).join(".config"))
                .join("herdr-app")
        });
    if !home.join("herdr-app.sock").exists() {
        return Err(Failure::new(
            "app_unavailable",
            "no local app socket",
            "open herdr-app",
        ));
    }
    shown_space()
}

/// `machine.exec` wraps the program's output: base64 `stdout`, its exit `code`.
fn exec_output(target: &str, wrapped: &Value) -> Result<Value, Failure> {
    use base64::Engine;
    let fail = |m: String| {
        Failure::new(
            "remote_prepare_failed",
            m,
            "deploy the matching vega version to the target before spawning",
        )
    };
    let stdout = wrapped["stdout"]
        .as_str()
        .and_then(|s| base64::engine::general_purpose::STANDARD.decode(s).ok())
        .ok_or_else(|| fail(format!("{target}: machine.exec returned no stdout")))?;
    if wrapped["code"] != 0 {
        let stderr = wrapped["stderr"]
            .as_str()
            .and_then(|s| base64::engine::general_purpose::STANDARD.decode(s).ok())
            .unwrap_or_default();
        return Err(fail(format!(
            "{target}: vega-mux prepare exited {}: {}",
            wrapped["code"],
            String::from_utf8_lossy(&stderr).trim()
        )));
    }
    serde_json::from_slice(&stdout)
        .map_err(|e| fail(format!("{target}: prepare output is not JSON: {e}")))
}

/// The space herdr-app shows now; `None` when none is shown.
fn shown_space() -> Result<Option<String>, Failure> {
    let v = app(&["ls"])?;
    let spaces = v["spaces"].as_array().ok_or_else(|| {
        Failure::new(
            "app_bad_output",
            "herdr-app ls has no spaces list",
            "inspect herdr-app status",
        )
    })?;
    Ok(spaces
        .iter()
        .find(|s| s["active"] == true)
        .and_then(|s| s["id"].as_str())
        .map(String::from))
}

pub fn app(args: &[&str]) -> Result<Value, Failure> {
    let out = Command::new("herdr-app").args(args).output().map_err(|e| {
        Failure::new(
            if e.kind() == std::io::ErrorKind::NotFound {
                "app_unavailable"
            } else {
                "app_failed"
            },
            e.to_string(),
            "open herdr-app",
        )
    })?;
    // Older app CLIs have no structured transport error. Restrict the legacy
    // compatibility check to the explicit no-window diagnostic on failed calls.
    if !out.status.success()
        && [&out.stdout, &out.stderr].iter().any(|s| {
            String::from_utf8_lossy(s)
                .trim()
                .starts_with("no Herdr window at ")
        })
    {
        return Err(Failure::new(
            "app_unavailable",
            "no local app window",
            "open herdr-app",
        ));
    }
    let v: Value = serde_json::from_slice(&out.stdout).map_err(|_| {
        Failure::new(
            "app_bad_output",
            "herdr-app returned non-JSON output",
            "inspect herdr-app status",
        )
    })?;
    if matches!(
        v["error"]["code"].as_str(),
        Some("no_window" | "app_unavailable")
    ) {
        return Err(Failure::new(
            "app_unavailable",
            "no local app window",
            "open herdr-app",
        ));
    }
    if !out.status.success() || v.get("error").is_some() {
        return Err(Failure::new(
            "app_failed",
            "herdr-app command failed",
            "inspect herdr-app status",
        ));
    }
    Ok(v)
}
pub fn reopen(r: &SpawnRecord, extra: &[String]) -> Result<(), Failure> {
    for target in [&r.name, &r.pane] {
        match backend::info(target) {
            Ok(v) if v["state"] == "live" => {
                return Err(Failure::new(
                    "agent_running",
                    "original terminal or name is still live",
                    "close it before reopening",
                ))
            }
            Ok(_) => (),
            Err(f) if f.code == "pane_not_found" => (),
            Err(f) => return Err(f),
        }
    }
    reopen::save(r)?;
    spawn(
        &r.name,
        &r.kind,
        Some(r.cwd.clone()),
        Place::At(At::Tab),
        None,
        &[],
        r.parent.is_none(),
        Mode::parse(&r.mode).unwrap_or(Mode::Headed),
        extra,
        r.tier.as_deref(),
        Some(r),
    )
    .map(|_| ())
}
pub fn resume(pane: &str) -> Result<(), Failure> {
    let r = kept_brief(pane).ok_or_else(|| {
        Failure::new(
            "no_brief",
            "no undelivered brief saved",
            "inspect `vega mux show` before sending another prompt",
        )
    })?;
    let a = backend::agent(pane)?;
    if !same_agent(&r, &a["result"]["agent"]) {
        return Err(Failure::new(
            "pane_reused",
            "terminal no longer matches the saved brief",
            "spawn a new agent",
        ));
    }
    backend::send(
        pane,
        &r.brief,
        handoff(r.no_parent, mode_of(machine(), pane, &r.name)).0,
    )?;
    drop_kept_brief(pane);
    println!("prompt: sent");
    Ok(())
}
