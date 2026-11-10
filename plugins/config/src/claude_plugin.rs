//! A local marketplace installed through Claude's CLI, never by writing its registry.
use super::*;
use serde_json::json;
use sha2::{Digest, Sha256};

const ID: &str = "vega-hooks@vega-local";
const HOOKS: &str = "plugins/vega-hooks/hooks/hooks.json";

fn root(ctx: &Ctx) -> PathBuf {
    ctx.env.home.join(".local/share/vega/claude-marketplace")
}

fn command(ctx: &Ctx, args: &[&OsStr]) -> Result<String, String> {
    let out = Command::new(&ctx.claude)
        .args(args)
        .env("HOME", &ctx.env.home)
        // Never inherit another session's config directory (also isolates tests).
        .env("CLAUDE_CONFIG_DIR", ctx.env.home.join(".claude"))
        .env_remove("CLAUDECODE")
        .current_dir(&ctx.env.home)
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| {
            format!(
                "claude plugin: {e}; install Claude Code then run vega config apply claude.hooks"
            )
        })?;
    if !out.status.success() {
        return Err(format!(
            "claude plugin {}: {} {}",
            out.status,
            String::from_utf8_lossy(&out.stdout).trim(),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

fn run(ctx: &Ctx, args: &[&str]) -> Result<String, String> {
    command(ctx, &args.iter().map(OsStr::new).collect::<Vec<_>>())
}

fn listing(ctx: &Ctx) -> Result<Vec<Value>, String> {
    serde_json::from_str(&run(ctx, &["plugin", "list", "--json"])?)
        .map_err(|e| format!("claude plugin list: invalid JSON: {e}"))
}

fn entry(list: &[Value]) -> Option<&Value> {
    list.iter().find(|v| v["id"] == ID && v["scope"] == "user")
}

fn active(list: &[Value], hooks: &str) -> bool {
    entry(list).is_some_and(|v| {
        // Local marketplaces may load directly from readFromFolder instead of cache.
        let path = v
            .get("readFromFolder")
            .and_then(Value::as_str)
            .or_else(|| v["installPath"].as_str());
        v["enabled"] == true
            && path.is_some_and(|p| {
                fs::read_to_string(Path::new(p).join("hooks/hooks.json"))
                    .ok()
                    .as_deref()
                    == Some(hooks)
            })
    })
}

pub(super) fn plan(ctx: &Ctx) -> Result<Plan, String> {
    if let Some(p) = need_dir(&ctx.env, ".claude") {
        return Ok(p);
    }
    let mut hooks = json!({});
    merge::claude_hooks(&mut hooks, &ctx.mux.to_string_lossy())?;
    let hooks = serde_json::to_string_pretty(&hooks).unwrap() + "\n";
    // Content versioning refreshes Claude's immutable plugin cache after path/hook changes.
    let version = format!("1.0.0-{:x}", Sha256::digest(hooks.as_bytes()));
    let manifest = json!({"name":"vega-hooks", "version":version,
        "description":"Vega parent questions, shell shim activation and shadow pane state"});
    let marketplace = json!({"name":"vega-local", "owner":{"name":"vega"},
        "plugins":[{"name":"vega-hooks", "source":"./plugins/vega-hooks", "version":version}]});
    let list = match listing(ctx) {
        Ok(list) => list,
        Err(e)
            if !Path::new(&ctx.claude).exists()
                && vega_plugin::find_on_path(&ctx.claude.to_string_lossy()).is_none() =>
        {
            return Ok(Plan::skip(e));
        }
        Err(e) => return Err(e),
    };
    let pending = !active(&list, &hooks);
    let base = root(ctx);
    let mut files = vec![
        whole(
            base.join(".claude-plugin/marketplace.json"),
            serde_json::to_string_pretty(&marketplace).unwrap() + "\n",
        )?,
        whole(
            base.join("plugins/vega-hooks/.claude-plugin/plugin.json"),
            serde_json::to_string_pretty(&manifest).unwrap() + "\n",
        )?,
        whole(base.join(HOOKS), hooks)?,
    ];
    // Preview only: apply re-reads this AFTER Claude's own settings writes and verification.
    if let Some((f, _)) = json_plan(
        ctx.env.home.join(".claude/settings.json"),
        true,
        merge::remove_claude_hooks,
    )? {
        files.push(f);
    }
    Ok(Plan {
        files,
        plugin_pending: pending,
        notes: if pending {
            vec!["install/enable vega-hooks@vega-local in user scope before migrating settings hooks; restart existing Claude sessions".into()]
        } else {
            vec![]
        },
        ..Plan::default()
    })
}

pub(super) fn apply(ctx: &Ctx, plan: &Plan, changes: &mut Vec<Value>) -> Outcome {
    match install(ctx, plan, changes) {
        Ok(done) if done.is_empty() => Outcome::Unchanged,
        Ok(done) => Outcome::Written(done),
        Err(e) => Outcome::Failed(e),
    }
}

fn install(ctx: &Ctx, plan: &Plan, changes: &mut Vec<Value>) -> Result<Vec<String>, String> {
    let mut done = Vec::new();
    let settings = ctx.env.home.join(".claude/settings.json");
    for f in plan
        .files
        .iter()
        .filter(|f| f.path != settings && f.changed())
    {
        write(f)?;
        changes.push(evidence::change(ctx, Unit::ClaudeHooks, plan, f));
        done.push(f.path.display().to_string());
    }
    let hooks = fs::read_to_string(root(ctx).join(HOOKS)).map_err(|e| e.to_string())?;
    if plan.plugin_pending {
        let operation = registration(ctx, plan, changes, || {
            let base = root(ctx);
            command(
                ctx,
                &[
                    OsStr::new("plugin"),
                    OsStr::new("marketplace"),
                    OsStr::new("add"),
                    base.as_os_str(),
                ],
            )?;
            let list = listing(ctx)?;
            let operation = if entry(&list).is_some() {
                "update"
            } else {
                "install"
            };
            run(ctx, &["plugin", operation, ID, "--scope", "user"])?;
            if entry(&listing(ctx)?).is_some_and(|v| v["enabled"] != true) {
                run(ctx, &["plugin", "enable", ID, "--scope", "user"])?;
            }
            Ok(operation)
        })?;
        done.push(format!(
            "{ID}: {operation}, user scope; restart existing Claude sessions"
        ));
    }
    if !active(&listing(ctx)?, &hooks) {
        return Err(format!("{ID}: enabled user plugin with current hooks not verified; legacy settings hooks retained"));
    }
    // Claude commands may have changed settings.json. Do not write the stale preview.
    if let Some((f, _)) = json_plan(settings, false, merge::remove_claude_hooks)? {
        if f.changed() {
            write(&f)?;
            changes.push(evidence::change(ctx, Unit::ClaudeHooks, plan, &f));
            done.push(f.path.display().to_string());
        }
    }
    Ok(done)
}

/// Claude owns registration writes. Observe its managed settings keys without
/// claiming a vega backup, and retain partial changes if a CLI operation fails.
fn registration<T>(
    ctx: &Ctx,
    plan: &Plan,
    changes: &mut Vec<Value>,
    mutate: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    let path = ctx.env.home.join(".claude/settings.json");
    let (before, _) = read_live(&path)?;
    let result = mutate();
    let (after, link) = read_live(&path)?;
    if before != after {
        let file = FilePlan {
            path,
            live: before,
            link,
            text: after.unwrap_or_else(|| "{}".into()),
            follow: true,
            remove: false,
        };
        let mut change = evidence::change(ctx, Unit::ClaudeHooks, plan, &file);
        change["backup_refreshed"] = Value::Null;
        change["writer"] = Value::from("claude_plugin_cli");
        if let Some(keys) = change["keys"].as_array_mut() {
            keys.retain(|key| {
                key["key"].as_str().is_some_and(|p| {
                    p == "/enabledPlugins/vega-hooks@vega-local"
                        || p == "/extraKnownMarketplaces/vega-local"
                        || p.starts_with("/extraKnownMarketplaces/vega-local/")
                })
            });
        }
        changes.push(change);
    }
    result
}
