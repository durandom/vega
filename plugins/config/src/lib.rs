//! `vega config`: vega owns the coding agents', herdr's and dcg's config
//! (docs/design/config.md, vega-lhc.107).
//!
//! A [`Unit`] is one target file (or a few of one kind) with one ownership
//! rule: whole file, key-merge, seeded keys, or the skills library. [`plan`]
//! renders what vega would write from the sources ([`sources`]: local
//! override, else repo default) and the live files; [`apply`] writes it,
//! refusing a unit while chezmoi manages one of its targets.

pub mod herdr;
pub mod infra;
pub mod merge;
pub mod sources;

use std::collections::BTreeSet;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{Map, Value};
pub use vega_theme_sync::Env;

use sources::{Origin, Source};

/// Everything a run resolves against; tests point it into a temp dir.
#[derive(Debug, Clone)]
pub struct Ctx {
    pub env: Env,
    pub chezmoi: OsString,
    pub herdr: OsString,
    /// The `vega-mux` the hooks run.
    pub mux: PathBuf,
    /// The `vega-skills` that syncs the library.
    pub skills: OsString,
}

impl Ctx {
    /// This user's context: the installed sibling binaries, chezmoi and herdr from PATH.
    pub fn from_env() -> Option<Ctx> {
        let env = Env::from_env()?;
        Some(Ctx {
            mux: installed(&env.home, "vega-mux"),
            skills: installed(&env.home, "vega-skills").into_os_string(),
            chezmoi: OsString::from("chezmoi"),
            herdr: OsString::from("herdr"),
            env,
        })
    }
}

/// The installed sibling binary: `~/.local/bin/<name>` where install-cli.sh
/// puts it (a worktree's target dir dies with the worktree), else PATH.
fn installed(home: &Path, name: &str) -> PathBuf {
    let p = home.join(".local/bin").join(name);
    if p.exists() {
        return p;
    }
    vega_plugin::find_on_path(name).unwrap_or_else(|| PathBuf::from(name))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unit {
    Herdr,
    DcgConfig,
    DcgPacks,
    ClaudeMd,
    CodexMd,
    PiMd,
    ClaudeStyles,
    ClaudeSettings,
    ClaudeHooks,
    CodexHooks,
    PiPrefix,
    Skills,
    Agentsview,
}

impl Unit {
    pub const ALL: [Unit; 13] = [
        Unit::Herdr,
        Unit::DcgConfig,
        Unit::DcgPacks,
        Unit::ClaudeMd,
        Unit::CodexMd,
        Unit::PiMd,
        Unit::ClaudeStyles,
        Unit::ClaudeSettings,
        Unit::ClaudeHooks,
        Unit::CodexHooks,
        Unit::PiPrefix,
        Unit::Skills,
        Unit::Agentsview,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Unit::Herdr => "herdr",
            Unit::DcgConfig => "dcg.config",
            Unit::DcgPacks => "dcg.packs",
            Unit::ClaudeMd => "claude.md",
            Unit::CodexMd => "codex.md",
            Unit::PiMd => "pi.md",
            Unit::ClaudeStyles => "claude.styles",
            Unit::ClaudeSettings => "claude.settings",
            Unit::ClaudeHooks => "claude.hooks",
            Unit::CodexHooks => "codex.hooks",
            Unit::PiPrefix => "pi.prefix",
            Unit::Skills => "skills",
            Unit::Agentsview => "agentsview",
        }
    }

    /// The target, for display.
    pub fn target(self) -> &'static str {
        match self {
            Unit::Herdr => "~/.config/herdr/config.toml",
            Unit::DcgConfig => "~/.config/dcg/config.toml",
            Unit::DcgPacks => "~/.config/dcg/packs/",
            Unit::ClaudeMd => "~/.claude/CLAUDE.md",
            Unit::CodexMd => "~/.codex/AGENTS.md",
            Unit::PiMd => "~/.pi/agent/AGENTS.md",
            Unit::ClaudeStyles => "~/.claude/output-styles/",
            Unit::ClaudeSettings | Unit::ClaudeHooks => "~/.claude/settings.json",
            Unit::CodexHooks => "~/.codex/hooks.json",
            Unit::PiPrefix => "~/.pi/agent/settings.json",
            Unit::Skills => "~/.agents/skills/, ~/.claude/skills/",
            Unit::Agentsview => "*/skills/agentsview-finding-history/LOCAL-SETUP.md",
        }
    }

    pub fn ownership(self) -> &'static str {
        match self {
            Unit::Herdr => "whole file, herdr keys kept",
            Unit::ClaudeMd | Unit::CodexMd | Unit::PiMd | Unit::Agentsview => "whole file",
            Unit::DcgConfig => "whole file",
            Unit::DcgPacks | Unit::ClaudeStyles => "whole file each",
            Unit::ClaudeSettings => "seeded keys",
            Unit::ClaudeHooks | Unit::CodexHooks | Unit::PiPrefix => "key-merge",
            Unit::Skills => "library",
        }
    }

    /// The Slayer's own preferences: never applied by install-cli.sh.
    pub fn personal(self) -> bool {
        matches!(
            self,
            Unit::ClaudeMd
                | Unit::CodexMd
                | Unit::PiMd
                | Unit::ClaudeStyles
                | Unit::ClaudeSettings
                | Unit::Agentsview
                | Unit::DcgConfig
                | Unit::DcgPacks
        )
    }
}

