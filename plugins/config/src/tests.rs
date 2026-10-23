//! Units against a temp HOME with fake herdr, chezmoi and vega-skills.

use super::*;
use std::os::unix::fs::symlink;

fn script(d: &Path, name: &str, body: &str) -> PathBuf {
    let bin = d.join(name);
    fs::write(&bin, format!("#!/bin/sh\n{body}\n")).unwrap();
    fs::set_permissions(&bin, fs::Permissions::from_mode(0o755)).unwrap();
    bin
}

/// A HOME with claude, codex and pi set up, no chezmoi, a passing herdr.
fn setup() -> (tempfile::TempDir, Ctx) {
    let d = tempfile::tempdir().unwrap();
    let home = d.path().join("home");
    for dir in [".claude", ".codex", ".pi/agent", ".config"] {
        fs::create_dir_all(home.join(dir)).unwrap();
    }
    let herdr = herdr::tests::fake_herdr(d.path());
    let skills = script(
        d.path(),
        "vega-skills",
        "echo \"$*\" >> \"$(dirname \"$0\")/skills-calls\"; echo 'skills: 2 up to date'",
    );
    let ctx = Ctx {
        env: Env::at(&home),
        chezmoi: d.path().join("no-chezmoi").into_os_string(),
        herdr: herdr.into_os_string(),
        mux: PathBuf::from("/opt/vega/bin/vega-mux"),
        skills: skills.into_os_string(),
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
        s(&["hooks", "herdr", "claude.hooks"]).unwrap(),
        [
            Unit::Herdr,
            Unit::ClaudeHooks,
            Unit::CodexHooks,
            Unit::PiPrefix
        ]
    );
    assert!(s(&["nope"]).unwrap_err().contains("known: herdr"));
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
        Unit::Herdr,
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
    let s: Value = serde_json::from_str(&read(&ctx, ".claude/settings.json")).unwrap();
    assert_eq!(s["outputStyle"], "Caveman ELI5");
    assert_eq!(
        s["hooks"]["PreToolUse"][0]["hooks"][0]["command"],
        "'/opt/vega/bin/vega-mux' hook ask-parent"
    );
    assert_eq!(s["hooks"]["SessionStart"].as_array().unwrap().len(), 1);
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
    assert!(v["hooks"]["PreToolUse"].is_array());
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
fn herdr_rejected_keeps_the_old_file() {
    let (_d, ctx) = setup();
    let local = sources::local_dir(&ctx.env).join("herdr");
    fs::create_dir_all(&local).unwrap();
    fs::write(local.join("config.toml"), "BAD = 1\n").unwrap();
    let path = herdr::path(&ctx.env);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, "old = true\n").unwrap();
    assert_eq!(
        apply(&ctx, Unit::Herdr, &BTreeSet::new()),
        Outcome::Rejected(vec!["bad key BAD".into()])
    );
    assert_eq!(fs::read_to_string(&path).unwrap(), "old = true\n");
    assert!(!backup_path(&path).exists());
    // Without herdr: skipped, nothing written.
    let mut ctx = ctx;
    ctx.herdr = "/nonexistent/herdr".into();
    fs::remove_file(local.join("config.toml")).unwrap();
    assert_eq!(
        apply(&ctx, Unit::Herdr, &BTreeSet::new()),
        Outcome::Skipped("herdr not installed".into())
    );
    assert_eq!(fs::read_to_string(&path).unwrap(), "old = true\n");
}

#[test]
fn herdr_written_with_backup_and_reload() {
    let (d, ctx) = setup();
    let path = herdr::path(&ctx.env);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, "[ui]\nagent_panel_sort = \"recent\"\n").unwrap();
    let p = plan(&ctx, Unit::Herdr).unwrap();
    assert_eq!(
        p.notes,
        ["kept: ui.agent_panel_sort = recent (herdr owns it)"]
    );
    let out = apply(&ctx, Unit::Herdr, &BTreeSet::new());
    assert_eq!(
        out,
        Outcome::Written(vec![path.display().to_string(), "reload ok".into()])
    );
    assert!(fs::read_to_string(&path)
        .unwrap()
        .contains("agent_panel_sort = \"recent\""));
    assert_eq!(
        fs::read_to_string(backup_path(&path)).unwrap(),
        "[ui]\nagent_panel_sort = \"recent\"\n"
    );
    assert_eq!(
        fs::read_to_string(d.path().join("calls")).unwrap(),
        "config check\nserver reload-config\n"
    );
    assert_eq!(fs::read_dir(path.parent().unwrap()).unwrap().count(), 2);
}
