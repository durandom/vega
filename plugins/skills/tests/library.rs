//! The skills library against throwaway repos: a private HOME, XDG dirs and
//! PATH per test, so nothing touches the real ~/.claude, ~/.agents or
//! ~/.config/vega. Tests marked "real engine" need skills.sh (`skills` on
//! PATH) and skip without it; the others use a fake engine.

use std::env;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const GOOD: &str =
    "---\nname: vega-good\ndescription: Does good things. Use when good.\n---\n\n# vega good\n";

struct Env {
    root: tempfile::TempDir,
    path: String,
}

impl Env {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let bin = root.path().join("bin");
        fs::create_dir_all(&bin).unwrap();
        for d in ["home", "config", "data", "project"] {
            fs::create_dir_all(root.path().join(d)).unwrap();
        }
        script(
            &bin.join("vega-good"),
            &format!("[ \"$1\" = --skill ] && printf '%s' '{GOOD}'"),
        );
        // Only the tools the plugin needs; never the real vega-* plugins.
        let mut dirs = vec![bin];
        for tool in ["git", "skills", "node"] {
            if let Some(d) = tool_dir(tool) {
                if !dirs.contains(&d) {
                    dirs.push(d);
                }
            }
        }
        let path = env::join_paths(dirs).unwrap().into_string().unwrap();
        let e = Env { root, path };
        e.git(&e.dir("project"), &["init", "-q"]);
        e
    }

    fn dir(&self, name: &str) -> PathBuf {
        self.root.path().join(name)
    }

    fn home(&self) -> PathBuf {
        self.dir("home")
    }

    fn clone(&self) -> PathBuf {
        self.dir("data").join("vega/skills")
    }

    fn cmd(&self, program: &str, cwd: &Path) -> Command {
        let mut c = Command::new(program);
        c.current_dir(cwd)
            .env_clear()
            .env("PATH", &self.path)
            .env("HOME", self.home())
            .env("XDG_CONFIG_HOME", self.dir("config"))
            .env("XDG_DATA_HOME", self.dir("data"))
            .env("DO_NOT_TRACK", "1")
            // `lib add` commits in the clone; no global git config here.
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .env("VEGA_SKILL_TIMEOUT_MS", "60000");
        c
    }

    fn git(&self, cwd: &Path, args: &[&str]) {
        let out = self
            .cmd("git", cwd)
            .args(["-c", "user.name=t", "-c", "user.email=t@t"])
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}: {out:?}");
    }

    fn git_status(&self, cwd: &Path) -> String {
        let out = self
            .cmd("git", cwd)
            .args(["status", "--porcelain", "--untracked-files=all"])
            .output()
            .unwrap();
        String::from_utf8(out.stdout).unwrap()
    }

    fn run(&self, args: &[&str]) -> (Output, String) {
        self.run_in(&self.dir("project"), args)
    }

    fn run_in(&self, cwd: &Path, args: &[&str]) -> (Output, String) {
        let out = self
            .cmd(env!("CARGO_BIN_EXE_vega-skills"), cwd)
            .args(args)
            .output()
            .unwrap();
        let stdout = String::from_utf8(out.stdout.clone()).unwrap();
        (out, stdout)
    }

    fn with_engine(&self, args: &[&str]) -> (Output, String) {
        let out = self
            .cmd(env!("CARGO_BIN_EXE_vega-skills"), &self.dir("project"))
            .env("VEGA_SKILLS_ENGINE", self.dir("bin").join("fake-skills"))
            .args(args)
            .output()
            .unwrap();
        let stdout = String::from_utf8(out.stdout.clone()).unwrap();
        (out, stdout)
    }

    /// The `lib add` action `list` suggests.
    fn add_to_library(&self, owner: &str, name: &str) -> String {
        format!("vega skills lib add {owner} -s {name}")
    }

    /// A library repo with foo, bar (with a reference file), one skill named
    /// like the `good` plugin and one like a lib command; the remote setting
    /// points at it.
    fn library(&self) -> PathBuf {
        let lib = self.dir("lib");
        skill(&lib.join("skills/foo"), "foo", "Does foo. Use when foo.");
        skill(&lib.join("skills/bar"), "bar", "\"Does bar: well\"");
        fs::create_dir_all(lib.join("skills/bar/references")).unwrap();
        fs::write(lib.join("skills/bar/references/r.md"), "REF").unwrap();
        skill(&lib.join("skills/good"), "good", "Shadowed.");
        skill(&lib.join("skills/sync"), "sync", "Reserved.");
        self.git(&lib, &["init", "-q"]);
        self.git(&lib, &["add", "-A"]);
        self.git(&lib, &["commit", "-qm", "init"]);
        let cfg = self.dir("config").join("vega");
        fs::create_dir_all(&cfg).unwrap();
        fs::write(cfg.join("skills-remote"), format!("{}\n", lib.display())).unwrap();
        script(
            &self.dir("bin").join("fake-skills"),
            "echo \"ENGINE $*\"; echo \"CWD $(pwd -P)\"",
        );
        lib
    }
}