/// Unit groups for the command line.
pub const GROUPS: [(&str, &[Unit]); 2] = [
    ("instructions", &[Unit::ClaudeMd, Unit::CodexMd, Unit::PiMd]),
    (
        "hooks",
        &[Unit::ClaudeHooks, Unit::CodexHooks, Unit::PiPrefix],
    ),
];

/// The units `names` select (unit names or groups), in [`Unit::ALL`] order;
/// all of them without names.
pub fn select(names: &[String]) -> Result<Vec<Unit>, String> {
    if names.is_empty() {
        return Ok(Unit::ALL.to_vec());
    }
    let mut want = BTreeSet::new();
    for n in names {
        if let Some(u) = Unit::ALL.iter().position(|u| u.name() == n) {
            want.insert(u);
        } else if let Some((_, us)) = GROUPS.iter().find(|(g, _)| g == n) {
            want.extend(
                us.iter()
                    .map(|u| Unit::ALL.iter().position(|a| a == u).unwrap()),
            );
        } else {
            let known: Vec<&str> = Unit::ALL
                .iter()
                .map(|u| u.name())
                .chain(GROUPS.iter().map(|(g, _)| *g))
                .collect();
            return Err(format!("unknown unit {n} (known: {})", known.join(" ")));
        }
    }
    Ok(want.into_iter().map(|i| Unit::ALL[i]).collect())
}

/// One target file: its live content and what vega would write.
#[derive(Debug, Clone, PartialEq)]
pub struct FilePlan {
    pub path: PathBuf,
    /// None when missing.
    pub live: Option<String>,
    /// The target is a symlink: a whole-file target replaces it.
    pub link: bool,
    pub text: String,
    /// Key-merge targets are written through a symlink, whole files are not.
    pub follow: bool,
}

impl FilePlan {
    pub fn changed(&self) -> bool {
        (self.link && !self.follow) || self.live.as_deref() != Some(self.text.as_str())
    }
}

/// What a unit would do.
#[derive(Debug, Clone, Default)]
pub struct Plan {
    pub files: Vec<FilePlan>,
    pub notes: Vec<String>,
    /// The sources it renders from.
    pub sources: Vec<Source>,
    /// Why it does nothing on this host.
    pub skip: Option<String>,
    /// Seeded keys to record once written (`claude.settings`).
    pub seeds: Option<Map<String, Value>>,
    /// The global skills and the ones missing (`skills`).
    pub skills: Option<(Vec<String>, Vec<String>)>,
}

impl Plan {
    fn skip(why: impl Into<String>) -> Plan {
        Plan {
            skip: Some(why.into()),
            ..Plan::default()
        }
    }
}

