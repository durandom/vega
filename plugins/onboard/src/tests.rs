//! The changelog's rules, the since-version selection and the apply rules
//! against a temp HOME.

use super::*;
use std::path::Path;
use vega_config::infra::{Level, COMPONENTS};

fn blocks() -> Vec<Block> {
    parse(CHANGELOG).unwrap()
}

#[test]
fn changelog_parses_and_keeps_its_rules() {
    let b = blocks();
    validate(&b).unwrap();
    assert!(b
        .iter()
        .all(|b| b.entries.iter().all(|e| !e.text.is_empty())));
}

fn mentioned(b: &[Block], kinds: &[&str], name: &str) -> bool {
    let tick = format!("`{name}`");
    b.iter()
        .flat_map(|b| &b.entries)
        .any(|e| kinds.contains(&e.kind.as_str()) && e.text.contains(&tick))
}

#[test]
fn every_infra_component_and_unit_has_a_changelog_entry() {
    let b = blocks();
    for c in &COMPONENTS {
        let kind = match c.level {
            Level::Required => "required",
            Level::Optional => "optional",
        };
        assert!(
            mentioned(&b, &[kind], c.name),
            "CHANGELOG.md has no `{kind}:` entry naming `{}`",
            c.name
        );
    }
    for u in Unit::ALL {
        assert!(
            mentioned(&b, &["unit"], u.name()),
            "CHANGELOG.md has no `unit:` entry naming `{}`",
            u.name()
        );
    }
}

const SAMPLE: &str = "# Changelog\n\npreamble - not: an entry\n\n## 3 · 2026-10-15\n\n- feature: three\n  continued\n- migration: run it\n\n## 2 · 2026-10-15\n\n- unit: two\n\n## 1 · 2026-10-01\n\n- required: one\n";

#[test]
fn parse_joins_continuations_and_skips_the_preamble() {
    let b = parse(SAMPLE).unwrap();
    assert_eq!(b.len(), 3);
    assert_eq!(b[0].entries[0].text, "three continued");
    assert_eq!(b[0].entries[1].kind, "migration");
    validate(&b).unwrap();
}

#[test]
fn validate_rejects_gaps_order_and_bad_kinds() {
    let gap = SAMPLE.replace("## 2 ·", "## 5 ·");
    assert!(validate(&parse(&gap).unwrap()).is_err());
    let order = SAMPLE.replace("2026-10-01", "2026-10-20");
    assert!(validate(&parse(&order).unwrap()).is_err());
    assert!(parse(&SAMPLE.replace("- unit: two", "- fix: two")).is_err());
}

#[test]
fn since_selects_the_blocks_after_the_seen_one() {
    let b = parse(SAMPLE).unwrap();
    let ns = |seen| since(&b, seen).iter().map(|b| b.n).collect::<Vec<_>>();
    assert_eq!(ns(0), [3, 2, 1]);
    assert_eq!(ns(1), [3, 2]);
    assert_eq!(ns(3), Vec::<u32>::new());
    assert_eq!(mode(None, 3), Mode::FirstRun);
    assert_eq!(mode(Some(1), 3), Mode::Upgrade { from: 1, to: 3 });
    assert_eq!(mode(Some(3), 3), Mode::Current(3));
}

#[test]
fn state_records_the_seen_block() {
    let d = tempfile::tempdir().unwrap();
    let env = Env::at(d.path());
    assert_eq!(seen(&env), None);
    record(&env, 4, "vega-onboard 0.1.0-dev (abc)").unwrap();
    assert_eq!(seen(&env), Some(4));
}

fn ctx(home: &Path) -> Ctx {
    Ctx {
        env: Env::at(home),
        chezmoi: home.join("no-chezmoi").into_os_string(),
        herdr: home.join("no-herdr").into_os_string(),
        mux: PathBuf::from("/opt/vega/bin/vega-mux"),
        skills: home.join("no-skills").into_os_string(),
    }
}

