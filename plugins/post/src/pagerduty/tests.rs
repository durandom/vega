use super::*;
use serde_json::json;
use tempfile::{tempdir, TempDir};

const PROJECT: &str = "github.com/team/orrery";

fn incident(status: &str, changed: &str) -> Incident {
    serde_json::from_value(json!({
        "id":"QTEST01", "incident_number":43, "title":"disk full",
        "status":status, "last_status_change_at":changed,
        "service":{"id":"PTEST01", "summary":"Example Service"},
        "html_url":"https://example.pagerduty.com/incidents/QTEST01",
        "urgency":"high", "created_at":"2026-09-01T00:00:00Z"
    }))
    .unwrap()
}

// Routing policy fixtures already use their synthetic project strings as IDs.
fn destination(i: &Incident, ownership: &Ownership, view: &View) -> Destination {
    super::destination(i, ownership, view)
}

#[test]
fn local_ownership_path_routes_to_remote_v2_uac_id() {
    let tmp = tempdir().unwrap();
    let repo = tmp.path().join("different-local-folder");
    assert!(std::process::Command::new("git")
        .args(["init", "-q"])
        .arg(&repo)
        .status()
        .unwrap()
        .success());
    assert!(std::process::Command::new("git")
        .arg("-C")
        .arg(&repo)
        .args(["remote", "add", "origin", "git@github.com:team/repo.git"])
        .status()
        .unwrap()
        .success());
    let project = vega_herdr::project::resolve(&repo, &vega_herdr::state::post_host()).unwrap();
    let key = vega_herdr::lanes::project_key(&repo).unwrap();
    let own = Ownership {
        projects: [(key.clone(), ["Example Service".into()].into())].into(),
        ..Default::default()
    };
    let v = view_for(&project.id, "remote-host", "remote-pane");
    let to = super::destination(&incident("triggered", "t1"), &own, &v);
    assert!(
        matches!(to, Destination::Mail { project, host, pane } if project == key && host == "remote-host" && pane == "remote-pane")
    );
}

fn view(host: &str, pane: &str) -> View {
    view_for(PROJECT, host, pane)
}

fn view_for(project: &str, host: &str, pane: &str) -> View {
    let mut view = View::default();
    view.entries.insert(
        vega_registry::beat_key(host),
        json!({"at":vega_bd_sync::unix_now()}),
    );
    view.entries.insert(
        vega_registry::agent_key(host, pane),
        json!({"pane":pane,"status":"idle"}),
    );
    view.entries.insert(
        vega_registry::uac_key(host, project),
        json!({"schema":2,"host":host,"pane":pane,"project":project}),
    );
    view
}

fn ownership() -> Ownership {
    Ownership {
        projects: [(PROJECT.into(), ["Example Service".into()].into())].into(),
        ..Default::default()
    }
}

struct Fake {
    incidents: Vec<Incident>,
    poll_error: Option<String>,
    poll_calls: usize,
    fail_delivery: bool,
    registry_error: bool,
    view: View,
    ownership: Ownership,
    mails: Vec<(Destination, String)>,
    cleared: Vec<String>,
}

impl Default for Fake {
    fn default() -> Self {
        Self {
            incidents: vec![incident("triggered", "t1")],
            poll_error: None,
            poll_calls: 0,
            fail_delivery: false,
            registry_error: false,
            view: view("ganymede", "w1:p1"),
            ownership: ownership(),
            mails: Vec::new(),
            cleared: Vec::new(),
        }
    }
}

impl Ops for Fake {
    fn poll(&mut self) -> Result<Vec<Incident>, String> {
        self.poll_calls += 1;
        match &self.poll_error {
            Some(e) => Err(e.clone()),
            None => Ok(self.incidents.clone()),
        }
    }
    fn ownership(&mut self) -> Result<Ownership, String> {
        Ok(self.ownership.clone())
    }
    fn registry(&mut self) -> Result<View, String> {
        if self.registry_error {
            Err("registry unavailable".into())
        } else {
            Ok(self.view.clone())
        }
    }
    fn deliver(
        &mut self,
        to: &Destination,
        _: &Incident,
        text: &str,
        _: i64,
    ) -> Result<(), String> {
        if self.fail_delivery {
            return Err("mail unavailable".into());
        }
        self.mails.push((to.clone(), text.into()));
        Ok(())
    }
    fn clear_notes(&mut self, state: &State, _: i64) {
        self.cleared = state.incidents.keys().cloned().collect();
    }
}

