//! hc owns terminal creation and input; vega retains launch policy and lineage.
use super::*;
use vega_herdr::hc::{hc_local, hc_on, own_terminal, Terminal};
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
    let mut space = if let Some(Terminal::Hc(p)) = &own {
        hc_local(&["get", p])?["labels"]["herdr.space"]
            .as_str()
            .map(String::from)
    } else {
        None
    };
    if let Place::Worktree { branch, base } = place {
        let (path, how) = worktree::hc_worktree(&cwd, &branch, base.as_deref())?;
        cwd = path;
        println!("worktree: {} ({how})", cell(&branch));
        let v = app(&["do", "space.new", "--name", &branch, "--folder", &cwd])?;
        space = v["space"].as_str().or(v["id"].as_str()).map(String::from);
        if space.is_none() {
            return Err(Failure::new(
                "app_bad_output",
                "new space has no id",
                "inspect herdr-app ls before retrying",
            ));
        }
    }
    if machine().is_none() && !no_parent {
        vega_herdr::project::resolve_optional(Path::new(&cwd), &vega_herdr::state::post_host())?;
    }
    if machine().is_none() {
        if let Some(line) = trust::apply(kind, &cwd) {
            println!("{line}");
        }
    }
    let actor = beads_actor(&cwd);
    let socket = env::var("SSH_AUTH_SOCK")
        .ok()
        .filter(|_| machine().is_none());
    let vars = spawn_env(kind, actor.as_deref(), socket.as_deref(), user);
    let mut args: Vec<String> = [
        "run",
        "--bg",
        "--outlive-parent",
        "--name",
        name,
        "--cwd",
        &cwd,
        "--label",
        &format!("vega.kind={kind}"),
        "--label",
        &format!("vega.mode={}", mode.as_str()),
        "--label",
        "vega.role=sentinel",
    ]
    .into_iter()
    .map(String::from)
    .collect();
    if no_parent || !matches!(own, Some(Terminal::Hc(_))) {
        args.push("--no-parent".into());
    }
    if let Some(s) = space {
        args.extend(["--label".into(), format!("herdr.space={s}")]);
    }
    args.extend([
        "--".into(),
        "env".into(),
        "-u".into(),
        "HERDR_PANE_ID".into(),
        "-u".into(),
        "HERDR_SOCKET_PATH".into(),
    ]);
    args.extend(vars);
    if machine().is_none() {
        args.extend(["nice".into(), "-n".into(), AGENT_NICE.to_string()]);
    }
    args.push(kind.into());
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
            backend::body(p, handoff(no_parent, mode).0)?
        });
    }
    let v = hc_on(
        machine(),
        &args.iter().map(String::as_str).collect::<Vec<_>>(),
        None,
    )?;
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