fn script(path: &Path, body: &str) {
    fs::write(path, format!("#!/bin/sh\n{body}\n")).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

fn skill(dir: &Path, name: &str, description: &str) {
    fs::create_dir_all(dir).unwrap();
    fs::write(
        dir.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: {description}\n---\n\n# {name}\n\n{name}-BODY\n"),
    )
    .unwrap();
}

fn tool_dir(tool: &str) -> Option<PathBuf> {
    env::split_paths(&env::var_os("PATH")?)
        .find(|d| d.join(tool).is_file())
        .map(|d| d.canonicalize().unwrap_or(d))
}

fn real_engine() -> bool {
    let ok = tool_dir("skills").is_some();
    if !ok {
        eprintln!("skipped: skills.sh (`skills`) not on PATH");
    }
    ok
}

#[test]
fn missing_setting_is_a_definitive_empty_state() {
    let e = Env::new();
    let (out, stdout) = e.run(&["lib"]);
    assert!(out.status.success());
    assert!(stdout.contains("library: not configured\n"), "{stdout}");
    assert!(
        stdout.contains("skills-remote && vega skills lib sync"),
        "{stdout}"
    );

    let (out, stdout) = e.run(&["lib", "sync"]);
    assert!(out.status.success(), "{stdout}");
    assert!(stdout.contains("library: not configured\n"));
    assert!(stdout.contains("vega does not sync them"));

    for cmd in [&["lib", "install", "foo"][..], &["lib", "show", "foo"][..]] {
        let (out, stdout) = e.run(cmd);
        assert_eq!(out.status.code(), Some(1));
        assert!(stdout.contains("  not_configured,"), "{stdout}");
    }
}

#[test]
fn guides_index_has_no_library_rows() {
    let e = Env::new();
    e.library();
    e.run(&["lib", "sync"]);
    let (out, stdout) = e.run(&[]);
    assert!(out.status.success(), "{stdout}");
    assert!(stdout.contains("plugins[1]{name,description}:"), "{stdout}");
    assert!(
        !stdout.contains("library:") && !stdout.contains("skills["),
        "{stdout}"
    );
    assert!(
        // Rows, not bare names: the `bin:` path may hold them (vega.topbar).
        !stdout.contains("  foo,") && !stdout.contains("  bar,"),
        "{stdout}"
    );
}

#[test]
fn sync_clones_and_lib_lists_the_library() {
    let e = Env::new();
    e.library();
    let (_, stdout) = e.run(&["lib"]);
    assert!(stdout.contains("(not cloned)"), "{stdout}");

    let (out, stdout) = e.run(&["lib", "sync"]);
    assert!(out.status.success(), "{stdout}");
    assert!(stdout.contains("(cloned)"), "{stdout}");
    assert!(stdout.contains("global: 0 skills"), "{stdout}");
    assert!(e.clone().join("skills/foo/SKILL.md").is_file());
    let (_, stdout) = e.run(&["lib", "sync"]);
    assert!(stdout.contains("(current)"), "{stdout}");

    let (out, stdout) = e.run(&["lib"]);
    assert!(out.status.success());
    assert!(!stdout.contains("plugins"), "{stdout}");
    assert!(
        stdout.contains("skills[4]{name,installed,description}:"),
        "{stdout}"
    );
    assert!(stdout.contains("  bar,no,Does bar: well\n"), "{stdout}");
    assert!(stdout.contains("  foo,no,Does foo. Use when foo.\n"));
    // Plugin and command names no longer clash with library skills.
    assert!(stdout.contains("  good,no,Shadowed.\n"), "{stdout}");
    assert!(stdout.contains("  sync,no,Reserved.\n"), "{stdout}");
}

#[test]
fn guide_serves_plugins_only_and_points_to_lib_show() {
    let e = Env::new();
    e.library();
    e.run(&["lib", "sync"]);
    let (out, stdout) = e.with_engine(&["good"]);
    assert!(out.status.success());
    assert_eq!(stdout, GOOD);

    let (out, stdout) = e.with_engine(&["foo"]);
    assert_eq!(out.status.code(), Some(2), "{stdout}");
    assert!(stdout.contains("valid names: good,"), "{stdout}");
    assert!(
        stdout.contains("foo is a library skill: run `vega skills lib show foo`"),
        "{stdout}"
    );
    assert!(!stdout.contains("ENGINE"), "{stdout}");

    let (out, stdout) = e.with_engine(&["nope"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(
        stdout.contains("`vega skills lib` for library skills"),
        "{stdout}"
    );
}

#[test]
fn show_heads_skill_md_with_its_dir_in_the_clone() {
    let e = Env::new();
    let lib = e.library();
    vendor(&e, &lib);
    let tool = lib.join("skills/tool");
    skill(&tool, "tool", "Has scripts.");
    fs::create_dir_all(tool.join("scripts")).unwrap();
    script(&tool.join("scripts/x.sh"), "echo X");
    e.git(&lib, &["add", "-A"]);
    e.git(&lib, &["commit", "-qm", "tool"]);
    let commit = head(&e, &lib)[..7].to_string();
    e.run(&["lib", "sync"]);
    // The temp root sits behind a symlink on macOS (/var -> /private/var):
    // the header names the resolved path.
    let clone = e.clone().canonicalize().unwrap();
    for (name, dir) in [
        ("foo", "skills/foo"),
        ("ven", ".agents/skills/ven"),
        ("tool", "skills/tool"),
    ] {
        // Never runs skills.sh: the fake engine would print ENGINE.
        let (out, stdout) = e.with_engine(&["lib", "show", name]);
        assert!(out.status.success(), "{stdout}");
        let base = clone.join(dir);
        let expected = format!(
            "Base directory for this skill: {}\n\
             Relative paths in this skill (scripts/, references/, ...) resolve against it. Read-only:\n\
             keep state in the project, not in this dir.\n\
             library: {} @ {commit}\n\n\
             ---\nname: {name}\n",
            base.display(),
            lib.display(),
        );
        assert!(stdout.starts_with(&expected), "{stdout}");
        assert!(stdout.ends_with(&format!("{name}-BODY\n")), "{stdout}");
        assert!(!stdout.contains("ENGINE"), "{stdout}");
    }
    let (_, stdout) = e.with_engine(&["lib", "show", "tool"]);
    let base = stdout
        .lines()
        .next()
        .and_then(|l| l.strip_prefix("Base directory for this skill: "))
        .expect(&stdout);
    let x = Path::new(base).join("scripts/x.sh");
    assert!(x.is_absolute() && x.is_file(), "{x:?}");
    assert!(fs::metadata(&x).unwrap().permissions().mode() & 0o111 != 0);

    let (out, stdout) = e.with_engine(&["lib", "show", "nope"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(stdout.contains("  unknown_skill,"), "{stdout}");
    assert!(stdout.contains("run `vega skills lib`"), "{stdout}");
    let (out, stdout) = e.with_engine(&["lib", "show"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(stdout.contains("Usage: vega skills lib show"), "{stdout}");
}

#[test]
fn install_passes_the_clone_and_validates_names() {
    let e = Env::new();
    e.library();
    let (out, stdout) = e.with_engine(&["lib", "install", "nope"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(stdout.contains("  unknown_skill,"), "{stdout}");
    assert!(
        e.clone().join(".git").exists(),
        "install clones on first use"
    );

    let (out, stdout) = e.with_engine(&["lib", "install"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(
        stdout.contains("Usage: vega skills lib install"),
        "{stdout}"
    );

    let outside = e.dir("home");
    let (out, stdout) = e.run_in(&outside, &["lib", "install", "foo"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(stdout.contains("  not_in_project,"), "{stdout}");
}

/// Adds a vendored skill under `.agents/skills` to the library and commits it.
fn vendor(e: &Env, lib: &Path) {
    skill(&lib.join(".agents/skills/ven"), "ven", "Vendored.");
    e.git(lib, &["add", "-A"]);
    e.git(lib, &["commit", "-qm", "vendor"]);
}

#[test]
fn install_passes_each_skill_dir_including_vendored() {
    let e = Env::new();
    let lib = e.library();
    vendor(&e, &lib);
    let log = e.dir("engine.log");
    script(
        &e.dir("bin").join("fake-skills"),
        &format!("echo \"$*\" >> {}", log.display()),
    );
    e.with_engine(&["lib", "install", "foo", "ven"]);
    let calls = fs::read_to_string(&log).unwrap();
    let clone = e.clone().display().to_string();
    assert!(
        calls.contains(&format!("add {clone}/skills/foo --skill foo --agent")),
        "{calls}"
    );
    assert!(
        calls.contains(&format!(
            "add {clone}/.agents/skills/ven --skill ven --agent"
        )),
        "{calls}"
    );
}

#[test]
fn install_reports_drift_when_the_old_copy_stays() {
    let e = Env::new();
    e.library();
    // An older copy, linked; the fake engine leaves it untouched.
    let project = e.dir("project");
    skill(&project.join(".agents/skills/foo"), "foo", "Old foo.");
    fs::create_dir_all(project.join(".claude/skills")).unwrap();
    std::os::unix::fs::symlink(
        "../../.agents/skills/foo",
        project.join(".claude/skills/foo"),
    )
    .unwrap();
    let (out, stdout) = e.with_engine(&["lib", "install", "foo"]);
    assert_eq!(out.status.code(), Some(1), "{stdout}");
    assert!(stdout.contains(",drift\n"), "{stdout}");
    assert!(!stdout.contains(",ok\n"), "{stdout}");
}

#[test]
fn subcommands_have_help_and_reject_extra_args() {
    let e = Env::new();
    for (cmd, head) in [
        ("install", "Usage: vega skills lib install"),
        ("remove", "Usage: vega skills lib remove"),
        ("sync", "Usage: vega skills lib sync"),
        ("doctor", "Usage: vega skills lib doctor"),
    ] {
        let (out, stdout) = e.run(&["lib", cmd, "--help"]);
        assert!(out.status.success());
        assert!(stdout.starts_with(head), "{stdout}");
    }
    let (out, stdout) = e.run(&["lib", "sync", "x"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(stdout.contains("sync takes no arguments"));
}

#[test]
fn doctor_reports_library_problems() {
    let e = Env::new();
    let (out, stdout) = e.run(&["lib", "doctor"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(stdout.contains("library,-,not_configured,"), "{stdout}");

    e.library();
    e.run(&["lib", "sync"]);
    // A project skill without a Claude link (like beads today).
    skill(
        &e.dir("project").join(".agents/skills/beads"),
        "beads",
        "B.",
    );
    let (out, stdout) = e.run(&["lib", "doctor"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(!stdout.contains("library,"), "{stdout}");
    assert!(stdout.contains("project,beads,missing_claude_link,"));
}

#[test]
fn real_engine_install_serve_doctor_remove() {
    if !real_engine() {
        return;
    }
    let e = Env::new();
    e.library();
    let project = e.dir("project");

    let (out, stdout) = e.run(&["lib", "install", "foo"]);
    assert!(out.status.success(), "{stdout}");
    assert!(stdout.contains("  foo,project,"), "{stdout}");
    assert!(stdout.contains(",ok\n"));
    assert!(project.join(".agents/skills/foo/SKILL.md").is_file());
    let link = fs::read_link(project.join(".claude/skills/foo")).unwrap();
    assert!(link.is_relative(), "{link:?}");
    // The lock names the library remote, not a path relative to the project.
    let lock: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(project.join("skills-lock.json")).unwrap())
            .unwrap();
    let entry = &lock["skills"]["foo"];
    assert_eq!(
        entry["source"],
        e.dir("lib").display().to_string(),
        "{lock}"
    );
    assert_eq!(entry["sourceType"], "git", "{lock}");
    assert_eq!(entry["skillPath"], "skills/foo/SKILL.md", "{lock}");
    let (_, stdout) = e.run(&["lib", "list"]);
    assert!(
        row(&stdout, "foo").starts_with("  foo,library,-,")
            && row(&stdout, "foo").ends_with(",ok,keep"),
        "{stdout}"
    );

    let (_, stdout) = e.run(&["lib"]);
    assert!(stdout.contains("  foo,project,"), "{stdout}");

    // Not installed, still served with its supporting files.
    let (out, stdout) = e.run(&["lib", "show", "bar"]);
    assert!(out.status.success(), "{stdout}");
    assert!(stdout.contains("bar-BODY"), "{stdout}");
    let dir = stdout
        .lines()
        .next()
        .and_then(|l| l.strip_prefix("Base directory for this skill: "))
        .expect(&stdout);
    assert!(Path::new(dir).join("references/r.md").is_file());

    let (out, stdout) = e.run(&["lib", "doctor"]);
    assert!(
        !stdout.contains("project,foo,"),
        "{stdout} {:?}",
        out.status
    );
    fs::write(project.join(".agents/skills/foo/SKILL.md"), "edited").unwrap();
    let (out, stdout) = e.run(&["lib", "doctor"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(stdout.contains("project,foo,drift,"), "{stdout}");

    let (out, stdout) = e.run(&["lib", "remove", "foo"]);
    assert!(out.status.success(), "{stdout}");
    assert!(stdout.contains("  foo,project,removed\n"), "{stdout}");
    assert!(!project.join(".agents/skills/foo").exists());
    let (out, stdout) = e.run(&["lib", "remove", "foo"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(stdout.contains("  not_installed,"), "{stdout}");
}

#[test]
fn real_engine_installs_vendored_and_replaces_stale_copy() {
    if !real_engine() {
        return;
    }
    let e = Env::new();
    let lib = e.library();
    vendor(&e, &lib);
    let project = e.dir("project");
    skill(&project.join(".agents/skills/foo"), "foo", "Old foo.");
    let (out, stdout) = e.run(&["lib", "install", "ven", "foo"]);
    assert!(out.status.success(), "{stdout}");
    assert!(stdout.contains("  ven,project,"), "{stdout}");
    assert!(
        !stdout.contains("drift") && !stdout.contains("missing"),
        "{stdout}"
    );
    assert!(project.join(".agents/skills/ven/SKILL.md").is_file());
    // A vendored skill (hidden .agents/skills in the clone) is served too.
    let (out, stdout) = e.run(&["lib", "show", "ven"]);
    assert!(out.status.success(), "{stdout}");
    assert!(stdout.contains("ven-BODY"), "{stdout}");
    assert!(project.join(".claude/skills/ven").exists());
    let foo = fs::read_to_string(project.join(".agents/skills/foo/SKILL.md")).unwrap();
    assert!(foo.contains("Does foo."), "{foo}");
}

#[test]
fn real_engine_global_set_and_sync_restore() {
    if !real_engine() {
        return;
    }
    let e = Env::new();
    let lib = e.library();
    let home = e.home();

    let (out, stdout) = e.run(&["lib", "install", "bar", "-g"]);
    assert!(out.status.success(), "{stdout}");
    assert!(home.join(".agents/skills/bar/references/r.md").is_file());
    assert!(home.join(".claude/skills/bar").exists());
    let set = fs::read_to_string(e.dir("config").join("vega/skills-global")).unwrap();
    assert_eq!(set, "bar\n");

    // A new library commit: sync pulls it and updates the global copy.
    fs::write(lib.join("skills/bar/references/r.md"), "REF v2").unwrap();
    e.git(&lib, &["commit", "-qam", "v2"]);
    let (out, stdout) = e.run(&["lib", "sync"]);
    assert!(out.status.success(), "{stdout}");
    assert!(stdout.contains("(pulled)"), "{stdout}");
    assert!(stdout.contains("  bar,updated\n"), "{stdout}");
    assert_eq!(
        fs::read_to_string(home.join(".agents/skills/bar/references/r.md")).unwrap(),
        "REF v2"
    );
    let (_, stdout) = e.run(&["lib", "sync"]);
    assert!(stdout.contains("  bar,current\n"), "{stdout}");

    let (out, stdout) = e.run(&["lib", "remove", "bar", "-g"]);
    assert!(out.status.success(), "{stdout}");
    assert!(!home.join(".agents/skills/bar").exists());
    let set = fs::read_to_string(e.dir("config").join("vega/skills-global")).unwrap();
    assert_eq!(set, "");
}

#[test]
fn doctor_finds_and_fixes_shadowing_copies() {
    let e = Env::new();
    let home = e.home();
    let project = e.dir("project");
    for base in [&home, &project] {
        for n in ["linked", "same", "differs"] {
            skill(&base.join(".agents/skills").join(n), n, "X.");
        }
    }
    // Global pi: a link (as skills.sh leaves them), an identical copy, a changed copy.
    let pi = home.join(".pi/agent/skills");
    fs::create_dir_all(&pi).unwrap();
    std::os::unix::fs::symlink(home.join(".agents/skills/linked"), pi.join("linked")).unwrap();
    skill(&pi.join("same"), "same", "X.");
    skill(&pi.join("differs"), "differs", "Changed.");
    // codex's own `.system` dir and skills not in .agents/skills are not shadows.
    skill(&home.join(".codex/skills/.system/same"), "same", "X.");
    skill(&home.join(".codex/skills/other"), "other", "O.");
    // Project codex: an identical copy.
    skill(&project.join(".codex/skills/same"), "same", "X.");

    let (out, stdout) = e.run(&["lib", "doctor"]);
    assert_eq!(out.status.code(), Some(1), "{stdout}");
    assert!(stdout.contains("global,linked,shadow,"), "{stdout}");
    assert!(stdout.contains("global,same,shadow,"), "{stdout}");
    assert!(
        stdout.contains("global,differs,shadow_differs,"),
        "{stdout}"
    );
    assert!(stdout.contains("project,same,shadow,"), "{stdout}");
    assert!(!stdout.contains(",other,"), "{stdout}");
    assert!(!stdout.contains("removed["), "{stdout}");
    // Without --fix nothing changes.
    assert!(fs::symlink_metadata(pi.join("linked")).is_ok());
    assert!(pi.join("same").is_dir());
    assert!(project.join(".codex/skills/same").is_dir());

    let (out, stdout) = e.run(&["lib", "doctor", "--fix"]);
    assert_eq!(out.status.code(), Some(1), "{stdout}");
    assert!(stdout.contains("removed[3]{scope,name,path}:"), "{stdout}");
    assert!(
        stdout.contains("global,linked,~/.pi/agent/skills/linked"),
        "{stdout}"
    );
    assert!(
        stdout.contains("global,differs,shadow_differs,"),
        "{stdout}"
    );
    assert!(!stdout.contains(",shadow,"), "{stdout}");
    assert!(fs::symlink_metadata(pi.join("linked")).is_err());
    assert!(!pi.join("same").exists());
    assert!(!project.join(".codex/skills/same").exists());
    assert!(pi.join("differs/SKILL.md").is_file());
    assert!(home.join(".codex/skills/.system/same/SKILL.md").is_file());
    // The library copies stay.
    for base in [&home, &project] {
        for n in ["linked", "same", "differs"] {
            assert!(base
                .join(".agents/skills")
                .join(n)
                .join("SKILL.md")
                .is_file());
        }
    }
}

#[test]
fn doctor_ignores_an_agent_dir_linked_to_agents_skills() {
    let e = Env::new();
    let home = e.home();
    skill(&home.join(".agents/skills/foo"), "foo", "F.");
    fs::create_dir_all(home.join(".codex")).unwrap();
    std::os::unix::fs::symlink(home.join(".agents/skills"), home.join(".codex/skills")).unwrap();
    let (_, stdout) = e.run(&["lib", "doctor", "--fix"]);
    assert!(!stdout.contains("shadow"), "{stdout}");
    assert!(home.join(".agents/skills/foo/SKILL.md").is_file());
}

fn link(target: impl AsRef<Path>, at: &Path) {
    fs::create_dir_all(at.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(target, at).unwrap();
}

/// The data row of `name` in `vega skills lib list` output.
fn row<'a>(stdout: &'a str, name: &str) -> &'a str {
    stdout
        .lines()
        .find(|l| l.starts_with(&format!("  {name},")))
        .unwrap_or_else(|| panic!("no row {name}:\n{stdout}"))
}

#[test]
fn list_shows_every_origin_in_the_project() {
    let e = Env::new();
    let lib = e.library();
    e.run(&["lib", "sync"]);
    let p = e.dir("project");
    // library: an installed copy with its Claude link, one of them drifted.
    for n in ["foo", "bar"] {
        let src = lib.join("skills").join(n);
        let dst = p.join(".agents/skills").join(n);
        fs::create_dir_all(dst.join("references")).unwrap();
        for f in ["SKILL.md", "references/r.md"] {
            if src.join(f).is_file() {
                fs::copy(src.join(f), dst.join(f)).unwrap();
            }
        }
        link(
            format!("../../.agents/skills/{n}"),
            &p.join(".claude/skills").join(n),
        );
    }
    fs::write(p.join(".agents/skills/bar/SKILL.md"), "edited").unwrap();
    // A shadowing identical copy of foo in .codex/skills.
    fs::create_dir_all(p.join(".codex/skills/foo/references")).unwrap();
    fs::copy(
        p.join(".agents/skills/foo/SKILL.md"),
        p.join(".codex/skills/foo/SKILL.md"),
    )
    .unwrap();
    // upstream: recorded in skills-lock.json.
    skill(&p.join(".agents/skills/axi"), "axi", "A.");
    link("../../.agents/skills/axi", &p.join(".claude/skills/axi"));
    fs::write(
        p.join("skills-lock.json"),
        r#"{"version":1,"skills":{"axi":{"source":"kunchenguid/axi","sourceType":"github"}}}"#,
    )
    .unwrap();
    // local: a real dir; link: a symlink out of the project; broken: dangling.
    skill(&p.join(".claude/skills/mine"), "mine", "M.");
    skill(&e.home().join("src/ext"), "ext", "E.");
    link(e.home().join("src/ext"), &p.join(".claude/skills/ext"));
    link(e.home().join("gone"), &p.join(".pi/skills/gone"));
    fs::create_dir_all(p.join(".pi/skills/.hidden")).unwrap();

    let before = e.git_status(&p);
    let (out, stdout) = e.run(&["lib", "list"]);
    assert!(out.status.success(), "{stdout}");
    assert!(
        stdout.contains("skills[6]{name,origin,source,dirs,status,action}:"),
        "{stdout}"
    );
    assert_eq!(
        row(&stdout, "foo"),
        "  foo,library,-,.agents/skills .claude/skills .codex/skills,shadow,vega skills lib doctor --fix"
    );
    assert!(
        row(&stdout, "bar").starts_with(
            "  bar,library,-,.agents/skills .claude/skills,drift,vega skills lib install bar"
        ),
        "{stdout}"
    );
    let add = |owner: &str, n: &str| e.add_to_library(owner, n);
    assert_eq!(
        row(&stdout, "axi"),
        format!(
            "  axi,upstream,kunchenguid/axi,.agents/skills .claude/skills,ok,{}",
            add("kunchenguid/axi", "axi")
        )
    );
    assert_eq!(
        row(&stdout, "mine"),
        format!(
            "  mine,local,-,.claude/skills,ok,{}",
            add("<owner/repo>", "mine")
        )
    );
    assert_eq!(
        row(&stdout, "ext"),
        format!(
            "  ext,link,~/src/ext,.claude/skills,ok,{}",
            add("<owner/repo>", "ext")
        )
    );
    assert!(
        row(&stdout, "gone").starts_with("  gone,broken,~/gone,.pi/skills,broken_link,rm "),
        "{stdout}"
    );
    assert!(!stdout.contains("hidden"), "{stdout}");
    // Pure read.
    assert_eq!(e.git_status(&p), before);
    assert!(p.join(".codex/skills/foo/SKILL.md").is_file());
}

#[test]
fn list_global_scope_and_claude_ai_synced() {
    let e = Env::new();
    let home = e.home();
    skill(&home.join(".agents/skills/wt"), "wt", "W.");
    link("../../.agents/skills/wt", &home.join(".claude/skills/wt"));
    fs::write(
        home.join(".agents/.skill-lock.json"),
        r#"{"version":3,"skills":{"wt":{"source":"max-sixty/worktrunk"}}}"#,
    )
    .unwrap();
    skill(&home.join(".claude/skills/synced/a"), "a", "A.");
    skill(&home.join(".claude/skills/synced/b"), "b", "B.");
    skill(&home.join(".codex/skills/.system/x"), "x", "X.");
    skill(&home.join(".pi/agent/skills/own"), "own", "O.");
    // A project skill is not global.
    skill(&e.dir("project").join(".agents/skills/proj"), "proj", "P.");

    let (out, stdout) = e.run(&["lib", "list", "-g"]);
    assert!(out.status.success(), "{stdout}");
    assert!(stdout.starts_with("scope: global ~\n"), "{stdout}");
    assert!(stdout.contains("library: not configured\n"), "{stdout}");
    assert_eq!(
        row(&stdout, "wt"),
        format!(
            "  wt,upstream,max-sixty/worktrunk,.agents/skills .claude/skills,ok,{}",
            e.add_to_library("max-sixty/worktrunk", "wt")
        )
    );
    assert_eq!(
        row(&stdout, "own"),
        format!(
            "  own,local,-,.pi/agent/skills,ok,{}",
            e.add_to_library("<owner/repo>", "own")
        )
    );
    assert_eq!(
        row(&stdout, "synced"),
        "  synced,claude.ai,2 synced,.claude/skills,ok,keep"
    );
    assert!(stdout.contains("skills[3]"), "{stdout}");
    assert!(!stdout.contains("proj") && !stdout.contains("  x,"));
}

#[test]
fn list_empty_and_outside_a_project() {
    let e = Env::new();
    let (out, stdout) = e.run(&["lib", "list"]);
    assert!(out.status.success(), "{stdout}");
    assert!(stdout.contains("skills: 0 skills in"), "{stdout}");
    let (out, stdout) = e.run_in(&e.home(), &["lib", "list"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(stdout.contains("  not_in_project,"), "{stdout}");
    let (out, stdout) = e.run(&["lib", "list", "x"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(stdout.contains("Usage: vega skills lib list"), "{stdout}");
}

/// A project that owns foo (also in the library, identical) and mine (not in
/// the library): sources in skills/, `.agents/skills` and `.claude/skills` links.
fn owner_project(e: &Env, lib: &Path) -> PathBuf {
    let p = e.dir("project");
    e.git(
        &p,
        &["remote", "add", "origin", "git@github.com:me/proj.git"],
    );
    let src = lib.join("skills/foo/SKILL.md");
    fs::create_dir_all(p.join("skills/foo")).unwrap();
    fs::copy(src, p.join("skills/foo/SKILL.md")).unwrap();
    skill(&p.join("skills/mine"), "mine", "Mine.");
    for n in ["foo", "mine"] {
        link(
            format!("../../skills/{n}"),
            &p.join(".agents/skills").join(n),
        );
        link(
            format!("../../.agents/skills/{n}"),
            &p.join(".claude/skills").join(n),
        );
    }
    p
}

#[test]
fn list_shows_own_skills_and_library_actions() {
    let e = Env::new();
    let lib = e.library();
    e.run(&["lib", "sync"]);
    let p = owner_project(&e, &lib);
    // A copy of a library skill from elsewhere: install it from the library.
    skill(&e.home().join("src/bar"), "bar", "Other bar.");
    link(e.home().join("src/bar"), &p.join(".claude/skills/bar"));

    let (out, stdout) = e.run(&["lib", "list"]);
    assert!(out.status.success(), "{stdout}");
    assert_eq!(
        row(&stdout, "foo"),
        "  foo,own,skills/foo,.agents/skills .claude/skills,ok,keep"
    );
    assert_eq!(
        row(&stdout, "mine"),
        format!(
            "  mine,own,skills/mine,.agents/skills .claude/skills,ok,{}",
            e.add_to_library("me/proj", "mine")
        )
    );
    assert_eq!(
        row(&stdout, "bar"),
        "  bar,link,~/src/bar,.claude/skills,ok,vega skills lib install bar"
    );
}

#[test]
fn install_refuses_in_the_owner_repo() {
    let e = Env::new();
    let lib = e.library();
    let p = owner_project(&e, &lib);
    let log = e.dir("engine.log");
    script(
        &e.dir("bin").join("fake-skills"),
        &format!("echo \"$*\" >> {}", log.display()),
    );
    let (out, stdout) = e.with_engine(&["lib", "install", "bar", "foo"]);
    assert_eq!(out.status.code(), Some(1), "{stdout}");
    assert!(stdout.contains("  owned_here,"), "{stdout}");
    assert!(stdout.contains("keep it: edit skills/foo here"), "{stdout}");
    assert!(!log.exists(), "the engine ran");
    assert!(fs::symlink_metadata(p.join(".agents/skills/foo"))
        .unwrap()
        .is_symlink());
    // Globally the project's ownership does not matter.
    let (out, stdout) = e.with_engine(&["lib", "install", "foo", "-g"]);
    assert!(!stdout.contains("owned_here"), "{stdout} {:?}", out.status);
}

#[test]
fn doctor_accepts_own_links_and_reports_a_stale_library_copy() {
    let e = Env::new();
    let lib = e.library();
    e.run(&["lib", "sync"]);
    let p = owner_project(&e, &lib);
    let (_, stdout) = e.run(&["lib", "doctor"]);
    assert!(!stdout.contains("project,foo,"), "{stdout}");
    assert!(!stdout.contains("project,mine,"), "{stdout}");

    fs::write(p.join("skills/foo/SKILL.md"), "---\nname: foo\n---\nnew\n").unwrap();
    let (out, stdout) = e.run(&["lib", "doctor"]);
    assert_eq!(out.status.code(), Some(1), "{stdout}");
    let fix = "vega skills lib add me/proj -s foo";
    assert!(
        stdout.contains(&format!("  project,foo,library_stale,{fix}\n")),
        "{stdout}"
    );
    assert!(!stdout.contains("drift"), "{stdout}");
    let (_, stdout) = e.run(&["lib", "list"]);
    assert_eq!(
        row(&stdout, "foo"),
        format!("  foo,own,skills/foo,.agents/skills .claude/skills,library_stale,{fix}")
    );
}

#[test]
fn list_treats_lock_entries_pointing_at_the_library_as_library() {
    let e = Env::new();
    let lib = e.library();
    e.run(&["lib", "sync"]);
    let p = e.dir("project");
    for n in ["foo", "bar"] {
        fs::create_dir_all(p.join(".agents/skills")).unwrap();
        let out = Command::new("cp")
            .arg("-R")
            .arg(lib.join("skills").join(n))
            .arg(p.join(".agents/skills"))
            .output()
            .unwrap();
        assert!(out.status.success());
        link(
            format!("../../.agents/skills/{n}"),
            &p.join(".claude/skills").join(n),
        );
    }
    // foo as skills.sh writes it: a path relative to the project; bar with
    // the remote's GitHub name (`pin_lock` with a GitHub remote).
    let rel = "../data/vega/skills/skills/foo";
    let cfg = e.dir("config").join("vega/skills-remote");
    fs::write(&cfg, "git@github.com:me/lib.git\n").unwrap();
    e.git(
        &e.clone(),
        &["remote", "set-url", "origin", "git@github.com:me/lib.git"],
    );
    fs::write(
        p.join("skills-lock.json"),
        format!(
            r#"{{"version":1,"skills":{{"foo":{{"source":"{rel}","sourceType":"local"}},"bar":{{"source":"me/lib","sourceType":"github"}}}}}}"#
        ),
    )
    .unwrap();
    let (out, stdout) = e.run(&["lib", "list"]);
    assert!(out.status.success(), "{stdout}");
    for n in ["foo", "bar"] {
        assert_eq!(
            row(&stdout, n),
            format!("  {n},library,-,.agents/skills .claude/skills,ok,keep"),
            "{stdout}"
        );
    }
}

/// An owner repo with zed under `skills/` and yy nested deeper (found only
/// with --full-depth), committed.
fn owner_repo(e: &Env) -> PathBuf {
    let o = e.dir("owner");
    skill(&o.join("skills/zed"), "zed", "Zed.");
    fs::create_dir_all(o.join("skills/zed/references")).unwrap();
    fs::write(o.join("skills/zed/references/r.md"), "R1").unwrap();
    skill(&o.join("deep/x/skills/yy"), "yy", "Yy.");
    e.git(&o, &["init", "-q"]);
    e.git(&o, &["add", "-A"]);
    e.git(&o, &["commit", "-qm", "init"]);
    o
}

fn head(e: &Env, repo: &Path) -> String {
    let out = e
        .cmd("git", repo)
        .args(["rev-parse", "HEAD"])
        .output()
        .unwrap();
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

fn subject(e: &Env, repo: &Path) -> String {
    let out = e
        .cmd("git", repo)
        .args(["log", "-1", "--format=%s"])
        .output()
        .unwrap();
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

#[test]
fn add_vendors_commits_and_never_pushes() {
    let e = Env::new();
    let lib = e.library();
    let owner = owner_repo(&e);
    e.run(&["lib", "sync"]);
    let log = e.dir("engine.log");
    // Copies OWNER/skills/NAME (or OWNER/deep/x/skills/NAME with
    // --full-depth) into .agents/skills and records it in skills-lock.json.
    script(
        &e.dir("bin").join("fake-skills"),
        &format!(
            r#"echo "$*" >> {log}
o=$2; n=$5; src=$o/skills/$n
case "$*" in *--full-depth*) [ -d "$src" ] || src=$o/deep/x/skills/$n ;; esac
[ -d "$src" ] || {{ echo "No matching skills found for: $n"; exit 1; }}
/bin/mkdir -p .agents/skills && /bin/cp -R "$src" .agents/skills/ || exit 1
printf '{{"version":1,"skills":{{"%s":{{"source":"%s"}}}}}}\n' "$n" "$o" > skills-lock.json"#,
            log = log.display()
        ),
    );
    let lib_head = head(&e, &lib);
    let o = owner.canonicalize().unwrap().display().to_string();

    let (out, stdout) = e.with_engine(&["lib", "add", &o, "-s", "zed"]);
    assert!(out.status.success(), "{stdout}");
    assert!(stdout.contains(&format!("  zed,{o},added\n")), "{stdout}");
    assert!(stdout.contains(&format!("Add zed from {o}")), "{stdout}");
    assert!(
        stdout.contains(&format!(
            "help: not pushed; publish it: git -C {} push",
            e.clone().display()
        )),
        "{stdout}"
    );
    let calls = fs::read_to_string(&log).unwrap();
    assert!(
        calls.contains(&format!("add {o} --copy -s zed --agent universal -y\n")),
        "{calls}"
    );
    assert!(e
        .clone()
        .join(".agents/skills/zed/references/r.md")
        .is_file());
    assert_eq!(e.git_status(&e.clone()), "", "committed");
    assert_eq!(subject(&e, &e.clone()), format!("Add zed from {o}"));
    assert_eq!(head(&e, &lib), lib_head, "pushed to the library remote");

    // Nested: retried with --full-depth.
    let (out, stdout) = e.with_engine(&["lib", "add", &o, "-s", "yy"]);
    assert!(out.status.success(), "{stdout}");
    let calls = fs::read_to_string(&log).unwrap();
    assert!(
        calls.contains("-s yy --agent universal -y --full-depth\n"),
        "{calls}"
    );

    // Unknown skill: error, clone reset.
    let (out, stdout) = e.with_engine(&["lib", "add", &o, "-s", "nope"]);
    assert_eq!(out.status.code(), Some(1), "{stdout}");
    assert!(stdout.contains("  engine_failed,nope from "), "{stdout}");
    assert_eq!(e.git_status(&e.clone()), "");

    // Update re-vendors from the recorded owner.
    fs::write(
        owner.join("deep/x/skills/yy/SKILL.md"),
        "---\nname: yy\ndescription: Yy2.\n---\n",
    )
    .unwrap();
    let (out, stdout) = e.with_engine(&["lib", "update", "yy"]);
    assert!(out.status.success(), "{stdout}");
    assert!(stdout.contains(&format!("  yy,{o},updated\n")), "{stdout}");
    assert_eq!(subject(&e, &e.clone()), "Update yy from their owners");
    let (out, stdout) = e.with_engine(&["lib", "update", "yy"]);
    assert!(out.status.success(), "{stdout}");
    assert!(
        stdout.contains("  yy,") && stdout.contains(",unchanged\n"),
        "{stdout}"
    );
    assert!(stdout.contains("commit: none (no changes)"), "{stdout}");

    // A library-owned skill has no owner to update from.
    let (out, stdout) = e.with_engine(&["lib", "update", "foo"]);
    assert_eq!(out.status.code(), Some(1), "{stdout}");
    assert!(stdout.contains("  not_vendored,"), "{stdout}");

    // A dirty clone is refused before the engine runs.
    fs::write(e.clone().join("stray"), "x").unwrap();
    let before = fs::read_to_string(&log).unwrap();
    for args in [&["lib", "add", &o, "-s", "zed"][..], &["lib", "update"][..]] {
        let (out, stdout) = e.with_engine(args);
        assert_eq!(out.status.code(), Some(1), "{stdout}");
        assert!(stdout.contains("  dirty_clone,"), "{stdout}");
    }
    assert_eq!(fs::read_to_string(&log).unwrap(), before);
}

#[test]
fn add_and_update_usage() {
    let e = Env::new();
    for (args, msg) in [
        (&["lib", "add"][..], "no OWNER given"),
        (&["lib", "add", "o/r"][..], "no skill NAME given after -s"),
        (&["lib", "add", "o/r", "x"][..], "unexpected argument x"),
        (&["lib", "update", "--all"][..], "unknown flag --all"),
    ] {
        let (out, stdout) = e.run(args);
        assert_eq!(out.status.code(), Some(2), "{stdout}");
        assert!(stdout.contains(msg), "{stdout}");
    }
}

#[test]
fn real_engine_add_and_update_from_a_local_owner() {
    if !real_engine() {
        return;
    }
    let e = Env::new();
    let lib = e.library();
    let owner = owner_repo(&e);
    e.run(&["lib", "sync"]);
    let lib_head = head(&e, &lib);
    let o = owner.display().to_string();

    let (out, stdout) = e.run(&["lib", "add", &o, "-s", "zed"]);
    assert!(out.status.success(), "{stdout}");
    assert!(stdout.contains(",added\n"), "{stdout}");
    let clone = e.clone();
    assert!(clone.join(".agents/skills/zed/references/r.md").is_file());
    assert!(!clone.join(".claude").exists(), "only .agents/skills");
    assert_eq!(e.git_status(&clone), "");
    let lock = fs::read_to_string(clone.join("skills-lock.json")).unwrap();
    assert!(lock.contains("\"zed\""), "{lock}");

    let (out, stdout) = e.run(&["lib", "show", "zed"]);
    assert!(out.status.success(), "{stdout}");
    assert!(stdout.contains("zed-BODY"), "{stdout}");

    fs::write(owner.join("skills/zed/references/r.md"), "R2").unwrap();
    e.git(&owner, &["commit", "-qam", "v2"]);
    let (out, stdout) = e.run(&["lib", "update", "zed"]);
    assert!(out.status.success(), "{stdout}");
    assert!(stdout.contains(",updated\n"), "{stdout}");
    assert_eq!(
        fs::read_to_string(clone.join(".agents/skills/zed/references/r.md")).unwrap(),
        "R2"
    );
    assert_eq!(e.git_status(&clone), "");
    assert_eq!(head(&e, &lib), lib_head, "never pushed");
}