fn state_file(dir: &TempDir) -> PathBuf {
    dir.path().join("state.json")
}

#[test]
fn transition_and_four_hour_clocks_survive_restart() {
    let dir = tempdir().unwrap();
    let path = state_file(&dir);
    let mut ops = Fake::default();
    cycle(&path, 0, &mut ops).unwrap();
    assert_eq!(ops.mails.len(), 1);
    cycle(&path, 60, &mut ops).unwrap();
    assert_eq!(ops.poll_calls, 1);
    assert_eq!(ops.mails.len(), 1);
    for (now, status, change) in [
        (900, "acknowledged", "t2"),
        (1800, "triggered", "t3"),
        (2700, "triggered", "t4"),
    ] {
        ops.incidents[0] = incident(status, change);
        cycle(&path, now, &mut ops).unwrap();
    }
    assert_eq!(ops.mails.len(), 4);
    // Every cycle reloads disk, exercising restart at both sides of the boundary.
    cycle(&path, 2700 + REMIND_EVERY - 1, &mut ops).unwrap();
    assert_eq!(ops.mails.len(), 4);
    cycle(&path, 2700 + REMIND_EVERY, &mut ops).unwrap();
    assert_eq!(ops.mails.len(), 5);
    assert!(ops.mails[4].1.contains("reminder, still triggered"));
    cycle(&path, 2700 + 5 * REMIND_EVERY, &mut ops).unwrap();
    assert_eq!(ops.mails.len(), 6); // One overdue reminder, no backlog.
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
}

#[test]
fn failed_poll_never_closes_an_incident_or_resets_the_reminder_clock() {
    let dir = tempdir().unwrap();
    let path = state_file(&dir);
    let mut ops = Fake::default();
    cycle(&path, 0, &mut ops).unwrap();
    ops.poll_error = Some("partial snapshot".into());
    assert!(cycle(&path, 900, &mut ops).is_err());
    assert!(cycle(&path, 960, &mut ops).is_err());
    assert_eq!(ops.poll_calls, 2);
    let state = State::load(&path).unwrap();
    assert_eq!(state.incidents.len(), 1);
    assert_eq!(
        state
            .incidents
            .values()
            .next()
            .unwrap()
            .delivered
            .as_ref()
            .unwrap()
            .at,
        0
    );
    ops.poll_error = None;
    cycle(&path, 1800, &mut ops).unwrap();
    assert_eq!(ops.mails.len(), 1);
    ops.incidents.clear();
    cycle(&path, 2700, &mut ops).unwrap();
    assert!(State::load(&path).unwrap().incidents.is_empty());
    assert!(ops.cleared.is_empty());
    cycle(&path, REMIND_EVERY, &mut ops).unwrap();
    assert_eq!(ops.mails.len(), 1);
    ops.incidents.push(incident("triggered", "t1"));
    cycle(&path, REMIND_EVERY + POLL_EVERY, &mut ops).unwrap();
    assert_eq!(ops.mails.len(), 2);
}

#[test]
fn failed_send_retries_and_crash_after_send_may_duplicate_but_does_not_lose_mail() {
    let dir = tempdir().unwrap();
    let path = state_file(&dir);
    let mut ops = Fake {
        fail_delivery: true,
        ..Default::default()
    };
    assert!(cycle(&path, 0, &mut ops).is_err());
    let pending = fs::read(&path).unwrap();
    ops.fail_delivery = false;
    cycle(&path, 60, &mut ops).unwrap();
    assert_eq!(ops.poll_calls, 1);
    assert_eq!(ops.mails.len(), 1);
    // Replay the pre-acceptance checkpoint as if a crash lost the final rename.
    fs::write(&path, pending).unwrap();
    cycle(&path, 120, &mut ops).unwrap();
    assert_eq!(ops.mails.len(), 2);
    cycle(&path, 180, &mut ops).unwrap();
    assert_eq!(ops.mails.len(), 2);
}

#[test]
fn board_fallback_hands_off_to_a_remote_uac_without_waiting_four_hours() {
    let dir = tempdir().unwrap();
    let path = state_file(&dir);
    let mut ops = Fake {
        view: View::default(),
        ..Default::default()
    };
    cycle(&path, 0, &mut ops).unwrap();
    assert!(matches!(
        ops.mails[0].0,
        Destination::Board {
            project: Some(_),
            ..
        }
    ));
    cycle(&path, 60, &mut ops).unwrap();
    assert_eq!(ops.mails.len(), 1);
    ops.view = view("ganymede", "w1:p1");
    cycle(&path, 120, &mut ops).unwrap();
    assert!(matches!(&ops.mails[1].0, Destination::Mail { host, .. } if host == "ganymede"));
    assert!(!ops.mails[1].1.contains("reminder"));
    ops.view = view("titan", "w5:p2");
    cycle(&path, 180, &mut ops).unwrap();
    assert_eq!(ops.mails.len(), 3);
}

