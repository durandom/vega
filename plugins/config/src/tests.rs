//! Units against a temp HOME with fake chezmoi and vega-skills.

use super::*;
use std::os::unix::fs::symlink;

fn script(d: &Path, name: &str, body: &str) -> PathBuf {
    let bin = d.join(name);
    fs::write(&bin, format!("#!/bin/sh\n{body}\n")).unwrap();
    fs::set_permissions(&bin, fs::Permissions::from_mode(0o755)).unwrap();
    bin
}

/// A HOME with claude, codex and pi set up, no chezmoi, no herdr-app.
fn setup() -> (tempfile::TempDir, Ctx) {
    let d = tempfile::tempdir().unwrap();
    let home = d.path().join("home");
    for dir in [".claude", ".codex", ".pi/agent", ".config"] {
        fs::create_dir_all(home.join(dir)).unwrap();
    }
    let skills = script(
        d.path(),
        "vega-skills",
        "echo \"$*\" >> \"$(dirname \"$0\")/skills-calls\"; echo 'skills: 2 up to date'",
    );
    let claude = script(
        d.path(),
        "claude",
        &format!(
            "exec python3 '{}' \"$@\"",
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("src/test_claude.py")
                .display()
        ),
    );
    let ctx = Ctx {
        env: Env::at(&home),
        chezmoi: d.path().join("no-chezmoi").into_os_string(),
        herdr_app: d.path().join("no-herdr-app").into_os_string(),
        mux: PathBuf::from("/opt/vega/bin/vega-mux"),
        skills: skills.into_os_string(),
        claude: claude.into_os_string(),
    };
    (d, ctx)
}

fn read(ctx: &Ctx, rel: &str) -> String {
    fs::read_to_string(ctx.env.home.join(rel)).unwrap()
}

fn apply_all(ctx: &Ctx) -> Vec<(Unit, Outcome)> {
    let managed = chezmoi_managed(&ctx.chezmoi).unwrap();
    Unit::ALL
        .iter()
        .map(|u| (*u, apply(ctx, *u, &managed)))
        .collect()
}

fn outcome(all: &[(Unit, Outcome)], u: Unit) -> &Outcome {
    &all.iter().find(|(x, _)| *x == u).unwrap().1
}

#[test]
fn dcg_units_override_backup_and_preserve_unmanaged_files() {
    let (_d, ctx) = setup();
    let target = ctx.env.config.join("dcg/packs");
    fs::create_dir_all(&target).unwrap();
    fs::write(target.join("other.yaml"), "unmanaged\n").unwrap();
    fs::write(target.join("oc.yaml"), "old\n").unwrap();
    fs::write(ctx.env.config.join("dcg/allowlist.toml"), "# local\n").unwrap();
    let local = sources::local_dir(&ctx.env).join("dcg/packs");
    fs::create_dir_all(&local).unwrap();
    fs::write(local.join("extra.yml"), "id: personal.extra\n").unwrap();
    let config = sources::init(&ctx.env, "dcg/config.toml").unwrap();
    fs::write(config, "[general]\nverbose = true\n").unwrap();
    let p = plan(&ctx, Unit::DcgConfig).unwrap();
    assert!(has_local(&p));
    assert!(p.files[0].text.contains("verbose = true"));
    let managed = BTreeSet::from([target.join("oc.yaml")]);
    assert!(matches!(
        apply(&ctx, Unit::DcgPacks, &managed),
        Outcome::Managed(_)
    ));
    assert_eq!(fs::read_to_string(target.join("oc.yaml")).unwrap(), "old\n");
    for unit in [Unit::DcgConfig, Unit::DcgPacks] {
        assert!(matches!(
            apply(&ctx, unit, &BTreeSet::new()),
            Outcome::Written(_)
        ));
        assert_eq!(apply(&ctx, unit, &BTreeSet::new()), Outcome::Unchanged);
    }
    assert_eq!(
        fs::read_to_string(target.join("oc.yaml.vega-bak")).unwrap(),
        "old\n"
    );
    assert_eq!(
        fs::read_to_string(target.join("other.yaml")).unwrap(),
        "unmanaged\n"
    );
    assert_eq!(
        fs::read_to_string(ctx.env.config.join("dcg/allowlist.toml")).unwrap(),
        "# local\n"
    );
    assert!(target.join("extra.yml").is_file());
}

#[test]
fn dcg_default_pack_paths_follow_xdg_targets() {
    let (_d, mut ctx) = setup();
    ctx.env.config = ctx.env.home.join("custom-config");
    let p = plan(&ctx, Unit::DcgConfig).unwrap();
    let doc = p.files[0].text.parse::<toml_edit::DocumentMut>().unwrap();
    let paths = doc["packs"]["custom_paths"].as_array().unwrap();
    assert_eq!(
        paths.get(0).unwrap().as_str().unwrap(),
        ctx.env.config.join("dcg/packs/*.yaml").to_str().unwrap()
    );
    assert_eq!(p.files[0].path, ctx.env.config.join("dcg/config.toml"));
    let packs = plan(&ctx, Unit::DcgPacks).unwrap();
    assert!(packs
        .files
        .iter()
        .all(|f| f.path.starts_with(ctx.env.config.join("dcg/packs"))));
}

#[test]
fn select_takes_units_and_groups_in_order() {
    let s = |n: &[&str]| select(&n.iter().map(|s| s.to_string()).collect::<Vec<_>>());
    assert_eq!(s(&[]).unwrap().len(), Unit::ALL.len());
    assert_eq!(
        s(&["hooks", "claude.hooks"]).unwrap(),
        [
            Unit::ClaudeHooks,
            Unit::CodexHooks,
            Unit::PiPrefix,
            Unit::PiDcg
        ]
    );
    assert!(s(&["herdr"]).is_err());
    assert!(s(&["nope"]).unwrap_err().contains("known: dcg.config"));
    assert!(
        GROUPS[1].1.iter().all(|u| !u.personal()),
        "hooks are not personal"
    );
}

