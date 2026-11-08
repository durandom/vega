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
    let mut placeholder = None;
    if let Place::Worktree { branch, base } = place {
        let (path, how) = worktree::hc_worktree(&cwd, &branch, base.as_deref())?;
        cwd = path;
        println!("worktree: {} ({how})", cell(&branch));
        // space.new shows the new space; a spawn must not take the person's screen.
        let shown = shown_space();
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
        if let Some(prev) = shown.filter(|p| Some(p) != space.as_ref()) {
            // Best effort: the space exists either way; only the screen differs.
            let _ = app(&["do", "space.select", "--id", &prev]);
        }
    }
    if machine().is_none() && !no_parent {
        vega_herdr::project::resolve_optional(Path::new(&cwd), &vega_herdr::state::post_host())?;
    }
    let trust_line = if let Some(target) = machine() {
        let prepared =
            vega_herdr::hc::hc_on(
                Some(target),
                &[
                    "exec",
                    "--timeout",
                    "15",
                    "--",
                    "vega-mux",
                    "prepare",
                    "--kind",
                    kind,
                    "--cwd",
                    &cwd,
                ],
                Some(std::time::Duration::from_secs(20)),
            )
            .map_err(|f| {
                Failure::new("remote_prepare_failed",
            format!("cannot prepare {target}: {}", f.message),
            "deploy matching vega binaries to the target and check its cwd and hc connection")
            })?;
        let prepared = exec_output(target, &prepared)?;
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
    if let Some(p) = &placeholder {
        labels["vega.worktree-placeholder"] = p.clone().into();
    }
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
    if let Some(p) = &placeholder {
        hc_local(&["label", p, &format!("vega.worktree-owner={pane}")])?;
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
    });
    record.pane = pane.clone();
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

/// The space herdr-app shows now, if any.
fn shown_space() -> Option<String> {
    let v = app(&["ls"]).ok()?;
    let shown = v["spaces"]
        .as_array()?
        .iter()
        .find(|s| s["active"] == true)?;
    shown["id"].as_str().map(String::from)
}

pub fn app(args: &[&str]) -> Result<Value, Failure> {
    let out = Command::new("herdr-app")
        .args(args)
        .output()
        .map_err(|e| Failure::new("app_unavailable", e.to_string(), "open herdr-app"))?;
    let v: Value = serde_json::from_slice(&out.stdout).map_err(|_| {
        Failure::new(
            "app_bad_output",
            "herdr-app returned non-JSON output",
            "inspect herdr-app status",
        )
    })?;
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