/// The live content of `p` (None when missing) and whether it is a symlink.
fn read_live(p: &Path) -> Result<(Option<String>, bool), String> {
    let link = fs::symlink_metadata(p).is_ok_and(|m| m.file_type().is_symlink());
    match fs::read_to_string(p) {
        Ok(t) => Ok((Some(t), link)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok((None, link)),
        Err(e) => Err(format!("{}: {e}", p.display())),
    }
}

fn whole(path: PathBuf, text: String) -> Result<FilePlan, String> {
    let (live, link) = read_live(&path)?;
    Ok(FilePlan {
        path,
        live,
        link,
        text,
        follow: false,
    })
}

/// A key-merge plan: `merge` edits the parsed file (`{}` when missing; a
/// missing file is planned only with `create`). An unchanged value keeps the
/// live text byte for byte.
fn json_plan<T>(
    path: PathBuf,
    create: bool,
    merge: impl FnOnce(&mut Value) -> Result<T, String>,
) -> Result<Option<(FilePlan, T)>, String> {
    let (live, link) = read_live(&path)?;
    if live.is_none() && !create {
        return Ok(None);
    }
    let orig: Value = match &live {
        Some(t) => {
            serde_json::from_str(t).map_err(|e| format!("{}: not JSON: {e}", path.display()))?
        }
        None => Value::Object(Map::new()),
    };
    let mut v = orig.clone();
    let out = merge(&mut v).map_err(|e| format!("{}: {e}", path.display()))?;
    let text = match &live {
        Some(t) if v == orig => t.clone(),
        _ => serde_json::to_string_pretty(&v).expect("a JSON value serializes") + "\n",
    };
    Ok(Some((
        FilePlan {
            path,
            live,
            link,
            text,
            follow: true,
        },
        out,
    )))
}

/// `$XDG_STATE_HOME/vega/config.json`: the seeded values vega last set.
fn seeds_path(env: &Env) -> PathBuf {
    env.state.join("vega/config.json")
}

fn last_seeds(env: &Env) -> Map<String, Value> {
    fs::read_to_string(seeds_path(env))
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .and_then(|v| v.get(Unit::ClaudeSettings.name())?.as_object().cloned())
        .unwrap_or_default()
}

fn record_seeds(env: &Env, seeds: &Map<String, Value>) -> Result<(), String> {
    let p = seeds_path(env);
    let mut root = fs::read_to_string(&p)
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .filter(Value::is_object)
        .unwrap_or_else(|| Value::Object(Map::new()));
    root[Unit::ClaudeSettings.name()] = Value::Object(seeds.clone());
    if let Some(d) = p.parent() {
        fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
    }
    let text = serde_json::to_string_pretty(&root).expect("serializes") + "\n";
    fs::write(&p, text).map_err(|e| format!("{}: {e}", p.display()))
}

fn need_dir(env: &Env, rel: &str) -> Option<Plan> {
    (!env.home.join(rel).is_dir()).then(|| Plan::skip(format!("no ~/{rel}")))
}

fn instructions(env: &Env, dir: &str, file: &str, parts: &[&str]) -> Result<Plan, String> {
    if let Some(p) = need_dir(env, dir) {
        return Ok(p);
    }
    let (text, sources) = sources::instructions(env, parts)?;
    Ok(Plan {
        files: vec![whole(env.home.join(dir).join(file), text)?],
        sources,
        ..Plan::default()
    })
}

const AGENTSVIEW_DIRS: [&str; 2] = [
    ".claude/skills/agentsview-finding-history",
    ".agents/skills/agentsview-finding-history",
];

/// What `unit` would write on this host.
pub fn plan(ctx: &Ctx, unit: Unit) -> Result<Plan, String> {
    let env = &ctx.env;
    let home = &env.home;
    let mux = ctx.mux.display().to_string();
    match unit {
        Unit::DcgConfig => {
            let src = sources::get(env, "dcg/config.toml")?;
            // The default's pack glob must follow the targets under XDG_CONFIG_HOME.
            // Local sources retain their explicitly chosen paths.
            let text = if src.origin == Origin::Default && env.config != home.join(".config") {
                let mut doc = src
                    .text
                    .parse::<toml_edit::DocumentMut>()
                    .map_err(|e| e.to_string())?;
                if let Some(paths) = doc["packs"]["custom_paths"].as_array_mut() {
                    for path in paths.iter_mut() {
                        if let Some(suffix) = path
                            .as_str()
                            .and_then(|p| p.strip_prefix("~/.config/dcg/packs/"))
                        {
                            *path = toml_edit::Value::from(
                                env.config
                                    .join("dcg/packs")
                                    .join(suffix)
                                    .to_string_lossy()
                                    .as_ref(),
                            );
                        }
                    }
                }
                doc.to_string()
            } else {
                src.text.clone()
            };
            Ok(Plan {
                files: vec![whole(env.config.join("dcg/config.toml"), text)?],
                sources: vec![src],
                ..Plan::default()
            })
        }
        Unit::DcgPacks => {
            let mut plan = Plan::default();
            for (name, src) in sources::packs(env)? {
                plan.files.push(whole(
                    env.config.join("dcg/packs").join(name),
                    src.text.clone(),
                )?);
                plan.sources.push(src);
            }
            Ok(plan)
        }
        Unit::Herdr => {
            let path = herdr::path(env);
            let src = sources::get(env, "herdr/config.toml")?;
            let (live, link) = read_live(&path)?;
            let r = herdr::render(&src.text, &herdr::symbols(env), live.as_deref())?;
            Ok(Plan {
                notes: r
                    .kept
                    .iter()
                    .map(|(k, v)| format!("kept: {k} = {v} (herdr owns it)"))
                    .collect(),
                files: vec![FilePlan {
                    path,
                    live,
                    link,
                    text: r.text,
                    follow: false,
                }],
                sources: vec![src],
                ..Plan::default()
            })
        }
        Unit::ClaudeMd => instructions(env, ".claude", "CLAUDE.md", &sources::CLAUDE_PARTS),
        Unit::CodexMd => instructions(env, ".codex", "AGENTS.md", &sources::CODEX_PARTS),
        Unit::PiMd => instructions(env, ".pi/agent", "AGENTS.md", &sources::CLAUDE_PARTS),
        Unit::ClaudeStyles => {
            if let Some(p) = need_dir(env, ".claude") {
                return Ok(p);
            }
            let mut plan = Plan::default();
            for (name, src) in sources::styles(env)? {
                let path = home.join(".claude/output-styles").join(&name);
                plan.files.push(whole(path, src.text.clone())?);
                plan.sources.push(src);
            }
            Ok(plan)
        }
        Unit::ClaudeSettings => {
            if let Some(p) = need_dir(env, ".claude") {
                return Ok(p);
            }
            let src = sources::get(env, "claude/settings.json")?;
            let seeds = match serde_json::from_str::<Value>(&src.text) {
                Ok(Value::Object(m)) => m,
                _ => return Err(format!("{}: not a JSON object", src.rel)),
            };
            let last = last_seeds(env);
            let (f, s) = json_plan(home.join(".claude/settings.json"), true, |v| {
                merge::seed(v, &seeds, &last)
            })?
            .expect("created when missing");
            let live: Option<Value> = f.live.as_deref().and_then(|t| serde_json::from_str(t).ok());
            let notes = s
                .local
                .iter()
                .map(|k| {
                    let lv = live.as_ref().map(|l| l[k].to_string()).unwrap_or_default();
                    format!(
                        "local: {k} = {lv} stays until the source value ({}) changes",
                        seeds[k]
                    )
                })
                .collect();
            Ok(Plan {
                files: vec![f],
                notes,
                sources: vec![src],
                seeds: Some(seeds),
                ..Plan::default()
            })
        }
        Unit::ClaudeHooks => {
            if let Some(p) = need_dir(env, ".claude") {
                return Ok(p);
            }
            let (f, _) = json_plan(home.join(".claude/settings.json"), true, |v| {
                merge::claude_hooks(v, &mux)
            })?
            .expect("created when missing");
            Ok(Plan {
                files: vec![f],
                ..Plan::default()
            })
        }
        Unit::CodexHooks => {
            if let Some(p) = need_dir(env, ".codex") {
                return Ok(p);
            }
            let (f, _) = json_plan(home.join(".codex/hooks.json"), true, |v| {
                merge::codex_hooks(v, &mux)
            })?
            .expect("created when missing");
            Ok(Plan {
                files: vec![f],
                ..Plan::default()
            })
        }
        Unit::PiPrefix => {
            match json_plan(
                home.join(".pi/agent/settings.json"),
                false,
                merge::pi_prefix,
            )? {
                Some((f, _)) => Ok(Plan {
                    files: vec![f],
                    ..Plan::default()
                }),
                None => Ok(Plan::skip("no ~/.pi/agent/settings.json")),
            }
        }
        Unit::Skills => {
            let list = env.config.join("vega/skills-global");
            let Ok(text) = fs::read_to_string(&list) else {
                return Ok(Plan::skip("no ~/.config/vega/skills-global"));
            };
            let names: Vec<String> = text
                .lines()
                .map(str::trim)
                .filter(|l| !l.is_empty() && !l.starts_with('#'))
                .map(str::to_string)
                .collect();
            let missing: Vec<String> = names
                .iter()
                .filter(|n| {
                    !home
                        .join(".agents/skills")
                        .join(n)
                        .join("SKILL.md")
                        .exists()
                        || !home.join(".claude/skills").join(n).exists()
                })
                .cloned()
                .collect();
            Ok(Plan {
                skills: Some((names, missing)),
                ..Plan::default()
            })
        }
        Unit::Agentsview => {
            let src = sources::get(env, "agentsview/LOCAL-SETUP.md")?;
            let mut plan = Plan::default();
            for d in AGENTSVIEW_DIRS {
                let dir = home.join(d);
                if dir.join("SKILL.md").is_file() {
                    plan.files
                        .push(whole(dir.join("LOCAL-SETUP.md"), src.text.clone())?);
                }
            }
            if plan.files.is_empty() {
                return Ok(Plan::skip("agentsview generated no skill here"));
            }
            plan.sources.push(src);
            Ok(plan)
        }
    }
}

/// The paths chezmoi manages (`chezmoi managed -p absolute`); empty without
/// chezmoi. A chezmoi that fails is an error: vega must not guess.
pub fn chezmoi_managed(chezmoi: &OsStr) -> Result<BTreeSet<PathBuf>, String> {
    let out = match Command::new(chezmoi)
        .args(["managed", "-p", "absolute", "-i", "files,symlinks"])
        .output()
    {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(BTreeSet::new()),
        Err(e) => return Err(format!("chezmoi managed: {e}")),
        Ok(o) => o,
    };
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        let line = err
            .lines()
            .rev()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("");
        return Err(format!("chezmoi managed failed: {}", line.trim()));
    }
    Ok(String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(PathBuf::from)
        .collect())
}