#[test]
fn fresh_home_gets_everything_then_nothing_changes() {
    let (_d, ctx) = setup();
    fs::write(
        ctx.env.home.join(".pi/agent/settings.json"),
        "{\"theme\": \"x\"}",
    )
    .unwrap();
    let all = apply_all(&ctx);
    for u in [
        Unit::ClaudeMd,
        Unit::CodexMd,
        Unit::PiMd,
        Unit::ClaudeStyles,
        Unit::ClaudeSettings,
        Unit::ClaudeHooks,
        Unit::CodexHooks,
        Unit::PiPrefix,
    ] {
        assert!(
            matches!(outcome(&all, u), Outcome::Written(_)),
            "{u:?}: {:?}",
            outcome(&all, u)
        );
    }
    assert_eq!(
        outcome(&all, Unit::Skills),
        &Outcome::Skipped("no ~/.config/vega/skills-global".into())
    );
    assert!(matches!(
        outcome(&all, Unit::Agentsview),
        Outcome::Skipped(_)
    ));
    assert!(read(&ctx, ".claude/CLAUDE.md").starts_with("# Role: Active Partner\n"));
    assert_eq!(
        read(&ctx, ".claude/CLAUDE.md"),
        read(&ctx, ".pi/agent/AGENTS.md")
    );
    assert!(read(&ctx, ".codex/AGENTS.md").starts_with("# Git commits\n"));
    assert!(ctx.env.home.join(".claude/output-styles/ELI5.md").is_file());
    let settings: Value = serde_json::from_str(&read(&ctx, ".claude/settings.json")).unwrap();
    assert_eq!(settings["outputStyle"], "Caveman ELI5");
    assert!(merge::is_dcg_command(
        &settings["hooks"]["PreToolUse"][0]["hooks"][0]
    ));
    let s: Value = serde_json::from_str(&read(
        &ctx,
        ".local/share/vega/claude-marketplace/plugins/vega-hooks/hooks/hooks.json",
    ))
    .unwrap();
    assert_eq!(
        s["hooks"]["PreToolUse"][0]["hooks"][0]["command"],
        "'/opt/vega/bin/vega-mux' hook ask-parent"
    );
    assert_eq!(s["hooks"]["SessionStart"].as_array().unwrap().len(), 2);
    for event in [
        "SessionStart",
        "UserPromptSubmit",
        "Stop",
        "Notification",
        "SessionEnd",
    ] {
        assert!(
            s["hooks"][event].as_array().unwrap().iter().any(|entry| {
                entry["hooks"][0]["command"]
                    .as_str()
                    .is_some_and(|c| c.ends_with("hook state"))
            }),
            "missing state hook for {event}"
        );
    }
    let c: Value = serde_json::from_str(&read(&ctx, ".codex/hooks.json")).unwrap();
    assert_eq!(
        c["hooks"]["PreToolUse"][0]["matcher"],
        "request_user_input|request_user_input_async"
    );
    let p: Value = serde_json::from_str(&read(&ctx, ".pi/agent/settings.json")).unwrap();
    assert_eq!(p["theme"], "x");
    assert_eq!(p["shellCommandPrefix"], merge::ACTIVATE_LINE);
    // Second run: nothing to do; every state ok.
    for (u, o) in apply_all(&ctx) {
        assert!(
            matches!(o, Outcome::Unchanged | Outcome::Skipped(_)),
            "{u:?}: {o:?}"
        );
        let p = plan(&ctx, u).unwrap();
        assert!(
            matches!(state(&p, &[]).as_str(), "ok") || p.skip.is_some(),
            "{u:?}"
        );
    }
}

#[test]
fn missing_tool_dirs_skip_and_pi_needs_its_settings() {
    let (_d, ctx) = setup();
    fs::remove_dir_all(ctx.env.home.join(".codex")).unwrap();
    let all = apply_all(&ctx);
    assert_eq!(
        outcome(&all, Unit::CodexMd),
        &Outcome::Skipped("no ~/.codex".into())
    );
    assert_eq!(
        outcome(&all, Unit::CodexHooks),
        &Outcome::Skipped("no ~/.codex".into())
    );
    assert_eq!(
        outcome(&all, Unit::PiPrefix),
        &Outcome::Skipped("no ~/.pi/agent/settings.json".into())
    );
    assert!(!ctx.env.home.join(".codex").exists());
}

#[test]
fn chezmoi_managed_targets_are_refused_others_apply() {
    let (d, mut ctx) = setup();
    let claude_md = ctx.env.home.join(".claude/CLAUDE.md");
    fs::write(&claude_md, "chezmoi's\n").unwrap();
    let chezmoi = script(
        d.path(),
        "chezmoi",
        &format!(
            "echo 'warning: template changed' >&2\necho '{}'\necho '{}/.gitconfig'",
            claude_md.display(),
            ctx.env.home.display()
        ),
    );
    ctx.chezmoi = chezmoi.into_os_string();
    let all = apply_all(&ctx);
    assert_eq!(
        outcome(&all, Unit::ClaudeMd),
        &Outcome::Managed(vec![claude_md.clone()])
    );
    assert_eq!(fs::read_to_string(&claude_md).unwrap(), "chezmoi's\n");
    assert!(matches!(outcome(&all, Unit::CodexMd), Outcome::Written(_)));
    let p = plan(&ctx, Unit::ClaudeMd).unwrap();
    let m = managed_targets(&p, &chezmoi_managed(&ctx.chezmoi).unwrap());
    assert_eq!(state(&p, &m), "managed (chezmoi)");
}

#[test]
fn failing_chezmoi_is_an_error() {
    let d = tempfile::tempdir().unwrap();
    let c = script(
        d.path(),
        "chezmoi",
        "echo 'chezmoi: broken source' >&2; exit 1",
    );
    assert_eq!(
        chezmoi_managed(c.as_os_str()).unwrap_err(),
        "chezmoi managed failed: chezmoi: broken source"
    );
}

#[test]
fn whole_file_replaces_a_symlink_and_backs_up() {
    let (_d, ctx) = setup();
    let home = &ctx.env.home;
    // chezmoi's pi link to CLAUDE.md, whose content already matches.
    let (text, _) = sources::instructions(&ctx.env, &sources::CLAUDE_PARTS).unwrap();
    fs::write(home.join(".claude/CLAUDE.md"), &text).unwrap();
    symlink(
        home.join(".claude/CLAUDE.md"),
        home.join(".pi/agent/AGENTS.md"),
    )
    .unwrap();
    let p = plan(&ctx, Unit::PiMd).unwrap();
    assert_eq!(state(&p, &[]), "differs");
    assert!(matches!(
        apply(&ctx, Unit::PiMd, &BTreeSet::new()),
        Outcome::Written(_)
    ));
    let pi = home.join(".pi/agent/AGENTS.md");
    assert!(!fs::symlink_metadata(&pi).unwrap().file_type().is_symlink());
    assert_eq!(fs::read_to_string(&pi).unwrap(), text);
    assert_eq!(fs::read_to_string(backup_path(&pi)).unwrap(), text);
    // Edited target: replaced, the edit in the backup.
    fs::write(home.join(".claude/CLAUDE.md"), "mine\n").unwrap();
    assert_eq!(
        apply(&ctx, Unit::ClaudeMd, &BTreeSet::new()).kind(),
        "written"
    );
    assert_eq!(read(&ctx, ".claude/CLAUDE.md"), text);
    assert_eq!(read(&ctx, ".claude/CLAUDE.md.vega-bak"), "mine\n");
}

