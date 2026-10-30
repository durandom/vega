//! Reopen from durable spawn identity; supervision itself remains ephemeral.
use super::*;
use vega_herdr::state::{save_spawn, spawn_records, SpawnRecord};

/// Store an absolute cwd without changing the path passed to a fresh spawn.
pub fn recorded_cwd(cwd: &str) -> String {
    let path = Path::new(cwd);
    if path.is_absolute() || machine().is_some() {
        cwd.into()
    } else {
        env::current_dir()
            .unwrap_or_default()
            .join(path)
            .display()
            .to_string()
    }
}

pub fn save(record: &SpawnRecord) -> Result<(), Failure> {
    save_spawn(record).map_err(|e| {
        Failure::new(
            "state_write",
            format!("cannot save spawn {}: {e}", record.pane),
            "check the vega state directory permissions and disk space",
        )
    })
}

pub fn save_best_effort(record: &SpawnRecord) -> bool {
    match save(record) {
        Ok(()) => true,
        Err(f) => {
            eprintln!("vega mux: {}; {}", f.message, f.hint);
            false
        }
    }
}

pub fn capture(record: &mut SpawnRecord) {
    if let Ok(v) = herdr(&["pane", "get", &record.pane]) {
        let p = &v["result"]["pane"];
        if p["agent"] == record.kind {
            if let Some(id) = p["agent_session"]["value"]
                .as_str()
                .filter(|s| !s.is_empty())
            {
                record.session = Some(id.into());
            }
        }
    }
}

pub fn snapshot(agent: &Value) {
    let pane = str_of(agent, "pane_id");
    if let Some(mut r) = spawn_records()
        .into_iter()
        .find(|r| r.pane == pane && r.name == agent_name(agent) && r.kind == str_of(agent, "agent"))
    {
        capture(&mut r);
        r.parent = parent_of(pane, &r.name);
        r.mode = mode_of(machine(), pane, &r.name).as_str().into();
        r.task = saved_task(pane);
        save_best_effort(&r);
    }
}

fn resume_args(kind: &str, session: Option<&str>) -> Result<Vec<String>, Failure> {
    let command = match kind {
        "codex" => "resume",
        "claude" => "--resume",
        _ => {
            return Err(Failure::new(
                "unsupported_kind",
                format!("reopen does not support {kind}"),
                "start a new agent with `vega mux spawn <name> --kind claude|codex`",
            ))
        }
    };
    let session = session.filter(|s| !s.is_empty()).ok_or_else(|| {
        Failure::new(
            "no_session",
            "no agent session was recorded before the pane closed",
            "start a new agent with `vega mux spawn <name> --kind claude|codex`",
        )
    })?;
    Ok(vec![command.into(), session.into()])
}

fn find(target: &str, records: Vec<SpawnRecord>) -> Result<SpawnRecord, Failure> {
    // An exact pane takes precedence over a name; names may have been reused.
    if let Some(r) = records.iter().find(|r| r.pane == target) {
        return Ok(r.clone());
    }
    let mut matches = records.into_iter().filter(|r| r.name == target);
    let r = matches.next().ok_or_else(|| {
        Failure::new(
            "spawn_not_found",
            format!("no spawn record for {target}"),
            "reopen an agent started with `vega mux spawn` after this upgrade",
        )
    })?;
    if matches.next().is_some() {
        return Err(Failure::new(
            "ambiguous_name",
            format!("several spawn records name {target}"),
            "use the original pane id with `vega mux reopen <pane>`",
        ));
    }
    Ok(r)
}