/// The targets of `plan` chezmoi manages.
pub fn managed_targets(plan: &Plan, managed: &BTreeSet<PathBuf>) -> Vec<PathBuf> {
    plan.files
        .iter()
        .filter(|f| managed.contains(&f.path))
        .map(|f| f.path.clone())
        .collect()
}

/// Paths chezmoi manages inside the global skills' dirs (a warning only).
pub fn skills_overlap(plan: &Plan, managed: &BTreeSet<PathBuf>, home: &Path) -> Vec<PathBuf> {
    let Some((names, _)) = &plan.skills else {
        return vec![];
    };
    let dirs: Vec<PathBuf> = names
        .iter()
        .flat_map(|n| {
            [".agents/skills", ".claude/skills", ".pi/agent/skills"].map(|d| home.join(d).join(n))
        })
        .collect();
    managed
        .iter()
        .filter(|m| dirs.iter().any(|d| m.starts_with(d)))
        .cloned()
        .collect()
}

/// The unit's state for the status table.
pub fn state(plan: &Plan, managed: &[PathBuf]) -> String {
    if let Some(r) = &plan.skip {
        return format!("skipped: {r}");
    }
    if !managed.is_empty() {
        return "managed (chezmoi)".into();
    }
    if let Some((_, missing)) = &plan.skills {
        return if missing.is_empty() {
            "ok".into()
        } else {
            format!("missing: {}", missing.join(" "))
        };
    }
    let changed: Vec<&FilePlan> = plan.files.iter().filter(|f| f.changed()).collect();
    let n = plan.files.len();
    match changed.len() {
        0 => "ok".into(),
        c if changed.iter().all(|f| f.live.is_none() && !f.link) => {
            if n > 1 {
                format!("missing ({c} of {n})")
            } else {
                "missing".into()
            }
        }
        c if n > 1 => format!("differs ({c} of {n})"),
        _ => "differs".into(),
    }
}