#[test]
fn settings_merge_keeps_tool_state_and_local_seeds() {
    let (_d, ctx) = setup();
    let path = ctx.env.home.join(".claude/settings.json");
    let live = "{\n  \"enabledPlugins\": {\"a\": true},\n  \"outputStyle\": \"Concise\",\n  \"includeCoAuthoredBy\": false,\n  \"autoCompactEnabled\": true,\n  \"cleanupPeriodDays\": 3650\n}\n";
    fs::write(&path, live).unwrap();
    // No record yet: the source value is asserted once (run_onchange's first run).
    assert_eq!(
        apply(&ctx, Unit::ClaudeSettings, &BTreeSet::new()).kind(),
        "written"
    );
    let v: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(v["outputStyle"], "Caveman ELI5");
    assert_eq!(v["enabledPlugins"]["a"], true);
    // The user switches back: it stays, reported as local.
    let mut v2 = v.clone();
    v2["outputStyle"] = "Concise".into();
    fs::write(&path, serde_json::to_string(&v2).unwrap()).unwrap();
    let p = plan(&ctx, Unit::ClaudeSettings).unwrap();
    assert_eq!(state(&p, &[]), "ok");
    assert_eq!(p.notes.len(), 1);
    assert!(
        p.notes[0].starts_with("local: outputStyle = \"Concise\""),
        "{:?}",
        p.notes
    );
    assert_eq!(
        apply(&ctx, Unit::ClaudeSettings, &BTreeSet::new()),
        Outcome::Unchanged
    );
    // Hooks share the file and keep the seeds.
    assert_eq!(
        apply(&ctx, Unit::ClaudeHooks, &BTreeSet::new()).kind(),
        "written"
    );
    let v: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(v["outputStyle"], "Concise");
    assert!(merge::is_dcg_command(
        &v["hooks"]["PreToolUse"][0]["hooks"][0]
    ));
    assert_eq!(v["enabledPlugins"]["vega-hooks@vega-local"], true);
    // A file that does not parse stays untouched.
    fs::write(&path, "not json").unwrap();
    assert!(matches!(
        apply(&ctx, Unit::ClaudeHooks, &BTreeSet::new()),
        Outcome::Failed(_)
    ));
    assert_eq!(fs::read_to_string(&path).unwrap(), "not json");
}

#[test]
fn local_overrides_win_and_show() {
    let (_d, ctx) = setup();
    sources::init(&ctx.env, "instructions/git-commits.md").unwrap();
    let local = sources::local_dir(&ctx.env).join("instructions/git-commits.md");
    fs::write(&local, "# Commits\nmine\n").unwrap();
    let p = plan(&ctx, Unit::CodexMd).unwrap();
    assert!(has_local(&p));
    assert!(!has_local(&plan(&ctx, Unit::ClaudeMd).unwrap()));
    apply(&ctx, Unit::CodexMd, &BTreeSet::new());
    assert!(read(&ctx, ".codex/AGENTS.md").starts_with("# Commits\nmine\n\n# vega-mux"));
}

#[test]
fn agentsview_sidecar_only_next_to_a_generated_skill() {
    let (_d, ctx) = setup();
    let dir = ctx
        .env
        .home
        .join(".claude/skills/agentsview-finding-history");
    fs::create_dir_all(&dir).unwrap();
    assert!(plan(&ctx, Unit::Agentsview).unwrap().skip.is_some());
    fs::write(dir.join("SKILL.md"), "generated\n").unwrap();
    assert_eq!(
        apply(&ctx, Unit::Agentsview, &BTreeSet::new()).kind(),
        "written"
    );
    assert!(read(
        &ctx,
        ".claude/skills/agentsview-finding-history/LOCAL-SETUP.md"
    )
    .starts_with("# Local setup"));
    assert!(!ctx
        .env
        .home
        .join(".agents/skills/agentsview-finding-history")
        .exists());
    assert_eq!(
        read(&ctx, ".claude/skills/agentsview-finding-history/SKILL.md"),
        "generated\n"
    );
}

#[test]
fn skills_report_missing_warn_on_overlap_and_sync() {
    let (d, ctx) = setup();
    let home = &ctx.env.home;
    fs::create_dir_all(ctx.env.config.join("vega")).unwrap();
    fs::write(
        ctx.env.config.join("vega/skills-global"),
        "axi\n# note\nbeads\n",
    )
    .unwrap();
    fs::create_dir_all(home.join(".agents/skills/axi")).unwrap();
    fs::write(home.join(".agents/skills/axi/SKILL.md"), "x").unwrap();
    fs::create_dir_all(home.join(".claude/skills")).unwrap();
    symlink("../../.agents/skills/axi", home.join(".claude/skills/axi")).unwrap();
    let p = plan(&ctx, Unit::Skills).unwrap();
    assert_eq!(state(&p, &[]), "missing: beads");
    let managed: BTreeSet<PathBuf> = [
        home.join(".claude/skills/axi/SKILL.md"),
        home.join(".zshrc"),
    ]
    .into();
    assert_eq!(
        skills_overlap(&p, &managed, home),
        [home.join(".claude/skills/axi/SKILL.md")]
    );
    // An overlap is no refusal: the sync runs.
    assert_eq!(
        apply(&ctx, Unit::Skills, &managed),
        Outcome::Written(vec!["vega skills lib sync: skills: 2 up to date".into()])
    );
    assert_eq!(
        fs::read_to_string(d.path().join("skills-calls")).unwrap(),
        "lib sync\n"
    );
}

