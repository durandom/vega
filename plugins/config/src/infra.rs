//! The infra model (docs/design/onboarding.md): every component the plugins
//! need, required or optional, with a local check, what it enables and how
//! to get it. Checks never touch the network; a [`Probe`] holds the PATH, env
//! vars and directories they read, so tests run on a temp HOME.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::{Ctx, Env, Unit};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Required,
    Optional,
}

impl Level {
    pub fn as_str(self) -> &'static str {
        match self {
            Level::Required => "required",
            Level::Optional => "optional",
        }
    }
}

#[derive(Debug)]
pub struct Component {
    pub name: &'static str,
    pub level: Level,
    pub enables: &'static str,
    pub get: &'static str,
}

const fn c(
    name: &'static str,
    level: Level,
    enables: &'static str,
    get: &'static str,
) -> Component {
    Component {
        name,
        level,
        enables,
        get,
    }
}

use Level::{Optional, Required};

/// Required first; the order of the views.
pub const COMPONENTS: [Component; 21] = [
    c("mise", Required, "mise run install-cli, mise run check", "install mise: https://mise.jdx.dev/installing-mise.html"),
    c("cargo", Required, "building vega (toolchain pinned in rust-toolchain.toml)", "install rustup: https://rustup.rs"),
    c("git", Required, "clone, upgrade, worktrees (>= 2.54: the wt-guard hook)", "install git with the OS package manager"),
    c("vega", Required, "every vega <plugin> call", "mise run install-cli in the vega checkout; put ~/.local/bin on PATH"),
    c("herdr", Required, "vega mux, uac, board, open: agents in panes", "install herdr: https://github.com/herdrdev/herdr"),
    c("agent", Required, "the coding agents vega runs and configures", "install Claude Code, Codex or pi and start it once"),
    c("chezmoi", Optional, "vega config apply writes every unit", "drop vega's targets from the dotfiles; vega config lists them"),
    c("wt", Optional, "vega mux spawn --worktree with worktrunk (else herdr worktrees)", "install worktrunk (wt)"),
    c("dcg", Optional, "recommended: destructive command protection before shell execution", "install dcg: https://github.com/Dicklesworthstone/destructive_command_guard#installation; register hooks with its installer, then dcg doctor (Claude: dcg install; Codex: ~/.codex/hooks.json)"),
    c("bd", Optional, "beads: vega board, UAC decisions, the bd shim", "install beads: https://github.com/gastownhall/beads"),
    c("gh", Optional, "vega ci for GitHub, the gh shim", "install gh: https://cli.github.com, then gh auth login"),
    c("tea", Optional, "vega ci for Gitea and Forgejo", "install tea: https://gitea.com/gitea/tea"),
    c("nats", Optional, "vega post and vegad: cross-host mail, ci watch verdicts, beads sync, review notes", "own broker: mise run nats, its URL into ~/.config/vega/nats.url; estate: set VEGA_NATS_URL or ~/.config/vega/nats.url, a key from the Slayer, vega post keygen"),
    c("tailnet", Optional, "private services and vega deploy to other hosts", "an invite to the estate's tailnet"),
    c("dolt", Optional, "vega bd: a project's beads synced to a Dolt remote", "install dolt; set VEGA_BD_REMOTE or ~/.config/vega/bd.remote; creation also needs VEGA_BD_SQL or ~/.config/vega/bd.sql (otherwise beads stay local)"),
    c("uv", Optional, "vega lanista, the model catalog", "install uv: https://docs.astral.sh/uv/"),
    c("agent-browser", Optional, "vega review in a headed browser", "install agent-browser"),
    c("skills.sh", Optional, "vega skills lib, the global skills library", "install Node.js (npx skills)"),
    c("op", Optional, "vega secrets adopt and rekey from 1Password", "install the 1Password CLI: https://developer.1password.com/docs/cli/"),
    c("token-burn", Optional, "vega capacity: subscription windows per provider; the capacity line of vega uac status and the vega mux spawn warning", "install token-burn (github.com/durandom/token-burn), then token-burn install for its daemon; vega capacity shows its health"),
    c("moshi-hook", Optional, "phone notifications and approvals through Moshi; vega upgrade keeps it current", "brew install rjyo/moshi/moshi-hook, then brew services start moshi-hook"),
];