#[test]
fn fresh_home_gets_every_default() {
    let d = tempfile::tempdir().unwrap();
    for dir in [".claude", ".codex"] {
        fs::create_dir_all(d.path().join(dir)).unwrap();
    }
    let ctx = ctx(d.path());
    let done = apply(&ctx, &BTreeSet::new());
    assert!(done.iter().all(|(_, r)| !matches!(r, Done::Kept(_))));
    let claude = fs::read_to_string(d.path().join(".claude/CLAUDE.md")).unwrap();
    assert!(!claude.is_empty());
    let settings = fs::read_to_string(d.path().join(".claude/settings.json")).unwrap();
    assert!(settings.contains("outputStyle"), "{settings}");
    assert!(settings.contains("activate.sh"), "{settings}");
}

#[test]
fn own_files_are_kept_hooks_still_merge() {
    let d = tempfile::tempdir().unwrap();
    let claude = d.path().join(".claude");
    fs::create_dir_all(&claude).unwrap();
    fs::write(claude.join("CLAUDE.md"), "# mine\n").unwrap();
    fs::write(
        claude.join("settings.json"),
        "{\"outputStyle\": \"Explanatory\"}\n",
    )
    .unwrap();
    let ctx = ctx(d.path());
    assert_eq!(
        decide(&ctx, Unit::ClaudeMd).unwrap(),
        Step::Keep("your own file differs".into())
    );
    let done = apply(&ctx, &BTreeSet::new());
    let of = |u| &done.iter().find(|(x, _)| *x == u).unwrap().1;
    assert!(matches!(of(Unit::ClaudeMd), Done::Kept(_)));
    assert_eq!(
        of(Unit::ClaudeSettings),
        &Done::Kept("you set outputStyle".into())
    );
    assert!(matches!(
        of(Unit::ClaudeHooks),
        Done::Config(Outcome::Written(_))
    ));
    assert_eq!(
        fs::read_to_string(claude.join("CLAUDE.md")).unwrap(),
        "# mine\n"
    );
    let settings = fs::read_to_string(claude.join("settings.json")).unwrap();
    assert!(settings.contains("Explanatory") && settings.contains("activate.sh"));
}

#[test]
fn onboard_keeps_existing_dcg_config_and_packs() {
    let d = tempfile::tempdir().unwrap();
    let ctx = ctx(d.path());
    let dcg = ctx.env.config.join("dcg");
    fs::create_dir_all(dcg.join("packs")).unwrap();
    fs::write(dcg.join("config.toml"), "# my config\n").unwrap();
    fs::write(dcg.join("packs/oc.yaml"), "# my pack\n").unwrap();
    for unit in [Unit::DcgConfig, Unit::DcgPacks] {
        assert!(matches!(decide(&ctx, unit).unwrap(), Step::Keep(_)));
    }
    let done = apply(&ctx, &BTreeSet::new());
    assert!(done
        .iter()
        .filter(|(u, _)| matches!(u, Unit::DcgConfig | Unit::DcgPacks))
        .all(|(_, result)| matches!(result, Done::Kept(_))));
    assert_eq!(
        fs::read_to_string(dcg.join("config.toml")).unwrap(),
        "# my config\n"
    );
    assert_eq!(
        fs::read_to_string(dcg.join("packs/oc.yaml")).unwrap(),
        "# my pack\n"
    );
}

#[test]
fn vegad_is_recommended_also_without_a_broker() {
    let none = vegad_line(false, false);
    assert!(none.contains("run vega post relay install"), "{none}");
    assert!(none.contains("local mode"), "{none}");
    assert!(!vegad_line(false, true).contains("local mode"));
    assert_eq!(vegad_line(true, true), "vegad: installed");
    assert!(vegad_line(true, false).contains("local mode"));
    let d = tempfile::tempdir().unwrap();
    let env = Env {
        home: d.path().into(),
        config: d.path().join(".config"),
        state: d.path().join(".local/state"),
    };
    // XDG_DATA_HOME unset in tests that run vega_onboard; the default dir.
    if std::env::var_os("XDG_DATA_HOME").is_none() {
        assert!(!vegad_installed(&env));
        let p = d.path().join(".local/share/vega/herdr-plugin");
        fs::create_dir_all(&p).unwrap();
        fs::write(p.join("herdr-plugin.toml"), "").unwrap();
        assert!(vegad_installed(&env));
    }
}