#[test]
fn evidence_attributes_repair_and_source_change_without_values_and_refreshes_backup() {
    let (_d, ctx) = setup();
    let src = sources::init(&ctx.env, "claude/settings.json").unwrap();
    fs::write(&src, r#"{"outputStyle":"SOURCE_SECRET"}"#).unwrap();
    let target = ctx.env.home.join(".claude/settings.json");
    fs::write(
        &target,
        r#"{"env":{"TOKEN":"USER_SECRET"},"outputStyle":"OLD_SECRET"}"#,
    )
    .unwrap();
    let before = fs::read_to_string(&target).unwrap();
    let (out, first) = apply_report(&ctx, Unit::ClaudeSettings, &BTreeSet::new());
    assert_eq!(out.kind(), "written");
    assert_eq!(fs::read_to_string(backup_path(&target)).unwrap(), before);
    let first_key = first["writes"][0]["keys"]
        .as_array()
        .unwrap()
        .iter()
        .find(|k| k["key"] == "/outputStyle")
        .unwrap();
    assert_eq!(first_key["key"], "/outputStyle");
    assert_eq!(first_key["operation"], "changed");
    assert_eq!(first_key["source_changed"], true);
    let repaired = r#"{"env":{"TOKEN":"USER_SECRET"}}"#;
    fs::write(&target, repaired).unwrap();
    let (_, second) = apply_report(&ctx, Unit::ClaudeSettings, &BTreeSet::new());
    assert_eq!(fs::read_to_string(backup_path(&target)).unwrap(), repaired);
    let second_key = second["writes"][0]["keys"]
        .as_array()
        .unwrap()
        .iter()
        .find(|k| k["key"] == "/outputStyle")
        .unwrap();
    assert_eq!(second_key["operation"], "added");
    assert_eq!(second_key["reason"], "missing_key");
    assert_eq!(second_key["source_changed"], false);
    assert!(second["at_unix_ms"].as_u64().unwrap() >= first["at_unix_ms"].as_u64().unwrap());
    let text = fs::read_to_string(evidence::path(&ctx.env)).unwrap();
    let events: Vec<Value> = serde_json::from_str(&text).unwrap();
    assert_eq!(events, [first, second]);
    for forbidden in [
        "USER_SECRET",
        "SOURCE_SECRET",
        "OLD_SECRET",
        "TOKEN",
        "/env",
    ] {
        assert!(!text.contains(forbidden), "leaked {forbidden}");
    }
}

#[test]
fn hook_evidence_is_redacted_and_symlink_backup_refreshes_at_referent() {
    let (d, ctx) = setup();
    let target = ctx.env.home.join(".claude/settings.json");
    let real = d.path().join("settings.json");
    let mut legacy = serde_json::json!({"env":{"TOKEN":"USER_SECRET"},"hooks":{"Stop":[{"command":"SECRET_COMMAND"}]}});
    merge::claude_hooks(&mut legacy, "/old/vega-mux").unwrap();
    let before = serde_json::to_string(&legacy).unwrap();
    fs::write(&real, &before).unwrap();
    symlink(&real, &target).unwrap();
    let (_, event) = apply_report(&ctx, Unit::ClaudeHooks, &BTreeSet::new());
    let backup: Value =
        serde_json::from_str(&fs::read_to_string(backup_path(&real)).unwrap()).unwrap();
    assert_eq!(backup["hooks"], legacy["hooks"]);
    assert_eq!(backup["env"], legacy["env"]);
    assert_eq!(backup["enabledPlugins"]["vega-hooks@vega-local"], true);
    let writes = event["writes"].as_array().unwrap();
    assert_eq!(
        writes.len(),
        5,
        "three plugin files, CLI registration, legacy migration"
    );
    let registration = writes
        .iter()
        .find(|w| w["writer"] == "claude_plugin_cli")
        .unwrap();
    assert!(registration["backup_refreshed"].is_null());
    assert_eq!(
        registration["keys"][0]["key"],
        "/enabledPlugins/vega-hooks@vega-local"
    );
    assert!(writes.last().unwrap()["backup_refreshed"]
        .as_str()
        .unwrap()
        .ends_with("settings.json.vega-bak"));
    assert!(!backup_path(&target).exists());
    let text = event.to_string();
    assert!(text.contains("/hooks/PreToolUse"));
    for forbidden in [
        "USER_SECRET",
        "SECRET_COMMAND",
        "ask-parent",
        "Stop",
        "TOKEN",
    ] {
        assert!(!text.contains(forbidden));
    }
    let edited = before.replace("USER_SECRET", "NEXT_SECRET");
    fs::write(&real, &edited).unwrap();
    assert_eq!(
        apply(&ctx, Unit::ClaudeHooks, &BTreeSet::new()).kind(),
        "written"
    );
    assert_eq!(fs::read_to_string(backup_path(&real)).unwrap(), edited);
}

#[test]
fn evidence_is_bounded_and_keeps_results_without_error_text() {
    let (_d, ctx) = setup();
    // Seed a full history in one write: 260 retains would cost 260 fsyncs
    // (F_FULLFSYNC on macOS, seconds in all).
    let seed: Vec<Value> = (0..256).map(|i| serde_json::json!({ "seed": i })).collect();
    let p = evidence::path(&ctx.env);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(&p, serde_json::to_string(&seed).unwrap()).unwrap();
    for _ in 0..4 {
        evidence::retain(
            &ctx.env,
            &evidence::event(Unit::ClaudeMd, "unchanged", vec![]),
        )
        .unwrap();
    }
    let text = fs::read_to_string(evidence::path(&ctx.env)).unwrap();
    let events = serde_json::from_str::<Vec<Value>>(&text).unwrap();
    assert_eq!(events.len(), 256);
    // The oldest go first: seeds 0..4 dropped, the new events last.
    assert_eq!(events[0]["seed"], 4);
    assert!(events[252..].iter().all(|e| e["result"] == "unchanged"));
    assert_eq!(
        fs::metadata(evidence::path(&ctx.env))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
}

#[test]
fn evidence_retains_successful_writes_before_a_later_file_fails() {
    let (_d, ctx) = setup();
    for dir in AGENTSVIEW_DIRS {
        let dir = ctx.env.home.join(dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("SKILL.md"), "fixture").unwrap();
        fs::write(dir.join("LOCAL-SETUP.md"), "old").unwrap();
    }
    // A backup path that cannot be written causes the second file to fail,
    // independent of UID/permission behavior in the test environment.
    let second = ctx.env.home.join(AGENTSVIEW_DIRS[1]).join("LOCAL-SETUP.md");
    fs::create_dir(backup_path(&second)).unwrap();
    let (out, event) = apply_report(&ctx, Unit::Agentsview, &BTreeSet::new());
    assert_eq!(out.kind(), "failed");
    assert_eq!(event["result"], "failed");
    let writes = event["writes"].as_array().unwrap();
    assert_eq!(writes.len(), 1);
    assert_eq!(
        writes[0]["target"],
        "~/.claude/skills/agentsview-finding-history/LOCAL-SETUP.md"
    );
    assert_eq!(fs::read_to_string(second).unwrap(), "old");
    let history: Vec<Value> =
        serde_json::from_str(&fs::read_to_string(evidence::path(&ctx.env)).unwrap()).unwrap();
    assert_eq!(history, [event]);
}

#[test]
fn claude_plugin_migration_waits_for_enable_and_preserves_mixed_commands() {
    let (_d, ctx) = setup();
    let mut value = serde_json::json!({"permissions":{"allow":["Read"]}, "hooks": {
        "PreToolUse":[{"matcher":"Bash","hooks":[{"type":"command","command":"dcg"}]},
            {"matcher":"AskUserQuestion","hooks":[{"type":"command","command":"another hook ask-parent"}]}],
        "SessionStart":[{"hooks":[]}]}});
    merge::claude_hooks(&mut value, "/old/vega-mux").unwrap();
    value["hooks"]["PreToolUse"][1]["hooks"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({"type":"command","command":"unrelated"}));
    // Add an unrelated command to the entry the old installer owned.
    value["hooks"]["SessionStart"][0]["hooks"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({"type":"command","command":"moshi-hook"}));
    let path = ctx.env.home.join(".claude/settings.json");
    fs::write(&path, serde_json::to_string(&value).unwrap()).unwrap();
    let original = fs::read_to_string(&path).unwrap();
    // Preview and a CLI failure cannot remove hooks.
    assert!(plan(&ctx, Unit::ClaudeHooks).unwrap().plugin_pending);
    assert_eq!(fs::read_to_string(&path).unwrap(), original);
    fs::write(ctx.env.home.join("fail-claude"), "").unwrap();
    assert!(apply(&ctx, Unit::ClaudeHooks, &BTreeSet::new()).failed());
    assert_eq!(fs::read_to_string(&path).unwrap(), original);
    fs::remove_file(ctx.env.home.join("fail-claude")).unwrap();
    // An install that exits successfully but leaves the plugin disabled is insufficient.
    fs::write(ctx.env.home.join("inactive-claude"), "").unwrap();
    assert!(apply(&ctx, Unit::ClaudeHooks, &BTreeSet::new()).failed());
    let before: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(before["hooks"], value["hooks"]);
    fs::remove_file(ctx.env.home.join("inactive-claude")).unwrap();
    assert!(matches!(
        apply(&ctx, Unit::ClaudeHooks, &BTreeSet::new()),
        Outcome::Written(_)
    ));
    let migrated: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    let mut expected = value.clone();
    merge::remove_claude_hooks(&mut expected).unwrap();
    assert_eq!(migrated["hooks"], expected["hooks"]);
    assert_eq!(migrated["permissions"], value["permissions"]);
    assert_eq!(
        migrated["cliWritten"], true,
        "re-read settings after install"
    );
    assert_eq!(
        migrated["hooks"]["SessionStart"][0]["hooks"][0]["command"],
        "moshi-hook"
    );
    assert_eq!(
        apply(&ctx, Unit::ClaudeHooks, &BTreeSet::new()),
        Outcome::Unchanged
    );
}

#[test]
fn claude_plugin_repairs_disabled_missing_and_updated_hooks() {
    let (_d, mut ctx) = setup();
    assert!(!apply(&ctx, Unit::ClaudeHooks, &BTreeSet::new()).failed());
    let state = ctx.env.home.join("fake-plugins.json");
    let mut list: Value = serde_json::from_str(&fs::read_to_string(&state).unwrap()).unwrap();
    list[0]["enabled"] = false.into();
    fs::write(&state, serde_json::to_string(&list).unwrap()).unwrap();
    assert!(plan(&ctx, Unit::ClaudeHooks).unwrap().plugin_pending);
    assert!(!apply(&ctx, Unit::ClaudeHooks, &BTreeSet::new()).failed());
    ctx.mux = "/new/vega-mux".into();
    assert!(plan(&ctx, Unit::ClaudeHooks).unwrap().plugin_pending);
    assert!(!apply(&ctx, Unit::ClaudeHooks, &BTreeSet::new()).failed());
    let hooks = ctx
        .env
        .home
        .join(".local/share/vega/claude-marketplace/plugins/vega-hooks/hooks/hooks.json");
    fs::remove_file(hooks).unwrap();
    assert!(plan(&ctx, Unit::ClaudeHooks).unwrap().plugin_pending);
    assert!(!apply(&ctx, Unit::ClaudeHooks, &BTreeSet::new()).failed());
    // A managed settings file blocks CLI writes, including on a fresh host.
    fs::remove_file(ctx.env.home.join(".claude/settings.json")).unwrap();
    let managed = BTreeSet::from([ctx.env.home.join(".claude/settings.json")]);
    assert!(matches!(
        apply(&ctx, Unit::ClaudeHooks, &managed),
        Outcome::Managed(_)
    ));
}

#[test]
fn plugin_evidence_keeps_partial_cli_writes_and_retains_legacy_hooks_on_failure() {
    let (_d, ctx) = setup();
    let mut legacy = serde_json::json!({"env":{"TOKEN":"SECRET_VALUE"}});
    merge::claude_hooks(&mut legacy, "/old/vega-mux").unwrap();
    let path = ctx.env.home.join(".claude/settings.json");
    fs::write(&path, serde_json::to_string(&legacy).unwrap()).unwrap();
    fs::write(ctx.env.home.join("fail-registration"), "").unwrap();
    let (outcome, event) = apply_report(&ctx, Unit::ClaudeHooks, &BTreeSet::new());
    assert!(outcome.failed());
    let writes = event["writes"].as_array().unwrap();
    assert_eq!(
        writes.len(),
        4,
        "source writes and the CLI's partial registration must survive"
    );
    assert_eq!(writes[3]["writer"], "claude_plugin_cli");
    assert_eq!(
        writes[3]["keys"][0]["key"],
        "/enabledPlugins/vega-hooks@vega-local"
    );
    assert!(writes[3]["backup_refreshed"].is_null());
    let live: Value = serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap();
    assert_eq!(
        live["hooks"], legacy["hooks"],
        "migration waits for successful activation"
    );
    let text = event.to_string();
    for forbidden in ["SECRET_VALUE", "TOKEN", "cliWritten", "ask-parent"] {
        assert!(!text.contains(forbidden), "leaked {forbidden}");
    }
    let history: Vec<Value> =
        serde_json::from_str(&fs::read_to_string(evidence::path(&ctx.env)).unwrap()).unwrap();
    assert_eq!(history, [event]);
}

#[test]
fn herdr_app_units_skip_write_seed_and_reject() {
    let (d, mut ctx) = setup();
    let all = apply_all(&ctx);
    for u in [
        Unit::HerdrAppKeys,
        Unit::HerdrAppSettings,
        Unit::HerdrAppThemes,
    ] {
        assert!(
            matches!(outcome(&all, u), Outcome::Skipped(r) if r.contains("herdr-app")),
            "{u:?}"
        );
    }
    let home = herdr_app::home(&ctx.env);
    fs::create_dir_all(&home).unwrap();
    // The user's own settings key survives; the seeded ones are set.
    fs::write(
        home.join("settings.json"),
        "{\"font\": \"Mine\", \"zoom\": 1}\n",
    )
    .unwrap();
    let managed = BTreeSet::new();
    for u in [
        Unit::HerdrAppKeys,
        Unit::HerdrAppSettings,
        Unit::HerdrAppThemes,
    ] {
        assert_eq!(apply(&ctx, u, &managed).kind(), "written", "{u:?}");
        assert_eq!(apply(&ctx, u, &managed), Outcome::Unchanged, "{u:?}");
    }
    let s: Value = serde_json::from_str(&read(&ctx, ".config/herdr-app/settings.json")).unwrap();
    assert_eq!(s["font"], "Mine");
    assert_eq!(s["theme"]["dark"], "catppuccin-mocha");
    assert_eq!(s["sounds"]["enabled"], false);
    assert!(home.join("themes/catppuccin-latte.json").is_file());
    assert!(read(&ctx, ".config/herdr-app/keys.json").contains("\"ctrl-s ctrl-s\""));
    // A theme set in the app's Settings screen stays until the source changes.
    fs::write(
        home.join("settings.json"),
        "{\"font\": \"Mine\", \"theme\": \"paper\", \"sounds\": {\"enabled\": false}}\n",
    )
    .unwrap();
    assert_eq!(
        apply(&ctx, Unit::HerdrAppSettings, &managed),
        Outcome::Unchanged
    );
    // An unknown command is rejected before writing, with herdr-app present.
    ctx.herdr_app = script(
        d.path(),
        "herdr-app",
        "echo '[{\"name\":\"tab.new\",\"args\":[]}]'",
    )
    .into_os_string();
    let local = sources::local_dir(&ctx.env).join("herdr-app");
    fs::create_dir_all(&local).unwrap();
    fs::write(
        local.join("keys.json"),
        "{\"global\": {\"ctrl-s c\": \"tab.new\", \"ctrl-s q\": \"nope.quit\"}}\n",
    )
    .unwrap();
    assert_eq!(
        apply(&ctx, Unit::HerdrAppKeys, &managed),
        Outcome::Rejected(vec!["global ctrl-s q: unknown command nope.quit".into()])
    );
    assert!(read(&ctx, ".config/herdr-app/keys.json").contains("\"ctrl-s ctrl-s\""));
    fs::write(
        local.join("keys.json"),
        "{\"global\": {\"ctrl-s c\": \"tab.new\"}}\n",
    )
    .unwrap();
    assert_eq!(apply(&ctx, Unit::HerdrAppKeys, &managed).kind(), "written");
}

fn fake_app(d: &Path) -> OsString {
    script(
        d,
        "app",
        // inline-md: allow (fake app executable, not help text)
        r#"
case "$1" in
  --version) echo 'herdr-app 0.1.14';;
  check)
    test -f "$2/main.js" && test -f "$2/mod.json" || exit 1
    test "$2" != "$HERDR_APP_HOME/mods/vega" || exit 1
    echo generated > "$2/gpui-kit.d.ts"
    echo generated > "$2/jsconfig.json"
    if grep -q INVALID "$2/main.js"; then echo 'invalid mod'; exit 1; fi
    echo 'check passed';;
  status)
    echo status >> "$HERDR_APP_HOME/layout-calls"
    if test -f "$HERDR_APP_HOME/layout-error"; then echo '{"layout":"vega","errors":["rejected update"]}'; exit 0; fi
    if test -f "$HERDR_APP_HOME/offline"; then echo '{"error":"not running"}'; exit 1; fi
    if test -f "$HERDR_APP_HOME/selected"; then echo '{"layout":"vega","errors":[]}'
    else echo '{"layout":"main","errors":[]}'; fi;;
  do)
    echo "$*" >> "$HERDR_APP_HOME/layout-calls"
    rm -f "$HERDR_APP_HOME/layout-error"
    if test "$4" = main; then
      rm -f "$HERDR_APP_HOME/selected"
      echo '{"ok":true}'; exit 0
    fi
    if test -f "$HERDR_APP_HOME/reject"; then
      if test -f "$HERDR_APP_HOME/selected"; then rm "$HERDR_APP_HOME/reject"; fi
      touch "$HERDR_APP_HOME/layout-error"
      echo '{"ok":true}'; exit 0
    fi
    if test -f "$HERDR_APP_HOME/mod-pending"; then
      rm "$HERDR_APP_HOME/mod-pending"
      echo '{"error":"mod vega: no mod folder"}'; exit 1
    fi
    if test -f "$HERDR_APP_HOME/mod-missing"; then
      echo '{"error":"mod vega: no mod folder"}'; exit 1
    fi
    test -f "$HERDR_APP_HOME/layouts/vega.json" || exit 1
    echo selected > "$HERDR_APP_HOME/selected"
    echo '{"ok":true}';;
  *) exit 1;;