/// What the checks read.
#[derive(Debug, Clone)]
pub struct Probe {
    pub env: Env,
    /// PATH without vega's shim dir: a shim is no proof of the real tool.
    pub path: Vec<PathBuf>,
    pub vars: BTreeMap<String, String>,
    /// The macOS app's CLI, which is not on PATH.
    pub tailscale_app: PathBuf,
}

const VARS: [&str; 5] = [
    "VEGA_NATS_URL",
    "VEGA_TOKEN_BURN",
    "VEGA_BD_REMOTE",
    "HERDR_BIN_PATH",
    "VEGA_SKILLS_ENGINE",
];

impl Probe {
    pub fn from_env(env: Env) -> Probe {
        let shims = env.home.join(".local/share/vega/shims/bin");
        let path = std::env::var_os("PATH")
            .map(|p| std::env::split_paths(&p).collect::<Vec<_>>())
            .unwrap_or_default()
            .into_iter()
            .filter(|d| *d != shims)
            .collect();
        let vars = VARS
            .iter()
            .filter_map(|k| {
                let v = std::env::var(k).ok().filter(|v| !v.is_empty())?;
                Some((k.to_string(), v))
            })
            .collect();
        Probe {
            env,
            path,
            vars,
            tailscale_app: PathBuf::from("/Applications/Tailscale.app/Contents/MacOS/Tailscale"),
        }
    }

    pub fn find(&self, bin: &str) -> Option<PathBuf> {
        use std::os::unix::fs::PermissionsExt;
        self.path.iter().map(|d| d.join(bin)).find(|p| {
            p.metadata()
                .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        })
    }

    fn var(&self, k: &str) -> Option<&str> {
        self.vars.get(k).map(String::as_str)
    }

    fn tailnet(&self) -> bool {
        self.find("tailscale").is_some() || self.tailscale_app.is_file()
    }

    /// The configured broker: env, else the first line of nats.url.
    pub fn broker(&self) -> Option<String> {
        vega_herdr::service_config::resolve(
            self.var("VEGA_NATS_URL"),
            Some(&self.env.config),
            "nats.url",
        )
    }
}

/// One component's state on this host.
#[derive(Debug, Clone)]
pub struct Row {
    pub component: &'static Component,
    pub ok: bool,
    pub state: String,
}

impl Row {
    pub fn name(&self) -> &'static str {
        self.component.name
    }

    /// The next step: empty when ok.
    pub fn next(&self) -> &'static str {
        if self.ok {
            ""
        } else {
            self.component.get
        }
    }
}

fn bin(p: &Probe, name: &str) -> (bool, String) {
    match p.find(name) {
        Some(_) => (true, "ok".into()),
        None => (false, "missing".into()),
    }
}

fn dcg(p: &Probe) -> (bool, String) {
    if p.find("dcg").is_none() {
        return (false, "missing (recommended)".into());
    }
    let mut gaps = Vec::new();
    for (agent, dir, file) in [
        ("claude", ".claude", "settings.json"),
        ("codex", ".codex", "hooks.json"),
    ] {
        if !p.env.home.join(dir).is_dir() {
            continue;
        }
        let registered = std::fs::read_to_string(p.env.home.join(dir).join(file))
            .ok()
            .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
            .and_then(|v| v.get("hooks")?.get("PreToolUse")?.as_array().cloned())
            .is_some_and(|entries| {
                entries.iter().any(|entry| {
                    let matcher = entry.get("matcher").and_then(|m| m.as_str()).unwrap_or("");
                    if !["", "Bash", "Bash|PowerShell", ".*"].contains(&matcher) {
                        return false;
                    }
                    entry
                        .get("hooks")
                        .and_then(|h| h.as_array())
                        .is_some_and(|hooks| {
                            hooks.iter().any(|h| {
                                if h.get("type").and_then(|t| t.as_str()) != Some("command") {
                                    return false;
                                }
                                let cmd = h
                                    .get("command")
                                    .and_then(|c| c.as_str())
                                    .unwrap_or("")
                                    .trim();
                                let executable = cmd.trim_matches(['\'', '"']);
                                Path::new(executable)
                                    .file_name()
                                    .is_some_and(|n| n == "dcg")
                                    && (executable == "dcg"
                                        || Path::new(executable).is_absolute()
                                            && std::fs::metadata(executable).is_ok_and(|m| {
                                                use std::os::unix::fs::PermissionsExt;
                                                m.is_file() && m.permissions().mode() & 0o111 != 0
                                            }))
                            })
                        })
                })
            });
        if !registered {
            gaps.push(agent);
        }
    }
    if gaps.is_empty() {
        (
            true,
            "ok: installed; hooks registered for present Claude/Codex agents".into(),
        )
    } else {
        (
            false,
            format!(
                "installed; missing or invalid PreToolUse hook: {}",
                gaps.join(" ")
            ),
        )
    }
}