#[test]
fn unmapped_duplicate_local_owner_and_stale_or_duplicate_uacs_reach_the_board() {
    let i = incident("triggered", "t1");
    let mut owners = Ownership::default();
    assert!(matches!(
        destination(&i, &owners, &View::default()),
        Destination::Board { project: None, .. }
    ));
    owners = ownership();
    owners
        .projects
        .insert("~/src/other/.git".into(), [i.service.id.clone()].into());
    assert!(
        matches!(destination(&i,&owners,&View::default()), Destination::Board { reason, .. } if reason.contains("multiple local"))
    );
    let mut v = view("ganymede", "w1:p1");
    v.entries
        .insert(vega_registry::beat_key("ganymede"), json!({"at":0}));
    assert!(matches!(
        destination(&i, &ownership(), &v),
        Destination::Board {
            project: Some(_),
            ..
        }
    ));
    v = view("ganymede", "w1:p1");
    v.entries.extend(view("titan", "w5:p1").entries);
    assert!(
        matches!(destination(&i,&ownership(),&v), Destination::Board { reason, .. } if reason.contains("several UACs"))
    );
    let mut owners = ownership();
    owners
        .invalid_configs
        .insert(PROJECT.into(), "invalid config".into());
    assert!(
        matches!(destination(&i,&owners,&v), Destination::Board { reason, .. } if reason.contains("invalid local"))
    );
}

#[test]
fn registry_outage_is_not_misreported_as_no_uac() {
    let dir = tempdir().unwrap();
    let path = state_file(&dir);
    let mut ops = Fake {
        registry_error: true,
        ..Default::default()
    };
    assert!(cycle(&path, 0, &mut ops).is_err());
    assert!(ops.mails.is_empty());
    ops.registry_error = false;
    cycle(&path, 60, &mut ops).unwrap();
    assert_eq!(ops.poll_calls, 1);
    assert_eq!(ops.mails.len(), 1);
}

#[test]
fn corrupt_or_unwritable_state_prevents_sending() {
    let dir = tempdir().unwrap();
    let path = state_file(&dir);
    let mut ops = Fake::default();
    fs::write(&path, "broken").unwrap();
    assert!(cycle(&path, 0, &mut ops).unwrap_err().starts_with("state:"));
    assert_eq!(ops.poll_calls, 0);
    assert!(ops.mails.is_empty());
    assert_eq!(fs::read_to_string(&path).unwrap(), "broken");
    fs::remove_file(&path).unwrap();
    fs::create_dir(path.with_extension("tmp")).unwrap();
    assert!(cycle(&path, 0, &mut ops).is_err());
    assert_eq!(ops.poll_calls, 0);
}

#[test]
fn full_json_is_required_and_partial_or_budgeted_output_is_rejected() {
    let i = incident("triggered", "t1");
    let value = serde_json::to_value(&i).unwrap();
    assert_eq!(
        parse_incidents(serde_json::to_string(&vec![&i]).unwrap().as_bytes())
            .unwrap()
            .len(),
        1
    );
    for bytes in [b"[]".as_slice(), b"null"] {
        assert!(parse_incidents(bytes).unwrap().is_empty());
    }
    for bytes in [
        b"".as_slice(),
        b"{}",
        b"{\"success\":false}",
        b"{\"success\":true,\"data\":[],\"metadata\":{\"more\":true}}",
    ] {
        assert!(parse_incidents(bytes).is_err());
    }
    let mut missing = value.clone();
    missing.as_object_mut().unwrap().remove("html_url");
    assert!(parse_incidents(json!([missing]).to_string().as_bytes()).is_err());
    assert!(parse_incidents(json!([value, value]).to_string().as_bytes()).is_err());
    assert!(parse_incidents(serde_json::to_string(&vec![i; 10_000]).unwrap().as_bytes()).is_err());
}

#[test]
fn ownership_toml_is_exact_and_rejects_unknown_or_empty_selectors() {
    assert_eq!(
        parse_services("services = ['Example Service', 'PTEST01', 'Example Service']")
            .unwrap()
            .len(),
        2
    );
    assert!(parse_services("services = []").unwrap().is_empty());
    for s in [
        "",
        "services = ['']",
        "services = [42]",
        "services='x'",
        "services=[]\nhost='proteus'",
        "services = [",
    ] {
        assert!(parse_services(s).is_err(), "{s}");
    }
}

fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .current_dir(dir)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .args([
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
        ])
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn committed_main_head_config_ignores_dirty_edits_and_sentinel_heads() {
    let dir = tempdir().unwrap();
    git(dir.path(), &["init"]);
    fs::create_dir(dir.path().join(".vega")).unwrap();
    let config = dir.path().join(CONFIG);
    fs::write(&config, "services=['Example Service']").unwrap();
    git(dir.path(), &["add", "."]);
    git(
        dir.path(),
        &["commit", "-m", "fixture", "--trailer", "Assisted-by: codex"],
    );
    let common = dir.path().join(".git");
    fs::write(&config, "services=['dirty']").unwrap();
    assert!(read_services(&common).unwrap().contains("Example Service"));
    assert!(!read_services(&common).unwrap().contains("dirty"));
    // A linked worktree's HEAD may point elsewhere; only the common HEAD is read.
    fs::create_dir_all(common.join("worktrees/sentinel")).unwrap();
    fs::write(
        common.join("worktrees/sentinel/HEAD"),
        "ref: refs/heads/nonexistent\n",
    )
    .unwrap();
    assert!(read_services(&common).unwrap().contains("Example Service"));
    git(dir.path(), &["rm", "-f", CONFIG]);
    git(
        dir.path(),
        &[
            "commit",
            "-m",
            "remove mapping",
            "--trailer",
            "Assisted-by: codex",
        ],
    );
    assert!(read_services(&common).unwrap().is_empty());
}

fn executable(dir: &Path, script: &str) -> PathBuf {
    let path = dir.join("fake-pdc");
    fs::write(&path, format!("#!/bin/sh\n{script}\n")).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
    path
}

#[test]
fn fake_pdc_verifies_isolated_read_only_call_and_failures_hide_stderr() {
    let dir = tempdir().unwrap();
    fs::write(
        dir.path().join("response.json"),
        serde_json::to_vec(&vec![incident("triggered", "t1")]).unwrap(),
    )
    .unwrap();
    let bin = executable(
        dir.path(),
        r#"
[ "$PDC_TOKEN" = test-secret ] || exit 1
[ -z "$PDC_TEAM" ] && [ -z "$PDC_SERVICE" ] || exit 1
printf '%s\n' "$@" > args
cat response.json
"#,
    );
    assert_eq!(
        run_pdc(&bin, dir.path(), &[("PDC_TOKEN", "test-secret")], TIMEOUT)
            .unwrap()
            .len(),
        1
    );
    let args = fs::read_to_string(dir.path().join("args")).unwrap();
    assert!(args.contains("--format\njson\n"));
    assert!(args.ends_with("incident\nlist\n--all\n--status\ntriggered\n--status\nacknowledged\n"));
    assert!(!args.contains("test-secret"));
    assert_eq!(
        fs::read_to_string(dir.path().join("pdc.toml")).unwrap(),
        "config_version = 1\n"
    );
    let bin = executable(
        dir.path(),
        "echo 'test-secret' >&2\ncat response.json\nexit 1",
    );
    let error = run_pdc(&bin, dir.path(), &[("PDC_TOKEN", "test-secret")], TIMEOUT).unwrap_err();
    assert!(!error.contains("test-secret"));
    let bin = executable(dir.path(), "exec sleep 30");
    assert!(run_pdc(
        &bin,
        dir.path(),
        &[("PDC_TOKEN", "test-secret")],
        Duration::from_millis(40)
    )
    .unwrap_err()
    .contains("timed out"));
    assert!(run_pdc(
        &dir.path().join("missing"),
        dir.path(),
        &[("PDC_TOKEN", "test-secret")],
        TIMEOUT
    )
    .is_err());
}

#[test]
fn log_once_recovers_and_mail_contains_rights_and_bounded_untrusted_text() {
    let mut said = Failures::default();
    let errors = ["pdc unavailable".into()].into();
    assert_eq!(said.update(errors).len(), 1);
    assert!(said.update(["pdc unavailable".into()].into()).is_empty());
    assert_eq!(said.update(BTreeSet::new()), vec!["pagerduty: recovered"]);
    assert_eq!(said.update(["pdc unavailable".into()].into()).len(), 1);
    let mut i = incident("triggered", "t1");
    i.title = format!("\u{001b}[31m{}\nINJECT", "x".repeat(5000));
    let text = mail_text(
        &i,
        &Destination::Board {
            project: None,
            reason: "unmapped".into(),
        },
        false,
    );
    assert!(text.contains("Example Service (PTEST01)"));
    assert!(text.contains(&i.html_url));
    assert!(text
        .contains("Resolve, escalate or reassign only after the Slayer approves a board decision"));
    assert!(text.contains("Board fallback: unmapped"));
    assert!(!text.contains('\u{001b}'));
    assert!(text.len() < 1800);
}