esac
"#,
    )
    .into_os_string()
}

#[test]
fn app_mod_stages_rejects_and_replaces_the_whole_folder() {
    let (d, mut ctx) = setup();
    ctx.herdr_app = fake_app(d.path());
    let dir = herdr_app::home(&ctx.env).join("mods/vega");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("main.js"), "old").unwrap();
    fs::write(dir.join("stale.js"), "stale").unwrap();
    let local = sources::init(&ctx.env, "herdr-app/mods/vega/main.js").unwrap();
    fs::write(&local, "INVALID").unwrap();
    assert!(matches!(
        apply(&ctx, Unit::HerdrAppMods, &BTreeSet::new()),
        Outcome::Rejected(_)
    ));
    assert_eq!(fs::read_to_string(dir.join("main.js")).unwrap(), "old");
    assert!(dir.join("stale.js").exists());
    fs::write(
        &local,
        sources::default("herdr-app/mods/vega/main.js").unwrap(),
    )
    .unwrap();
    let managed = BTreeSet::from([dir.join("stale.js")]);
    assert!(matches!(
        apply(&ctx, Unit::HerdrAppMods, &managed),
        Outcome::Managed(_)
    ));
    assert_eq!(
        apply(&ctx, Unit::HerdrAppMods, &BTreeSet::new()).kind(),
        "written"
    );
    assert!(!dir.join("stale.js").exists());
    assert_eq!(
        fs::read_to_string(dir.join("lock.svg")).unwrap(),
        sources::default("herdr-app/mods/vega/lock.svg").unwrap()
    );
    assert!(!dir.join("gpui-kit.d.ts").exists());
    assert!(!dir.join("jsconfig.json").exists());
    assert!(herdr_app::home(&ctx.env)
        .join(".vega-mod.vega-bak/stale.js")
        .exists());
    assert_eq!(
        apply(&ctx, Unit::HerdrAppMods, &BTreeSet::new()),
        Outcome::Unchanged
    );
    // App-generated editor files are app-owned: no drift, and they survive a replacement.
    fs::write(dir.join("gpui-kit.d.ts"), "app types").unwrap();
    fs::write(dir.join("jsconfig.json"), "app config").unwrap();
    assert_eq!(
        apply(&ctx, Unit::HerdrAppMods, &BTreeSet::new()),
        Outcome::Unchanged
    );
    fs::write(dir.join("main.js"), "edited").unwrap();
    assert_eq!(
        apply(&ctx, Unit::HerdrAppMods, &BTreeSet::new()).kind(),
        "written"
    );
    assert_eq!(
        fs::read_to_string(dir.join("gpui-kit.d.ts")).unwrap(),
        "app types"
    );
    assert_eq!(
        fs::read_to_string(dir.join("jsconfig.json")).unwrap(),
        "app config"
    );
    // Drift consisting only of an extra file still replaces the folder.
    fs::write(dir.join("extra"), "extra").unwrap();
    assert_eq!(
        apply(&ctx, Unit::HerdrAppMods, &BTreeSet::new()).kind(),
        "written"
    );
    assert!(!dir.join("extra").exists());
    let manifest = sources::init(&ctx.env, "herdr-app/mods/vega/mod.json").unwrap();
    fs::write(manifest, "not JSON").unwrap();
    assert!(matches!(
        apply(&ctx, Unit::HerdrAppMods, &BTreeSet::new()),
        Outcome::Rejected(_)
    ));
    assert_eq!(
        fs::read_to_string(dir.join("mod.json")).unwrap(),
        "{ \"place\": \"none\" }\n"
    );
}