pub fn run(target: &str) -> Result<(), Failure> {
    if machine().is_some() {
        return Err(Failure::new(
            "unsupported",
            "reopen needs local session files and cwd",
            "run `vega mux reopen` on the spawning machine",
        ));
    }
    let r = find(target, spawn_records())?;
    let mut extra = r.extra.clone();
    extra.extend(resume_args(&r.kind, r.session.as_deref())?);
    if !Path::new(&r.cwd).is_dir() {
        return Err(Failure::new(
            "cwd_gone",
            format!("recorded cwd {} no longer exists", cell(&r.cwd)),
            "restore the directory or worktree before reopening",
        ));
    }
    match herdr(&["agent", "get", &r.name]) {
        Ok(_) => {
            return Err(Failure::new(
                "agent_running",
                format!("{} is still running", r.name),
                &format!("use `vega mux send {} \"<message>\"`", r.name),
            ))
        }
        Err(f) if f.code == "agent_not_found" => (),
        Err(f) => return Err(f),
    }
    match herdr(&["agent", "get", &r.pane]) {
        Ok(v)
            if v["result"]["agent"]["agent_session"]["value"].as_str() == r.session.as_deref() =>
        {
            return Err(Failure::new(
                "agent_running",
                format!("session still runs in {}", r.pane),
                "close the original agent before reopening",
            ));
        }
        Ok(_) => (), // The old pane id has been reused by another session.
        Err(f) if f.code == "agent_not_found" => (),
        Err(f) => return Err(f),
    }
    // Check durable state before creating any pane; ordinary spawn remains
    // best effort and must never fail after delivering the brief.
    save(&r)?;
    // A workspace reuses the cwd directly: do not recreate a removed worktree.
    // The original parent is installed before startup so hooks see it immediately.
    spawn_inner(
        &r.name,
        &r.kind,
        Some(r.cwd.clone()),
        Place::At(At::Workspace),
        None,
        &[],
        r.parent.is_none(),
        Mode::parse(&r.mode).unwrap_or(Mode::Headed),
        &extra,
        Some(&r),
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn close_then_reopen_preserves_session_cwd_parent_and_mode() {
        if env::var_os("VEGA_REOPEN_TEST").is_none() {
            let dir = env::temp_dir().join(format!("vega-reopen-{}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::create_dir_all(dir.join("vega")).unwrap();
            std::fs::write(
                dir.join("vega/vegad.env"),
                "VEGA_POST_HOST=reopen-registry-host\n",
            )
            .unwrap();
            let out = Command::new(env::current_exe().unwrap())
                .args([
                    "--exact",
                    "reopen::tests::close_then_reopen_preserves_session_cwd_parent_and_mode",
                    "--nocapture",
                ])
                .env("VEGA_REOPEN_TEST", "1")
                .env("XDG_STATE_HOME", &dir)
                .env_remove("VEGA_POST_HOST")
                .output()
                .unwrap();
            std::fs::remove_dir_all(dir).unwrap();
            assert!(
                out.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
            return;
        }
        let dir = PathBuf::from(env::var_os("XDG_STATE_HOME").unwrap())
            .canonicalize()
            .unwrap();
        assert_eq!(vega_herdr::state::post_host(), "reopen-registry-host");
        let bin = dir.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let fake = bin.join("herdr");
        std::fs::write(&fake, include_str!("../tests/fixtures/reopen-herdr.py")).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        let mut paths = vec![bin];
        paths.extend(env::split_paths(&env::var_os("PATH").unwrap()));
        env::set_var("PATH", env::join_paths(paths).unwrap());
        env::set_var("HERDR_PANE_ID", "w0:p1");
        env::set_var("VEGA_TEST_PID", std::process::id().to_string());
        for kind in ["codex", "claude"] {
            for no_parent in [false, true] {
                let mode = if no_parent {
                    Mode::Headed
                } else {
                    Mode::Headless
                };
                env::set_var("VEGA_TEST_KIND", kind);
                env::set_var("VEGA_TEST_MODE", mode.as_str());
                env::set_var("VEGA_TEST_NEXT_PANE", "w1:p1");
                env::set_var("VEGA_TEST_RESUME", "0");
                let _ = std::fs::remove_file(dir.join("prompted"));
                env::set_var("VEGA_TEST_DIALOG", if no_parent { "1" } else { "0" });
                let expected = (!no_parent).then(|| vega_herdr::state::Parent {
                    pane: "w0:p1".into(),
                    host: "reopen-registry-host".into(),
                });
                env::set_var(
                    "VEGA_TEST_PARENT",
                    serde_json::to_string(&expected).unwrap(),
                );
                let result = spawn_inner(
                    "fix",
                    kind,
                    Some(dir.display().to_string()),
                    Place::At(At::Workspace),
                    (!no_parent).then_some("original task"),
                    &[],
                    no_parent,
                    mode,
                    &[],
                    None,
                );
                if no_parent {
                    assert_eq!(result.unwrap_err().code, "timeout");
                } else {
                    assert_eq!(result.unwrap(), "w1:p1");
                }
                let mut r = find("fix", spawn_records()).unwrap();
                assert_eq!(r.session.as_deref(), Some("saved-session"));
                assert_eq!(r.parent, expected);
                assert_eq!(r.mode, mode.as_str());
                assert_eq!(r.cwd, dir.display().to_string());
                assert_eq!(run("fix").unwrap_err().code, "agent_running");
                // A renamed agent still owns the recorded session in its pane.
                let live = dir.join("running");
                let mut renamed: Value =
                    serde_json::from_str(&std::fs::read_to_string(&live).unwrap()).unwrap();
                renamed["name"] = serde_json::json!("renamed");
                std::fs::write(&live, renamed.to_string()).unwrap();
                assert_eq!(run("w1:p1").unwrap_err().code, "agent_running");
                renamed["name"] = serde_json::json!("fix");
                std::fs::write(&live, renamed.to_string()).unwrap();
                // Model a later adoption by a parent on another host.
                r.parent = (!no_parent).then(|| vega_herdr::state::Parent {
                    pane: "original-parent".into(),
                    host: "remote-host".into(),
                });
                vega_herdr::state::restore_parent(&r.pane, &r.name, r.parent.as_ref());
                save_task(&r.pane, Some("original task"));
                close("fix").unwrap();
                assert!(parent_of(&r.pane, &r.name).is_none());
                env::set_var(
                    "VEGA_TEST_PARENT",
                    serde_json::to_string(&r.parent).unwrap(),
                );
                env::set_var("VEGA_TEST_NEXT_PANE", "w2:p1");
                env::set_var("VEGA_TEST_RESUME", "1");
                env::set_var("VEGA_TEST_DIALOG", "0");
                // A different session reusing the old pane must not block reopen.
                renamed["name"] = serde_json::json!("unrelated");
                renamed["agent_session"]["value"] = serde_json::json!("other-session");
                std::fs::write(&live, renamed.to_string()).unwrap();
                run("w1:p1").unwrap();
                let new = find("fix", spawn_records()).unwrap();
                assert_eq!(new.pane, "w2:p1");
                assert_eq!(new.parent, r.parent);
                assert_eq!(new.cwd, r.cwd);
                assert_eq!(new.extra, r.extra);
                assert_eq!(new.task.as_deref(), Some("original task"));
                assert_eq!(mode_of(None, &new.pane, &new.name), mode);
                assert_eq!(parent_of(&new.pane, &new.name), r.parent);
                close("fix").unwrap();
                // A stopped reopen also replaces the original record exactly once.
                env::set_var("VEGA_TEST_NEXT_PANE", "w3:p1");
                env::set_var("VEGA_TEST_DIALOG", "1");
                assert_eq!(run("fix").unwrap_err().code, "timeout");
                let stopped = find("fix", spawn_records()).unwrap();
                assert_eq!(stopped.pane, "w3:p1");
                assert_eq!(stopped.parent, r.parent);
                let mut missing = stopped.clone();
                missing.cwd = dir.join("gone").display().to_string();
                save_spawn(&missing).unwrap();
                assert_eq!(run("fix").unwrap_err().code, "cwd_gone");
                save_spawn(&stopped).unwrap();
                // Force a write failure while keeping the saved record readable.
                let blocked = state_file("spawns", &stopped.pane)
                    .unwrap()
                    .with_extension(format!("{}.tmp", std::process::id()));
                std::fs::create_dir(&blocked).unwrap();
                close("fix").unwrap();
                // Snapshot failure must not prevent close. Reopen checks state
                // before creating a pane and reports the same write failure.
                assert!(!live.exists());
                assert_eq!(run("fix").unwrap_err().code, "state_write");
                assert!(!live.exists());
                std::fs::remove_dir(blocked).unwrap();
                std::fs::remove_file(state_file("spawns", &stopped.pane).unwrap()).unwrap();
            }
        }
        // A fresh spawn still succeeds when its new state directory is unwritable.
        let spawns = state_file("spawns", "w4:p1")
            .unwrap()
            .parent()
            .unwrap()
            .to_path_buf();
        std::fs::remove_dir(&spawns).unwrap();
        std::fs::write(&spawns, "blocks directory creation").unwrap();
        env::set_var("VEGA_TEST_PARENT", "null");
        env::set_var("VEGA_TEST_MODE", "headed");
        env::set_var("VEGA_TEST_NEXT_PANE", "w4:p1");
        env::set_var("VEGA_TEST_RESUME", "0");
        env::set_var("VEGA_TEST_DIALOG", "0");
        assert_eq!(
            spawn_inner(
                "fix",
                "claude",
                Some(dir.display().to_string()),
                Place::At(At::Workspace),
                None,
                &[],
                true,
                Mode::Headed,
                &[],
                None
            )
            .unwrap(),
            "w4:p1"
        );
        assert!(dir.join("running").exists());
        close("fix").unwrap();
        std::fs::remove_file(spawns).unwrap();
        assert_eq!(find("unknown", vec![]).unwrap_err().code, "spawn_not_found");
    }

    #[test]
    fn recorded_cwd_is_absolute_without_resolving_symlinks() {
        assert_eq!(recorded_cwd("/tmp/symlink"), "/tmp/symlink");
        assert_eq!(
            recorded_cwd("relative"),
            env::current_dir()
                .unwrap()
                .join("relative")
                .display()
                .to_string()
        );
    }

    #[test]
    fn resume_cli_syntax_and_errors() {
        assert_eq!(
            resume_args("codex", Some("uuid")).unwrap(),
            ["resume", "uuid"]
        );
        assert_eq!(
            resume_args("claude", Some("uuid")).unwrap(),
            ["--resume", "uuid"]
        );
        assert_eq!(
            resume_args("pi", Some("uuid")).unwrap_err().code,
            "unsupported_kind"
        );
        assert_eq!(resume_args("codex", None).unwrap_err().code, "no_session");
        assert_eq!(
            resume_args("claude", Some("")).unwrap_err().code,
            "no_session"
        );
    }
    #[test]
    fn reopen_parser_rejects_unknown_flags() {
        assert!(Cli::try_parse_from(["vega-mux", "reopen", "fix"]).is_ok());
        assert!(Cli::try_parse_from(["vega-mux", "reopen", "fix", "--resume"]).is_err());
        assert!(Cli::try_parse_from(["vega-mux", "reopen"]).is_err());
    }
}