/// `git version 2.51.0` -> (2, 51).
fn git_version(text: &str) -> Option<(u32, u32)> {
    let v = text.split_whitespace().nth(2)?;
    let mut it = v.split('.');
    Some((it.next()?.parse().ok()?, it.next()?.parse().ok()?))
}

fn git(p: &Probe) -> (bool, String) {
    let Some(g) = p.find("git") else {
        return (false, "missing".into());
    };
    let out = Command::new(g).arg("--version").output();
    let text = out
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default();
    match git_version(&text) {
        Some(v) if v < (2, 54) => (
            true,
            format!("ok, {}.{}: wt-guard hook off (needs 2.54)", v.0, v.1),
        ),
        _ => (true, "ok".into()),
    }
}

fn agent(p: &Probe) -> (bool, String) {
    let found: Vec<&str> = [
        ("claude", ".claude"),
        ("codex", ".codex"),
        ("pi", ".pi/agent"),
    ]
    .iter()
    .filter(|(_, d)| p.env.home.join(d).is_dir())
    .map(|(n, _)| *n)
    .collect();
    if found.is_empty() {
        (false, "none of ~/.claude ~/.codex ~/.pi/agent".into())
    } else {
        (true, format!("ok: {}", found.join(" ")))
    }
}

/// How many of vega's config targets chezmoi manages.
fn chezmoi(p: &Probe, ctx: &Ctx) -> (bool, String) {
    if p.find("chezmoi").is_none() {
        return (true, "ok: not installed".into());
    }
    let managed = match crate::chezmoi_managed(&ctx.chezmoi) {
        Ok(m) => m,
        Err(e) => return (false, format!("chezmoi failed: {e}")),
    };
    let n: usize = Unit::ALL
        .iter()
        .filter_map(|u| crate::plan(ctx, *u).ok())
        .map(|plan| crate::managed_targets(&plan, &managed).len())
        .sum();
    if n == 0 {
        (true, "ok: manages no vega target".into())
    } else {
        (false, format!("manages {n} vega target(s)"))
    }
}

fn nats(p: &Probe) -> (bool, String) {
    match p.broker() {
        Some(u) => (true, format!("ok: {u}")),
        None => (
            false,
            "no broker set: configure VEGA_NATS_URL or ~/.config/vega/nats.url".into(),
        ),
    }
}

fn dolt(p: &Probe) -> (bool, String) {
    if p.find("dolt").is_none() {
        return (false, "missing".into());
    }
    match vega_herdr::service_config::resolve(
        p.var("VEGA_BD_REMOTE"),
        Some(&p.env.config),
        "bd.remote",
    ) {
        Some(r) => (true, format!("ok: {r}")),
        None => (false, "no remote: beads stay local".into()),
    }
}

fn skills_engine(p: &Probe) -> (bool, String) {
    if let Some(e) = p.var("VEGA_SKILLS_ENGINE") {
        return (true, format!("ok: {e}"));
    }
    match (p.find("skills"), p.find("npx")) {
        (Some(_), _) => (true, "ok".into()),
        (None, Some(_)) => (true, "ok: npx skills".into()),
        _ => (false, "missing (no skills, no npx)".into()),
    }
}

/// moshi-hook as installed on this host (docs/design/upgrade.md).
#[derive(Debug, Clone, PartialEq)]
pub struct Moshi {
    pub bin: PathBuf,
    /// Installed by Homebrew: the binary resolves into `Cellar/moshi-hook/`.
    pub brew: bool,
    /// Its registered service file (launchd plist or systemd user unit).
    pub service: Option<PathBuf>,
}