#[test]
fn app_mod_without_checker_never_writes() {
    let (_d, ctx) = setup();
    fs::create_dir_all(herdr_app::home(&ctx.env)).unwrap();
    assert!(matches!(
        apply(&ctx, Unit::HerdrAppMods, &BTreeSet::new()),
        Outcome::Rejected(_)
    ));
    assert!(!herdr_app::home(&ctx.env).join("mods/vega").exists());
}

#[test]
fn app_layout_selects_once_via_supported_command_and_rejects_false_success() {
    let (d, mut ctx) = setup();
    ctx.herdr_app = fake_app(d.path());
    let home = herdr_app::home(&ctx.env);
    fs::create_dir_all(&home).unwrap();
    let managed = BTreeSet::new();
    assert!(matches!(
        apply(&ctx, Unit::HerdrAppLayouts, &managed),
        Outcome::Rejected(_)
    ));
    for unit in [
        Unit::HerdrAppMods,
        Unit::HerdrAppSidebar,
        Unit::HerdrAppInbox,
    ] {
        assert_eq!(apply(&ctx, unit, &managed).kind(), "written");
    }
    assert_eq!(
        fs::read_to_string(home.join("mods/vega-sidebar/pin.svg")).unwrap(),
        sources::default("herdr-app/mods/vega-sidebar/pin.svg").unwrap()
    );
    fs::write(home.join("offline"), "").unwrap();
    assert!(matches!(
        apply(&ctx, Unit::HerdrAppLayouts, &managed),
        Outcome::Rejected(_)
    ));
    assert!(!home.join("layouts/vega.json").exists());
    fs::remove_file(home.join("offline")).unwrap();
    fs::write(home.join("reject"), "").unwrap();
    assert!(matches!(
        apply(&ctx, Unit::HerdrAppLayouts, &managed),
        Outcome::Rejected(_)
    ));
    assert!(!home.join("layouts/vega.json").exists());
    fs::remove_file(home.join("reject")).unwrap();
    assert_eq!(
        apply(&ctx, Unit::HerdrAppLayouts, &managed).kind(),
        "written"
    );
    assert!(home.join("selected").exists());
    // A later user layout choice is not overwritten on apply.
    fs::remove_file(home.join("selected")).unwrap();
    assert_eq!(
        apply(&ctx, Unit::HerdrAppLayouts, &managed),
        Outcome::Unchanged
    );
    assert!(!home.join("selected").exists());
    let local = sources::init(&ctx.env, "herdr-app/layouts/vega.json").unwrap();
    fs::write(local, "{broken").unwrap();
    let before = fs::read(home.join("layouts/vega.json")).unwrap();
    assert!(matches!(
        apply(&ctx, Unit::HerdrAppLayouts, &managed),
        Outcome::Rejected(_)
    ));
    assert_eq!(fs::read(home.join("layouts/vega.json")).unwrap(), before);
}