/// Whether any source of `plan` is a local override.
pub fn has_local(plan: &Plan) -> bool {
    plan.sources.iter().any(|s| s.origin == Origin::Local)
}

/// What [`apply`] did with a unit.
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    /// Written files (with their backups) and further detail.
    Written(Vec<String>),
    Unchanged,
    /// chezmoi manages these targets: nothing written.
    Managed(Vec<PathBuf>),
    Skipped(String),
    /// `herdr config check` rejected the rendered config: its issues.
    Rejected(Vec<String>),
    Failed(String),
}

impl Outcome {
    /// The summary bucket.
    pub fn kind(&self) -> &'static str {
        match self {
            Outcome::Written(_) => "written",
            Outcome::Unchanged => "unchanged",
            Outcome::Managed(_) => "managed",
            Outcome::Skipped(_) => "skipped",
            Outcome::Rejected(_) => "rejected",
            Outcome::Failed(_) => "failed",
        }
    }

    pub fn failed(&self) -> bool {
        matches!(self, Outcome::Rejected(_) | Outcome::Failed(_))
    }
}

/// Where a write keeps the previous file.
pub fn backup_path(p: &Path) -> PathBuf {
    let name = p
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    p.with_file_name(format!("{name}.vega-bak"))
}

/// Saves the previous content as `<file>.vega-bak`, then replaces the file
/// via a temp file and a rename (a whole-file target replaces a symlink;
/// a key-merge target writes through it).
fn write(f: &FilePlan) -> Result<(), String> {
    let path = if f.follow {
        fs::canonicalize(&f.path).unwrap_or_else(|_| f.path.clone())
    } else {
        f.path.clone()
    };
    let dir = path.parent().ok_or("no parent directory")?;
    fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    if let Some(old) = &f.live {
        let bak = backup_path(&path);
        fs::write(&bak, old).map_err(|e| format!("{}: {e}", bak.display()))?;
    }
    let mode = fs::symlink_metadata(&path)
        .ok()
        .filter(|m| m.is_file())
        .map(|m| m.permissions().mode());
    let tmp = dir.join(format!(
        ".{}.vega-config.{}",
        path.file_name().and_then(|n| n.to_str()).unwrap_or("file"),
        std::process::id()
    ));
    let res = (|| {
        let mut file = fs::File::create(&tmp)?;
        file.write_all(f.text.as_bytes())?;
        file.sync_all()?;
        if let Some(m) = mode {
            fs::set_permissions(&tmp, fs::Permissions::from_mode(m))?;
        }
        fs::rename(&tmp, &path)
    })();
    if res.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    res.map_err(|e| format!("{}: {e}", path.display()))
}