/// moshi-hook on PATH, with how it was installed; `None` when missing.
pub fn moshi(p: &Probe) -> Option<Moshi> {
    let bin = p.find("moshi-hook")?;
    let brew = std::fs::canonicalize(&bin)
        .is_ok_and(|r| r.to_string_lossy().contains("/Cellar/moshi-hook/"));
    let dirs = [
        (p.env.home.join("Library/LaunchAgents"), "moshi-hook.plist"),
        (p.env.config.join("systemd/user"), "moshi-hook.service"),
    ];
    let service = dirs.iter().find_map(|(d, suffix)| {
        std::fs::read_dir(d)
            .ok()?
            .flatten()
            .map(|e| e.path())
            .find(|f| {
                f.file_name()
                    .is_some_and(|n| n.to_string_lossy().ends_with(suffix))
            })
    });
    Some(Moshi { bin, brew, service })
}

fn moshi_row(p: &Probe) -> (bool, String) {
    match moshi(p) {
        None => (false, "missing".into()),
        Some(m) => (
            true,
            format!(
                "ok: {}, {}",
                if m.brew {
                    "Homebrew"
                } else {
                    "not from Homebrew"
                },
                if m.service.is_some() {
                    "service registered"
                } else {
                    "no service"
                }
            ),
        ),
    }
}

/// Every component's state, in [`COMPONENTS`] order.
pub fn check(p: &Probe, ctx: &Ctx) -> Vec<Row> {
    COMPONENTS
        .iter()
        .map(|comp| {
            let (ok, state) = match comp.name {
                "git" => git(p),
                "dcg" => dcg(p),
                "herdr" => match p.var("HERDR_BIN_PATH") {
                    Some(b) if Path::new(b).is_file() => (true, "ok".into()),
                    _ => bin(p, "herdr"),
                },
                "agent" => agent(p),
                "chezmoi" => chezmoi(p, ctx),
                "nats" => nats(p),
                "tailnet" => {
                    if p.tailnet() {
                        (true, "ok".into())
                    } else {
                        (false, "no tailscale".into())
                    }
                }
                "dolt" => dolt(p),
                "skills.sh" => skills_engine(p),
                "moshi-hook" => moshi_row(p),
                "token-burn" => match p.var("VEGA_TOKEN_BURN") {
                    Some(b) if Path::new(b).is_file() => (true, format!("ok: {b}")),
                    _ => bin(p, "token-burn"),
                },
                name => bin(p, name),
            };
            Row {
                component: comp,
                ok,
                state,
            }
        })
        .collect()
}