#[test]
fn app_selected_layout_update_reverifies_and_restores_on_false_success() {
    let (d, mut ctx) = setup();
    ctx.herdr_app = fake_app(d.path());
    let home = herdr_app::home(&ctx.env);
    fs::create_dir_all(&home).unwrap();
    let managed = BTreeSet::new();
    for unit in [
        Unit::HerdrAppMods,
        Unit::HerdrAppSidebar,
        Unit::HerdrAppInbox,
    ] {
        assert_eq!(apply(&ctx, unit, &managed).kind(), "written");
    }
    assert_eq!(
        apply(&ctx, Unit::HerdrAppLayouts, &managed).kind(),
        "written"
    );
    assert!(ctx
        .env
        .state
        .join("vega/herdr-app-layout-selected")
        .exists());
    assert!(home.join("selected").exists());
    // Only existence is checked: no live app or actual socket is needed.
    fs::write(home.join("herdr-app.sock"), "").unwrap();
    let local = sources::init(&ctx.env, "herdr-app/layouts/vega.json").unwrap();
    let accepted = "{\"type\":\"row\",\"children\":[]}\n";
    fs::write(&local, accepted).unwrap();
    let plan = herdr_app::plan(&ctx, Unit::HerdrAppLayouts).unwrap();
    assert!(!plan.herdr_app_pending);
    assert!(plan.files[0].changed());
    fs::write(home.join("layout-calls"), "").unwrap();
    assert_eq!(
        apply(&ctx, Unit::HerdrAppLayouts, &managed).kind(),
        "written"
    );
    assert_eq!(
        fs::read_to_string(home.join("layouts/vega.json")).unwrap(),
        accepted
    );
    assert_eq!(
        fs::read_to_string(home.join("layout-calls")).unwrap(),
        "status\ndo layout.switch --name vega\nstatus\n"
    );

    // The fake acknowledges the switch but reports an activation error. Its
    // one-shot rejection allows the subsequent rollback switch to succeed.
    fs::write(&local, "{\"type\":\"column\",\"children\":[]}\n").unwrap();
    fs::write(home.join("reject"), "").unwrap();
    fs::write(home.join("layout-calls"), "").unwrap();
    let result = apply(&ctx, Unit::HerdrAppLayouts, &managed);
    assert!(matches!(result, Outcome::Rejected(ref errors)
        if errors.iter().any(|e| e.contains("rejected update"))));
    assert_eq!(
        fs::read_to_string(home.join("layouts/vega.json")).unwrap(),
        accepted
    );
    assert!(home.join("selected").exists());
    assert!(!home.join("layout-error").exists());
    assert_eq!(
        fs::read_to_string(home.join("layout-calls")).unwrap(),
        "status\ndo layout.switch --name vega\nstatus\ndo layout.switch --name vega\n"
    );
}