/// Applies one unit: plans it afresh (units sharing a file see each other's
/// writes), refuses while chezmoi manages a target, writes what changed.
pub fn apply(ctx: &Ctx, unit: Unit, managed: &BTreeSet<PathBuf>) -> Outcome {
    let plan = match plan(ctx, unit) {
        Ok(p) => p,
        Err(e) => return Outcome::Failed(e),
    };
    if let Some(r) = plan.skip {
        return Outcome::Skipped(r);
    }
    let m = managed_targets(&plan, managed);
    if !m.is_empty() {
        return Outcome::Managed(m);
    }
    if unit == Unit::Skills {
        return sync_skills(&ctx.skills);
    }
    let changed: Vec<&FilePlan> = plan.files.iter().filter(|f| f.changed()).collect();
    let record = || match &plan.seeds {
        Some(s) => record_seeds(&ctx.env, s),
        None => Ok(()),
    };
    if changed.is_empty() {
        return match record() {
            Ok(()) => Outcome::Unchanged,
            Err(e) => Outcome::Failed(e),
        };
    }
    if unit == Unit::Herdr {
        match herdr::check(&changed[0].path, &changed[0].text, &ctx.herdr) {
            Err(e) => return Outcome::Failed(e),
            Ok(herdr::Check::NoHerdr) => return Outcome::Skipped("herdr not installed".into()),
            Ok(herdr::Check::Rejected(lines)) => return Outcome::Rejected(lines),
            Ok(herdr::Check::Ok) => {}
        }
    }
    let mut done = Vec::new();
    for f in &changed {
        if let Err(e) = write(f) {
            return Outcome::Failed(e);
        }
        done.push(f.path.display().to_string());
    }
    if let Err(e) = record() {
        return Outcome::Failed(e);
    }
    if unit == Unit::Herdr {
        done.push(match herdr::reload(&ctx.herdr) {
            Ok(()) => "reload ok".into(),
            Err(e) => format!("reload: {e} (herdr reads the file on its next start)"),
        });
    }
    Outcome::Written(done)
}

fn sync_skills(skills: &OsStr) -> Outcome {
    match Command::new(skills).args(["lib", "sync"]).output() {
        Err(e) => Outcome::Failed(format!("vega skills lib sync: {e}")),
        Ok(o) => {
            let text = String::from_utf8_lossy(&o.stdout).into_owned();
            let last = text
                .lines()
                .rev()
                .find(|l| !l.trim().is_empty())
                .unwrap_or("")
                .trim()
                .to_string();
            if !o.status.success() {
                let err = String::from_utf8_lossy(&o.stderr);
                let e = err
                    .lines()
                    .rev()
                    .find(|l| !l.trim().is_empty())
                    .unwrap_or(&last)
                    .trim()
                    .to_string();
                Outcome::Failed(format!("vega skills lib sync: {e}"))
            } else if text.starts_with("library: not configured") {
                Outcome::Skipped("library not configured".into())
            } else {
                Outcome::Written(vec![format!("vega skills lib sync: {last}")])
            }
        }
    }
}

#[cfg(test)]
mod tests;