#[test]
fn board_notes_are_durable_scoped_and_resolve_on_handoff_or_absence() {
    let dir = tempdir().unwrap();
    let store = Store {
        dir: dir.path().join("notes"),
    };
    let i = incident("triggered", "t1");
    let to = Destination::Board {
        project: Some(PROJECT.into()),
        reason: "no UAC".into(),
    };
    let text = mail_text(&i, &to, false);
    board_note(&store, "proteus", &to, &i, &text, 100).unwrap();
    let notes = store.list();
    assert_eq!(notes.len(), 1);
    assert_eq!(notes[0].project.as_deref(), Some(PROJECT));
    assert_eq!(notes[0].cause.as_deref(), Some("pagerduty:QTEST01"));
    assert_eq!(notes[0].body, text);
    let mut state = State::default();
    state.observe(vec![i.clone()], 100);
    resolve_notes(&store, &state, 200);
    assert!(store.list()[0].resolved.is_none());
    state.incidents.get_mut(&i.id).unwrap().delivered = Some(Receipt {
        status: i.status.clone(),
        changed_at: i.last_status_change_at.clone(),
        to: Destination::Mail {
            project: PROJECT.into(),
            host: "ganymede".into(),
            pane: "w1:p1".into(),
        },
        at: 300,
    });
    resolve_notes(&store, &state, 300);
    assert_eq!(store.list()[0].resolved, Some(300));
    board_note(&store, "proteus", &to, &i, &text, 400).unwrap();
    state.observe(Vec::new(), 500);
    resolve_notes(&store, &state, 500);
    assert!(store.list().iter().all(|n| n.resolved.is_some()));
    let unavailable = Store {
        dir: dir.path().join("file"),
    };
    fs::write(&unavailable.dir, "not a directory").unwrap();
    assert!(board_note(&unavailable, "proteus", &to, &i, &text, 600).is_err());
}

#[test]
fn agent_project_discovery_uses_local_workspace_common_dirs() {
    let agents = json!({"result":{"agents":[
        {"workspace_id":"w1","repo_key":"/ignored-agent-metadata","cwd":"/not/the/repo"},
        {"repo_key":"/local/orrery/.git","workspace_id":"w2"},
        {"workspace_id":"w3","cwd":"/nonexistent-pd-test-directory"}
    ]}});
    let spaces = json!({"result":{"workspaces":[{"workspace_id":"w1","worktree":{"repo_key":"/local/orrery/.git"}}]}});
    assert_eq!(
        agent_projects(&agents, &spaces),
        vec![
            "/local/orrery/.git",
            "/local/orrery/.git",
            "/nonexistent-pd-test-directory"
        ]
    );
}

#[test]
fn missing_token_never_invokes_pdc_or_uses_personal_credentials() {
    let dir = tempdir().unwrap();
    let bin = executable(dir.path(), "touch invoked\necho null");
    assert!(run_pdc(&bin, dir.path(), &[], TIMEOUT)
        .unwrap_err()
        .contains("PDC_TOKEN"));
    assert!(!dir.path().join("invoked").exists());
    assert!(!dir.path().join("pdc.toml").exists());
}

#[test]
fn a_changed_board_reason_does_not_rearm_the_same_incident_status() {
    let dir = tempdir().unwrap();
    let path = state_file(&dir);
    let mut ops = Fake {
        view: View::default(),
        ..Default::default()
    };
    cycle(&path, 0, &mut ops).unwrap();
    let mut state = State::load(&path).unwrap();
    let tracked = state.incidents.values_mut().next().unwrap();
    let Destination::Board { reason, .. } = &mut tracked.delivered.as_mut().unwrap().to else {
        panic!("board receipt expected");
    };
    *reason = "different unavailable-UAC detail".into();
    state.save(&path).unwrap();
    cycle(&path, 60, &mut ops).unwrap();
    assert_eq!(ops.mails.len(), 1);
}