#[test]
fn app_layout_drift_and_unit_dependency_order_are_visible() {
    let (d, mut ctx) = setup();
    ctx.herdr_app = fake_app(d.path());
    fs::create_dir_all(herdr_app::home(&ctx.env)).unwrap();
    fs::write(
        herdr_app::home(&ctx.env).join("installed.json"),
        r#"{"layout:main":{"json":"d25b961ea961fba1"}}"#,
    )
    .unwrap();
    let p = plan(&ctx, Unit::HerdrAppLayouts).unwrap();
    assert!(p
        .notes
        .iter()
        .any(|n| n.contains("main clone: app 0.1.14, installed 0.1.14")));
    fs::write(
        herdr_app::home(&ctx.env).join("installed.json"),
        r#"{"layout:main":{"json":"changed"}}"#,
    )
    .unwrap();
    let p = plan(&ctx, Unit::HerdrAppLayouts).unwrap();
    assert!(p
        .notes
        .iter()
        .any(|n| n.contains("layout drift") && n.contains("changed")));
    ctx.herdr_app = script(d.path(), "new-app", "echo 'herdr-app 0.2.0'").into_os_string();
    let p = plan(&ctx, Unit::HerdrAppLayouts).unwrap();
    assert!(p
        .notes
        .iter()
        .any(|n| n.contains("layout drift") && n.contains("0.2.0")));
    let selected = select(&["herdr-app".into()]).unwrap();
    let index = |u| selected.iter().position(|v| *v == u).unwrap();
    assert!(index(Unit::HerdrAppMods) < index(Unit::HerdrAppLayouts));
    assert!(index(Unit::HerdrAppLayouts) < index(Unit::HerdrAppSettings));
}

#[test]
fn app_first_layout_apply_retries_fresh_mod_discovery() {
    let (d, mut ctx) = setup();
    ctx.herdr_app = fake_app(d.path());
    let home = herdr_app::home(&ctx.env);
    fs::create_dir_all(&home).unwrap();
    let managed = BTreeSet::new();
    assert!(!home.join("mods/vega").exists());
    for unit in [
        Unit::HerdrAppMods,
        Unit::HerdrAppSidebar,
        Unit::HerdrAppInbox,
    ] {
        assert_eq!(apply(&ctx, unit, &managed).kind(), "written");
    }
    fs::write(home.join("mod-pending"), "").unwrap();
    assert_eq!(
        apply(&ctx, Unit::HerdrAppLayouts, &managed).kind(),
        "written"
    );
    let calls = fs::read_to_string(home.join("layout-calls")).unwrap();
    assert_eq!(calls.matches("do layout.switch --name vega").count(), 2);
    assert!(home.join("selected").exists());
}

#[test]
fn app_layout_mod_discovery_retry_is_bounded_and_rolls_back() {
    let (d, mut ctx) = setup();
    ctx.herdr_app = fake_app(d.path());
    let home = herdr_app::home(&ctx.env);
    fs::create_dir_all(&home).unwrap();
    let managed = BTreeSet::new();
    for unit in [
        Unit::HerdrAppMods,
        Unit::HerdrAppSidebar,
        Unit::HerdrAppInbox,
    ] {
        assert_eq!(apply(&ctx, unit, &managed).kind(), "written");
    }
    fs::write(home.join("mod-missing"), "").unwrap();
    let started = std::time::Instant::now();
    let result = apply(&ctx, Unit::HerdrAppLayouts, &managed);
    assert!(
        matches!(result, Outcome::Rejected(ref errors) if errors.iter().any(|e| e.contains("no mod folder")))
    );
    assert!(started.elapsed() < std::time::Duration::from_secs(15));
    assert!(!home.join("layouts/vega.json").exists());
    assert!(!home.join("selected").exists());
}

#[test]
fn dcg_hooks_preserve_mixed_entries_and_pi_source_is_managed() {
    let (_d, ctx) = setup();
    let original = serde_json::json!({"hooks": {"PreToolUse": [{"matcher":"Bash", "hooks":[
        {"type":"command","command":"/old/dcg"},
        {"type":"command","command":"other-guard"}
    ]}, {"matcher":"Read", "hooks":[]}]}, "other":true});
    for (unit, rel) in [
        (Unit::ClaudeSettings, ".claude/settings.json"),
        (Unit::CodexHooks, ".codex/hooks.json"),
    ] {
        fs::write(ctx.env.home.join(rel), original.to_string()).unwrap();
        assert_eq!(apply(&ctx, unit, &BTreeSet::new()).kind(), "written");
        let value: Value = serde_json::from_str(&read(&ctx, rel)).unwrap();
        assert_eq!(value["other"], true);
        let entries = value["hooks"]["PreToolUse"].as_array().unwrap();
        assert!(entries
            .iter()
            .any(|e| e["matcher"] == "Read" && e["hooks"].as_array().unwrap().is_empty()));
        assert!(entries
            .iter()
            .any(|e| e["hooks"][0]["command"] == "other-guard"));
        assert_eq!(
            entries
                .iter()
                .flat_map(|e| e["hooks"].as_array().unwrap())
                .filter(|h| merge::is_dcg_command(h))
                .count(),
            1
        );
        assert_eq!(apply(&ctx, unit, &BTreeSet::new()), Outcome::Unchanged);
        // A canonical hook alongside a stale duplicate must still be repaired.
        let mut duplicate = value.clone();
        duplicate["hooks"]["PreToolUse"].as_array_mut().unwrap().push(
            serde_json::json!({"matcher":"Bash", "hooks":[{"type":"command","command":"/stale/dcg"}]}));
        fs::write(ctx.env.home.join(rel), duplicate.to_string()).unwrap();
        assert_eq!(apply(&ctx, unit, &BTreeSet::new()).kind(), "written");
        let repaired: Value = serde_json::from_str(&read(&ctx, rel)).unwrap();
        assert_eq!(repaired, value);
    }
    assert_eq!(apply(&ctx, Unit::PiDcg, &BTreeSet::new()).kind(), "written");
    assert_eq!(
        read(&ctx, ".pi/agent/extensions/dcg-guard.ts"),
        sources::default("pi/extensions/dcg-guard.ts").unwrap()
    );
    let local = sources::init(&ctx.env, "pi/extensions/dcg-guard.ts").unwrap();
    fs::write(local, "// local guard\n").unwrap();
    assert_eq!(apply(&ctx, Unit::PiDcg, &BTreeSet::new()).kind(), "written");
    assert_eq!(
        read(&ctx, ".pi/agent/extensions/dcg-guard.ts"),
        "// local guard\n"
    );
}