/// `required 6/6, optional 5/12`.
pub fn summary(rows: &[Row]) -> String {
    let count = |l: Level| {
        let of: Vec<&Row> = rows.iter().filter(|r| r.component.level == l).collect();
        (of.iter().filter(|r| r.ok).count(), of.len())
    };
    let (r, rn) = count(Level::Required);
    let (o, on) = count(Level::Optional);
    format!("required {r}/{rn}, optional {o}/{on}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    fn fake(dir: &Path, name: &str, body: &str) {
        let p = dir.join(name);
        fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
        fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
    }

    fn setup(bins: &[&str]) -> (tempfile::TempDir, Probe, Ctx) {
        let d = tempfile::tempdir().unwrap();
        let home = d.path().join("home");
        let bin = d.path().join("bin");
        fs::create_dir_all(&home).unwrap();
        fs::create_dir_all(&bin).unwrap();
        for b in bins {
            fake(&bin, b, "exit 0");
        }
        let env = Env::at(&home);
        let probe = Probe {
            env: env.clone(),
            path: vec![bin],
            vars: BTreeMap::new(),
            tailscale_app: d.path().join("no-tailscale-app"),
        };
        let ctx = Ctx {
            env,
            chezmoi: d.path().join("bin/chezmoi").into_os_string(),
            herdr: d.path().join("no-herdr").into_os_string(),
            mux: PathBuf::from("/opt/vega/bin/vega-mux"),
            skills: d.path().join("no-skills").into_os_string(),
        };
        (d, probe, ctx)
    }

    fn row<'a>(rows: &'a [Row], name: &str) -> &'a Row {
        rows.iter().find(|r| r.name() == name).unwrap()
    }

    #[test]
    fn dcg_requires_a_binary_and_real_pretool_hooks_for_present_agents() {
        let (d, p, ctx) = setup(&[]);
        assert!(!row(&check(&p, &ctx), "dcg").ok);
        fake(&d.path().join("bin"), "dcg", "exit 0");
        for agent in [".claude", ".codex"] {
            fs::create_dir_all(p.env.home.join(agent)).unwrap();
        }
        let claude = p.env.home.join(".claude/settings.json");
        let codex = p.env.home.join(".codex/hooks.json");
        fs::write(&claude, r#"{"comment":"dcg"}"#).unwrap();
        assert!(!row(&check(&p, &ctx), "dcg").ok);
        let hook = r#"{"hooks":{"PreToolUse":[{"matcher":"Bash","hooks":[{"type":"command","command":"dcg"}]}]}}"#;
        fs::write(&claude, hook).unwrap();
        assert!(row(&check(&p, &ctx), "dcg").state.contains("codex"));
        fs::write(&codex, hook).unwrap();
        assert!(row(&check(&p, &ctx), "dcg").ok);
        let absolute_dir = d.path().join("absolute path");
        fs::create_dir_all(&absolute_dir).unwrap();
        fake(&absolute_dir, "dcg", "exit 0");
        let absolute = absolute_dir.join("dcg").display().to_string();
        // The installer uses an absolute path; quoting must survive spaces too.
        for command in [
            d.path().join("bin/dcg").display().to_string(),
            format!("'{absolute}'"),
            format!("\"{absolute}\""),
        ] {
            let mut installed: serde_json::Value = serde_json::from_str(hook).unwrap();
            installed["hooks"]["PreToolUse"][0]["matcher"] = "Bash|PowerShell".into();
            installed["hooks"]["PreToolUse"][0]["hooks"][0]["command"] = command.into();
            let text = serde_json::to_string(&installed).unwrap();
            fs::write(&claude, &text).unwrap();
            fs::write(&codex, &text).unwrap();
            assert!(row(&check(&p, &ctx), "dcg").ok, "{text}");
        }
        for bad in [
            hook.replace("Bash", "Read"),
            hook.replace("dcg", "/missing/dcg"),
            "invalid".into(),
        ] {
            fs::write(&codex, bad).unwrap();
            assert!(!row(&check(&p, &ctx), "dcg").ok);
        }
    }

    #[test]
    fn required_come_first_and_names_are_unique() {
        let first_opt = COMPONENTS.iter().position(|c| c.level == Optional).unwrap();
        assert!(COMPONENTS[first_opt..].iter().all(|c| c.level == Optional));
        let mut names: Vec<&str> = COMPONENTS.iter().map(|c| c.name).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), COMPONENTS.len());
    }

    #[test]
    fn bare_home_misses_everything_with_a_next_step() {
        let (_d, p, ctx) = setup(&[]);
        let rows = check(&p, &ctx);
        assert_eq!(summary(&rows), "required 0/6, optional 1/15");
        // chezmoi absent is fine: nothing manages vega's targets.
        assert!(row(&rows, "chezmoi").ok);
        for r in rows.iter().filter(|r| !r.ok) {
            assert!(!r.next().is_empty(), "{} has no next step", r.name());
        }
        assert_eq!(
            row(&rows, "nats").state,
            "no broker set: configure VEGA_NATS_URL or ~/.config/vega/nats.url"
        );
    }

    #[test]
    fn outside_user_core_is_complete_without_estate_services() {
        let (_d, p, ctx) = setup(&["mise", "cargo", "git", "vega", "herdr", "gh"]);
        fs::create_dir_all(p.env.home.join(".claude")).unwrap();
        let rows = check(&p, &ctx);
        assert!(rows
            .iter()
            .filter(|r| r.component.level == Required)
            .all(|r| r.ok));
        assert_eq!(row(&rows, "agent").state, "ok: claude");
        assert!(row(&rows, "gh").ok);
        assert!(!row(&rows, "tailnet").ok);
        assert!(!row(&rows, "nats").ok);
    }

    #[test]
    fn broker_requires_config_even_on_tailnet() {
        let (d, mut p, ctx) = setup(&["dolt"]);
        let url = p.env.config.join("vega/nats.url");
        fs::create_dir_all(url.parent().unwrap()).unwrap();
        fs::write(&url, "nats://10.0.0.5:4222\n# comment\n").unwrap();
        assert_eq!(
            row(&check(&p, &ctx), "nats").state,
            "ok: nats://10.0.0.5:4222"
        );
        p.vars
            .insert("VEGA_NATS_URL".into(), "nats://127.0.0.1:4222".into());
        assert_eq!(
            row(&check(&p, &ctx), "nats").state,
            "ok: nats://127.0.0.1:4222"
        );
        p.vars.clear();
        fs::remove_file(&url).unwrap();
        assert_eq!(
            row(&check(&p, &ctx), "dolt").state,
            "no remote: beads stay local"
        );
        fake(&d.path().join("bin"), "tailscale", "exit 0");
        let rows = check(&p, &ctx);
        assert!(!row(&rows, "nats").ok);
        assert!(!row(&rows, "dolt").ok);
        fs::write(
            p.env.config.join("vega/bd.remote"),
            "http://dolt.example:8080\n",
        )
        .unwrap();
        assert_eq!(
            row(&check(&p, &ctx), "dolt").state,
            "ok: http://dolt.example:8080"
        );
    }

    #[test]
    fn old_git_is_ok_with_a_note() {
        let (d, p, ctx) = setup(&[]);
        fake(&d.path().join("bin"), "git", "echo 'git version 2.51.0'");
        let r = check(&p, &ctx);
        assert!(row(&r, "git").ok);
        assert_eq!(
            row(&r, "git").state,
            "ok, 2.51: wt-guard hook off (needs 2.54)"
        );
        assert_eq!(
            git_version("git version 2.54.1 (Apple Git-1)"),
            Some((2, 54))
        );
    }

    #[test]
    fn chezmoi_managing_a_vega_target_is_a_gap() {
        let (d, p, ctx) = setup(&[]);
        fs::create_dir_all(p.env.home.join(".claude")).unwrap();
        let target = p.env.home.join(".claude/CLAUDE.md");
        fake(
            &d.path().join("bin"),
            "chezmoi",
            &format!("echo '{}'", target.display()),
        );
        let r = check(&p, &ctx);
        assert!(!row(&r, "chezmoi").ok);
        assert_eq!(row(&r, "chezmoi").state, "manages 1 vega target(s)");
    }

    #[test]
    fn moshi_hook_from_homebrew_with_its_service() {
        let (d, p, ctx) = setup(&[]);
        assert_eq!(row(&check(&p, &ctx), "moshi-hook").state, "missing");
        let cellar = d.path().join("Cellar/moshi-hook/0.4.27/bin");
        fs::create_dir_all(&cellar).unwrap();
        fake(&cellar, "moshi-hook", "exit 0");
        std::os::unix::fs::symlink(cellar.join("moshi-hook"), d.path().join("bin/moshi-hook"))
            .unwrap();
        assert_eq!(
            row(&check(&p, &ctx), "moshi-hook").state,
            "ok: Homebrew, no service"
        );
        let agents = p.env.home.join("Library/LaunchAgents");
        fs::create_dir_all(&agents).unwrap();
        fs::write(agents.join("sh.brew.moshi-hook.plist"), "").unwrap();
        let m = moshi(&p).unwrap();
        assert!(m.brew);
        assert_eq!(m.service, Some(agents.join("sh.brew.moshi-hook.plist")));
        assert_eq!(
            row(&check(&p, &ctx), "moshi-hook").state,
            "ok: Homebrew, service registered"
        );
    }

    #[test]
    fn shims_and_npx_and_herdr_bin_path() {
        let (d, mut p, ctx) = setup(&["npx"]);
        let herdr = d.path().join("herdr-real");
        fake(d.path(), "herdr-real", "exit 0");
        p.vars
            .insert("HERDR_BIN_PATH".into(), herdr.display().to_string());
        let r = check(&p, &ctx);
        assert!(row(&r, "herdr").ok);
        assert_eq!(row(&r, "skills.sh").state, "ok: npx skills");
    }

    #[test]
    fn token_burn_on_path_or_by_env() {
        let (d, mut p, ctx) = setup(&[]);
        assert_eq!(row(&check(&p, &ctx), "token-burn").state, "missing");
        let tb = d.path().join("tb");
        fake(d.path(), "tb", "exit 0");
        p.vars
            .insert("VEGA_TOKEN_BURN".into(), tb.display().to_string());
        assert_eq!(
            row(&check(&p, &ctx), "token-burn").state,
            format!("ok: {}", tb.display())
        );
        let (_d, p, ctx) = setup(&["token-burn"]);
        assert!(row(&check(&p, &ctx), "token-burn").ok);
    }
}