#[test]
fn inherited_agent_environment_still_produces_full_json() {
    // Run in a separate process: do not mutate the parallel test suite's environment.
    // These are pdc v0.13.5's detection markers, independently listed in this fixture.
    let markers = [
        "CLAUDE_CODE",
        "CLAUDECODE",
        "CURSOR_AGENT",
        "CODEX",
        "OPENAI_CODEX",
        "AIDER",
        "CLINE",
        "WINDSURF_AGENT",
        "GITHUB_COPILOT",
        "AMAZON_Q",
        "AWS_Q_DEVELOPER",
        "GEMINI_CODE_ASSIST",
        "SRC_CODY",
        "FORCE_AGENT_MODE",
    ];
    if env::var_os("VEGA_PAGERDUTY_ENV_TEST").is_none() {
        let mut child = Command::new(env::current_exe().unwrap());
        child
            .args([
                "--exact",
                "pagerduty::tests::inherited_agent_environment_still_produces_full_json",
            ])
            .env("VEGA_PAGERDUTY_ENV_TEST", "1");
        for marker in markers {
            child.env(marker, "1");
        }
        let out = child.output().unwrap();
        assert!(
            out.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        return;
    }
    for marker in markers {
        assert_eq!(env::var(marker).unwrap(), "1");
    }
    let dir = tempdir().unwrap();
    fs::write(
        dir.path().join("response.json"),
        serde_json::to_vec(&vec![incident("triggered", "t1")]).unwrap(),
    )
    .unwrap();
    let inherited = markers
        .iter()
        .map(|m| format!("${{{m}}}"))
        .collect::<String>();
    let bin = executable(
        dir.path(),
        &format!(
            r#"
if [ -n "{inherited}" ]; then
  echo '{{"success":true,"data":[],"metadata":{{"total":0,"more":false}}}}'
else
  cat response.json
fi
"#
        ),
    );
    // pdc gives agent markers priority over --format json. They must be removed.
    assert_eq!(
        run_pdc(&bin, dir.path(), &[("PDC_TOKEN", "test-secret")], TIMEOUT)
            .unwrap()
            .len(),
        1
    );
}

fn mapped_project(dir: &Path) -> String {
    git(dir, &["init"]);
    fs::create_dir(dir.join(".vega")).unwrap();
    fs::write(dir.join(CONFIG), "services=['Example Service']").unwrap();
    git(dir, &["add", "."]);
    git(
        dir,
        &["commit", "-m", "fixture", "--trailer", "Assisted-by: codex"],
    );
    vega_herdr::home_relative(
        &fs::canonicalize(dir.join(".git"))
            .unwrap()
            .to_string_lossy(),
    )
}

#[test]
fn a_stale_registration_without_a_checkout_does_not_block_mapped_uac_mail() {
    let dir = tempdir().unwrap();
    let project_path = mapped_project(dir.path());
    let project = vega_herdr::project::resolve(dir.path(), &vega_herdr::state::post_host())
        .unwrap()
        .id
        .to_string();
    let missing = dir
        .path()
        .join("missing/.git")
        .to_string_lossy()
        .to_string();
    let ownership = read_ownership(vec![missing, project_path.clone(), project_path], |p| {
        fs::canonicalize(p)
    });
    assert_eq!(ownership.projects.len(), 1);
    assert!(ownership.invalid_configs.is_empty());
    assert!(ownership.lookup_errors.is_empty());
    let mut ops = Fake {
        ownership,
        view: view_for(&project, "ganymede", "w1:p1"),
        ..Default::default()
    };
    cycle(&dir.path().join("state.json"), 0, &mut ops).unwrap();
    assert_eq!(ops.mails.len(), 1);
    assert!(matches!(&ops.mails[0].0, Destination::Mail { project:p, .. } if p == &project));
}

#[test]
fn canonicalization_errors_are_per_project_and_other_projects_are_still_read_and_mailed() {
    let healthy = tempdir().unwrap();
    let project_path = mapped_project(healthy.path());
    let project = vega_herdr::project::resolve(healthy.path(), &vega_herdr::state::post_host())
        .unwrap()
        .id
        .to_string();
    let unavailable = tempdir().unwrap();
    let other_path = mapped_project(unavailable.path());
    let other = vega_herdr::project::resolve(unavailable.path(), &vega_herdr::state::post_host())
        .unwrap()
        .id
        .to_string();
    let unavailable_common = fs::canonicalize(unavailable.path().join(".git")).unwrap();
    let ownership = read_ownership(vec![other_path, project_path], |p| {
        if p == unavailable_common {
            Err(std::io::Error::other("simulated lookup failure"))
        } else {
            fs::canonicalize(p)
        }
    });
    assert!(ownership.lookup_errors.contains_key(&other));
    assert!(ownership.invalid_configs.is_empty());
    assert!(ownership.projects.contains_key(&project));
    let mut ops = Fake {
        ownership,
        view: view_for(&project, "ganymede", "w1:p1"),
        ..Default::default()
    };
    assert!(cycle(&healthy.path().join("state.json"), 0, &mut ops)
        .unwrap_err()
        .contains("cannot canonicalize"));
    assert_eq!(ops.mails.len(), 1);
    assert!(matches!(&ops.mails[0].0, Destination::Mail { project:p, .. } if p == &project));
}

#[test]
fn granted_secret_environment_preserves_region_and_overrides_filters_and_agent_mode() {
    let dir = tempdir().unwrap();
    let bin = executable(
        dir.path(),
        r#"
[ "$PDC_TOKEN" = test-secret ] || exit 1
[ "$PDC_BASE_URL" = https://api.eu.pagerduty.com ] || exit 1
[ "$OTHER_PDC_GRANT" = preserved ] || exit 1
[ -z "$PDC_SERVICE" ] && [ -z "$PDC_TEAM" ] || exit 1
[ -z "$FORCE_AGENT_MODE" ] || exit 1
printf 'null\n'
"#,
    );
    let vars = [
        ("PDC_TOKEN", "test-secret"),
        ("PDC_BASE_URL", "https://api.eu.pagerduty.com"),
        ("OTHER_PDC_GRANT", "preserved"),
        ("PDC_SERVICE", "accidental service filter"),
        ("PDC_TEAM", "accidental team filter"),
        ("FORCE_AGENT_MODE", "1"),
    ];
    assert!(run_pdc(&bin, dir.path(), &vars, TIMEOUT)
        .unwrap()
        .is_empty());
}

#[test]
fn diagnostic_note_store_failure_does_not_block_healthy_uac_mail() {
    let dir = tempdir().unwrap();
    let notes = Store {
        dir: dir.path().join("not-a-directory"),
    };
    fs::write(&notes.dir, "broken note store").unwrap();
    let mut owners = ownership();
    owners.lookup_errors.insert(
        "~/missing-project/.git".into(),
        "cannot canonicalize known project".into(),
    );
    owners.report_problems(|project, error| {
        problem_note(
            &notes,
            &format!("pagerduty-config:{project}"),
            Some(project),
            error,
        )
    });
    assert_eq!(owners.note_errors.len(), 1);
    let mut ops = Fake {
        ownership: owners,
        ..Default::default()
    };
    let path = state_file(&dir);
    let error = cycle(&path, 0, &mut ops).unwrap_err();
    assert!(error.contains("cannot persist PagerDuty board note"));
    assert_eq!(ops.mails.len(), 1);
    assert!(matches!(ops.mails[0].0, Destination::Mail { .. }));
    assert!(State::load(&path)
        .unwrap()
        .incidents
        .values()
        .next()
        .unwrap()
        .delivered
        .is_some());
    assert!(cycle(&path, 60, &mut ops).is_err());
    assert_eq!(ops.mails.len(), 1);
}

#[test]
fn equal_clone_ownership_coalesces_but_divergent_committed_sets_block() {
    let root = tempdir().unwrap();
    let a = root.path().join("a");
    let b = root.path().join("b");
    fs::create_dir(&a).unwrap();
    fs::create_dir(&b).unwrap();
    let paths = vec![mapped_project(&a), mapped_project(&b)];
    for dir in [&a, &b] {
        git(
            dir,
            &[
                "remote",
                "add",
                "origin",
                "https://github.com/team/shared.git",
            ],
        );
    }
    let equal = read_ownership(paths.clone(), |p| fs::canonicalize(p));
    assert_eq!(equal.projects.len(), 1);
    assert!(equal.invalid_configs.is_empty());
    assert_eq!(
        equal.owner(&incident("triggered", "t")).unwrap(),
        "github.com/team/shared"
    );
    fs::write(b.join(CONFIG), "services=['different-service']").unwrap();
    git(&b, &["add", "."]);
    git(&b, &["commit", "-qm", "change ownership"]);
    let mut divergent = read_ownership(paths, |p| fs::canonicalize(p));
    assert!(divergent.invalid_configs["github.com/team/shared"].contains("divergent"));
    assert!(divergent.owner(&incident("triggered", "t")).is_err());
    let mut notes = Vec::new();
    divergent.report_problems(|id, reason| {
        notes.push((id.to_owned(), reason.to_owned()));
        Ok(())
    });
    assert_eq!(notes[0].0, "github.com/team/shared");
    assert!(notes[0].1.contains("divergent"));
}

#[test]
fn migrated_config_notes_keep_their_original_cause_without_duplicate_attention() {
    let dir = tempdir().unwrap();
    let store = Store {
        dir: dir.path().into(),
    };
    let original_cause = "pagerduty-config:~/old/clone/.git";
    let id = "github.com/team/repo";
    problem_note(
        &store,
        original_cause,
        Some(id),
        "~/old/clone/.git: invalid TOML",
    )
    .unwrap();
    let original = store.list()[0].clone();
    problem_note(
        &store,
        &format!("pagerduty-config:{id}"),
        Some(id),
        &format!("{id}: invalid TOML"),
    )
    .unwrap();
    assert_eq!(store.list(), vec![original]);
    let own = Ownership {
        lookup_errors: [(id.into(), "cannot read one clone".into())].into(),
        projects: [(id.into(), ["Example Service".into()].into())].into(),
        ..Default::default()
    };
    assert!(
        own.owner(&incident("triggered", "t")).is_err(),
        "a healthy clone cannot hide another clone's lookup error"
    );
}

#[test]
fn a_registered_failed_clone_blocks_only_its_id_and_healthy_projects_still_mail() {
    let root = tempdir().unwrap();
    let shared = "github.com/team/shared";
    let healthy = "github.com/team/healthy";
    let mut locations = Vec::new();
    for (name, id, services) in [
        ("a", shared, "Example Service"),
        ("b", shared, "Different Service"),
        ("healthy", healthy, "Healthy Estate"),
    ] {
        let path = root.path().join(name);
        fs::create_dir(&path).unwrap();
        mapped_project(&path);
        git(
            &path,
            &["remote", "add", "origin", &format!("https://{id}.git")],
        );
        fs::write(path.join(CONFIG), format!("services=['{services}']")).unwrap();
        git(&path, &["add", "."]);
        git(
            &path,
            &["commit", "--allow-empty", "-qm", "ownership fixture"],
        );
        locations.push(OwnershipLocation {
            project: Some(id.into()),
            path: path.join(".git").canonicalize().unwrap(),
        });
    }
    let config = root.path().join("b/.git/config");
    let original = fs::read(&config).unwrap();
    for failure in ["resolve", "committed", "missing"] {
        let mut locations = locations.clone();
        fs::write(&config, &original).unwrap();
        if failure == "resolve" {
            fs::write(&config, "[broken configuration").unwrap();
        } else if failure == "committed" {
            fs::write(root.path().join("b").join(CONFIG), "not valid [TOML").unwrap();
            git(&root.path().join("b"), &["add", "."]);
            git(
                &root.path().join("b"),
                &["commit", "-qm", "invalid ownership"],
            );
        }
        if failure == "missing" {
            locations[1].path = root.path().join("missing/.git");
        }
        let ownership = read_ownership(locations, |path| fs::canonicalize(path));
        if failure == "missing" {
            assert_eq!(ownership.lookup_errors.len(), 1);
            assert!(ownership.lookup_errors.contains_key(shared));
            assert!(!ownership.lookup_errors.contains_key(healthy));
        }
        assert!(ownership.has_problem(shared));
        assert!(ownership.owner(&incident("triggered", "t")).is_err());
        let mut healthy_incident = incident("triggered", "t");
        healthy_incident.id = "healthy-incident".into();
        healthy_incident.service.id = "healthy-service".into();
        healthy_incident.service.summary = "Healthy Estate".into();
        assert_eq!(ownership.owner(&healthy_incident).unwrap(), healthy);
        let mut registry = view_for(shared, "ganymede", "shared-pane");
        registry
            .entries
            .extend(view_for(healthy, "ganymede", "healthy-pane").entries);
        let mut ops = Fake {
            incidents: vec![incident("triggered", "t"), healthy_incident],
            ownership,
            view: registry,
            ..Default::default()
        };
        assert!(cycle(
            &root.path().join(format!("{failure}-state.json")),
            0,
            &mut ops
        )
        .is_err());
        assert!(ops.mails.iter().any(|(to, _)| matches!(to, Destination::Mail { project, pane, .. } if project == healthy && pane == "healthy-pane")));
        assert!(!ops
            .mails
            .iter()
            .any(|(to, _)| matches!(to, Destination::Mail { project, .. } if project == shared)));
    }
}
