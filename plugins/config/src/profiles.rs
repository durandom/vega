//! Independent herdr-app homes; no app or terminal mutations during config apply.
use crate::{sources, Ctx, Env, Outcome, Plan, Unit};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    process::Command,
};

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    pub layout: String,
    #[serde(default)]
    pub mods: Vec<String>,
    #[serde(default)]
    pub settings: BTreeMap<String, Value>,
    #[serde(default)]
    pub shared: BTreeMap<String, Value>,
    #[serde(default)]
    pub terminals: Vec<Terminal>,
    #[serde(default, rename = "tab")]
    pub tabs: Vec<Tab>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Tab {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    pub terminals: Vec<Terminal>,
}

impl Profile {
    pub fn terminals(&self) -> impl Iterator<Item = &Terminal> {
        self.terminals
            .iter()
            .chain(self.tabs.iter().flat_map(|tab| &tab.terminals))
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Terminal {
    pub name: String,
    pub machine: String,
    pub argv: Vec<String>,
    #[serde(default)]
    pub optional: bool,
}

pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 32
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
}

pub fn parse(text: &str) -> Result<Profile, String> {
    let p: Profile = toml_edit::de::from_str(text).map_err(|e| format!("profile: {e}"))?;
    let mut names = BTreeSet::new();
    if !valid_name(&p.layout) || p.mods.iter().any(|m| !valid_name(m)) {
        return Err(
            "profile layout/mod names must be 1-32 lowercase letters, digits, - or _".into(),
        );
    }
    if p.shared.contains_key("layout") {
        return Err("profile shared.layout is reserved for the generated window layout".into());
    }
    if !p.tabs.is_empty() && !p.terminals.is_empty() {
        return Err("profile cannot mix tab groups and flat terminals".into());
    }
    if p.terminals.iter().any(|t| t.optional) {
        return Err("optional terminals require tab groups".into());
    }
    if !p.tabs.is_empty() && !p.mods.iter().any(|m| m == "vega-window-tabs") {
        return Err(
            "tab profiles require mods = [\"vega-window-tabs\"] (plus any other mods)".into(),
        );
    }
    let mut tabs = BTreeSet::new();
    for tab in &p.tabs {
        if !valid_name(&tab.name)
            || !tabs.insert(&tab.name)
            || !(1..=2).contains(&tab.terminals.len())
        {
            return Err("profile tabs need unique valid names and one or two terminals".into());
        }
    }
    if p.terminals().next().is_none() && !p.mods.iter().any(|m| m == "vega-window-tabs") {
        return Err("profile needs at least one terminal".into());
    }
    for t in p.terminals() {
        if !valid_name(&t.name)
            || !names.insert(&t.name)
            || t.machine.is_empty()
            || t.machine.starts_with('-')
            || (t.optional && t.argv.is_empty())
            || t.argv.first().is_some_and(|a| a.is_empty())
            || t.argv.iter().any(|a| a.contains('\0'))
        {
            return Err(
                "profile terminals need unique valid names, a machine and valid argv (empty starts a shell; optional requires a program)".into(),
            );
        }
    }
    Ok(p)
}

pub fn home(env: &Env, name: &str) -> Result<PathBuf, String> {
    if !valid_name(name) {
        return Err("profile name must be 1-32 lowercase letters, digits, - or _".into());
    }
    Ok(env.config.join(format!("herdr-app-{name}")))
}

pub fn list(env: &Env) -> Result<Vec<(String, sources::Source)>, String> {
    sources::directory(env, "herdr-app/profiles", &[".toml"])
}

fn merge(dst: &mut Value, src: Value) {
    if let (Some(d), Some(s)) = (dst.as_object_mut(), src.as_object()) {
        for (k, v) in s {
            merge(d.entry(k.clone()).or_insert(Value::Null), v.clone());
        }
    } else {
        *dst = src;
    }
}

/// Substitute explicit profile slots only, never arbitrary strings or argv.
pub fn resolve_layout(
    layout: &mut Value,
    terminals: &BTreeMap<String, String>,
) -> Result<(), String> {
    match layout {
        Value::Object(o) => {
            if let Some(slot) = o
                .get("terminal")
                .and_then(Value::as_str)
                .and_then(|s| s.strip_prefix("profile:"))
            {
                let address = terminals
                    .get(slot)
                    .ok_or_else(|| format!("unknown profile terminal slot {slot}"))?;
                o.insert("terminal".into(), address.clone().into());
            }
            for v in o.values_mut() {
                resolve_layout(v, terminals)?;
            }
        }
        Value::Array(a) => {
            for v in a {
                resolve_layout(v, terminals)?;
            }
        }
        _ => {}
    }
    Ok(())
}

/// A layout-owned stack keeps cross-machine terminals out of shared hc spaces.
pub fn build_tabs(
    layout: &mut Value,
    profile: &Profile,
    addresses: &BTreeMap<String, String>,
) -> Result<(), String> {
    if profile.tabs.is_empty() && !profile.mods.iter().any(|m| m == "vega-window-tabs") {
        return Ok(());
    }
    fn replace(node: &mut Value, children: &[Value]) -> usize {
        if node["id"] == "profile:tabs" {
            node["type"] = "stack".into();
            node["children"] = children.to_vec().into();
            node["active"] = children
                .first()
                .map(|c| c["id"].clone())
                .unwrap_or(Value::Null);
            return 1;
        }
        node.get_mut("children")
            .and_then(Value::as_array_mut)
            .map(|nodes| nodes.iter_mut().map(|n| replace(n, children)).sum())
            .unwrap_or(0)
    }
    let children: Vec<_> = profile.tabs.iter().filter_map(|tab| {
        let blocks: Vec<_> = tab.terminals.iter().filter_map(|t| addresses.get(&t.name).map(|address|
            json!({"id": format!("slot-{}", t.name), "type":"block", "content":{"terminal":address}})
        )).collect();
        if blocks.is_empty() { return None; }
        Some(json!({"id":tab.name, "type":"row", "gap":4, "children":blocks}))
    }).collect();

    if replace(layout, &children) != 1 {
        return Err("tab profile layout needs exactly one profile:tabs node".into());
    }
    Ok(())
}

pub fn plan(ctx: &Ctx) -> Result<Plan, String> {
    if let Some(p) = crate::need_herdr_app(&ctx.env) {
        return Ok(p);
    }
    let mut plan = Plan::default();
    for (file, src) in list(&ctx.env)? {
        let name = file.trim_end_matches(".toml");
        let target = home(&ctx.env, name)?;
        let profile = parse(&src.text)?;
        let layout_src = sources::get(
            &ctx.env,
            &format!("herdr-app/layouts/{}.json", profile.layout),
        )?;
        let mut layout: Value =
            serde_json::from_str(&layout_src.text).map_err(|e| e.to_string())?;
        if !matches!(
            layout["type"].as_str(),
            Some("row" | "column" | "stack" | "block")
        ) {
            return Err("profile layout needs a valid root type".into());
        }
        let placeholders = profile
            .terminals()
            .map(|t| (t.name.clone(), "t0".into()))
            .collect();
        build_tabs(&mut layout, &profile, &placeholders)?;
        resolve_layout(
            &mut layout,
            &profile
                .terminals()
                .map(|t| (t.name.clone(), "t0".into()))
                .collect(),
        )?;
        plan.files
            .push(crate::whole(target.join("profile.toml"), src.text.clone())?);
        plan.files.push(crate::whole(
            target.join("profile-layout.json"),
            layout_src.text.clone(),
        )?);
        plan.sources.extend([src, layout_src]);
        let settings_src = sources::get(&ctx.env, "herdr-app/settings.json")?;
        let mut settings: Value =
            serde_json::from_str(&settings_src.text).map_err(|e| e.to_string())?;
        merge(&mut settings, json!(profile.settings));
        plan.files.push(crate::whole(
            target.join("settings.json"),
            serde_json::to_string_pretty(&settings).unwrap() + "\n",
        )?);
        plan.sources.push(settings_src);
        if profile.mods.iter().any(|m| m == "vega-window-tabs") {
            let keys_src = sources::get(&ctx.env, "herdr-app/keys.json")?;
            let mut keys: Value =
                serde_json::from_str(&keys_src.text).map_err(|e| e.to_string())?;
            if let Some(global) = keys["global"].as_object_mut() {
                for binding in global.values_mut() {
                    if let Some(command) = binding.as_str() {
                        if let Some(action) = command.strip_prefix("tab.") {
                            if matches!(action, "next" | "prev" | "new" | "close" | "rename")
                                || action.starts_with("select:")
                            {
                                *binding =
                                    format!("vega-window-tabs.{}", action.replace(':', "-")).into();
                            }
                        }
                    }
                }
            }
            plan.files.push(crate::whole(
                target.join("keys.json"),
                serde_json::to_string_pretty(&keys).unwrap() + "\n",
            )?);
            plan.sources.push(keys_src);
        }

        for (name, src) in sources::herdr_app_themes(&ctx.env)? {
            plan.files.push(crate::whole(
                target.join("themes").join(name),
                src.text.clone(),
            )?);
            plan.sources.push(src);
        }
        // Shared state is app-owned: seed once, never merge into a live home.
        if !target.join("shared.json").exists() {
            let mut shared = json!(profile.shared);
            shared["layout"] = "vega-window".into();
            plan.files.push(crate::whole(
                target.join("shared.json"),
                serde_json::to_string_pretty(&shared).unwrap() + "\n",
            )?);
        }
        for name in &profile.mods {
            let dir = format!("herdr-app/mods/{name}");
            let mut files: BTreeSet<String> = sources::DEFAULTS
                .iter()
                .filter_map(|(r, _)| r.strip_prefix(&format!("{dir}/")).map(str::to_string))
                .collect();
            for rel in sources::local_files(&ctx.env) {
                if let Some(file) = rel.strip_prefix(&format!("{dir}/")) {
                    files.insert(file.into());
                }
            }
            if !files.contains("main.js") || !files.contains("mod.json") {
                return Err(format!(
                    "profile mod {name}: main.js and mod.json sources required"
                ));
            }
            for file in files {
                if matches!(file.as_str(), "gpui-kit.d.ts" | "jsconfig.json") {
                    continue;
                }
                let src = sources::get(&ctx.env, &format!("{dir}/{file}"))?;
                plan.files.push(crate::whole(
                    target.join("mods").join(name).join(file),
                    src.text.clone(),
                )?);
                plan.sources.push(src);
            }
        }
    }
    Ok(plan)
}

pub fn apply(ctx: &Ctx, plan: &Plan, evidence: &mut Vec<Value>) -> Outcome {
    let changed: Vec<_> = plan.files.iter().filter(|f| f.changed()).collect();
    if changed.is_empty() {
        return Outcome::Unchanged;
    }
    // Validate every selected mod in an isolated home before writing any profile.
    let checked = || -> Result<(), String> {
        let stage = tempfile::tempdir().map_err(|e| e.to_string())?;
        let mut mods = BTreeSet::new();
        for f in &plan.files {
            let rel = f
                .path
                .strip_prefix(&ctx.env.config)
                .map_err(|e| e.to_string())?;
            let path = stage.path().join(rel);
            fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
            fs::write(&path, &f.text).map_err(|e| e.to_string())?;
            if path.file_name().is_some_and(|n| n == "mod.json") {
                mods.insert(path.parent().unwrap().to_path_buf());
            }
        }
        for path in mods {
            let out = Command::new(&ctx.herdr_app)
                .arg("check")
                .arg(&path)
                .env("HERDR_APP_HOME", path.parent().unwrap().parent().unwrap())
                .output()
                .map_err(|e| e.to_string())?;
            if !out.status.success() {
                return Err(format!("profile mod check failed: {}", path.display()));
            }
        }
        Ok(())
    };
    if let Err(e) = checked() {
        return Outcome::Rejected(vec![e]);
    }
    let mut written = Vec::new();
    for f in changed {
        if let Err(e) = crate::write(f) {
            return Outcome::Failed(e);
        }
        evidence.push(crate::evidence::change(
            ctx,
            Unit::HerdrAppProfiles,
            plan,
            f,
        ));
        written.push(f.path.display().to_string());
    }
    Outcome::Written(written)
}

pub fn read_applied(home: &Path) -> Result<(Profile, Value), String> {
    let profile = parse(&fs::read_to_string(home.join("profile.toml")).map_err(|e| {
        format!("profile not applied: {e}; run vega config apply herdr-app.profiles")
    })?)?;
    let layout = serde_json::from_slice(
        &fs::read(home.join("profile-layout.json")).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    Ok((profile, layout))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn ctx(dir: &Path) -> Ctx {
        Ctx {
            env: Env {
                home: dir.into(),
                config: dir.join("config"),
                state: dir.join("state"),
                pi_package: None,
            },
            herdr_app: "/usr/bin/true".into(),
            chezmoi: "false".into(),
            mux: "false".into(),
            skills: "false".into(),
            claude: "false".into(),
        }
    }
    #[test]
    fn profiles_skip_without_app_home_before_reading_sources_or_checking_mods() {
        let temp = tempfile::tempdir().unwrap();
        let ctx = ctx(temp.path());
        let local = sources::local_dir(&ctx.env).join("herdr-app/profiles");
        fs::create_dir_all(&local).unwrap();
        fs::write(local.join("status.toml"), "invalid profile").unwrap();
        let p = plan(&ctx).unwrap();
        assert!(p.skip.as_ref().unwrap().contains("herdr-app not set up"));
        assert!(p.files.is_empty());
        assert!(matches!(
            crate::apply(&ctx, Unit::HerdrAppProfiles, &BTreeSet::new()),
            Outcome::Skipped(_)
        ));
        assert!(!home(&ctx.env, "status").unwrap().exists());
        // A configured app still reports invalid sources rather than hiding them.
        fs::create_dir_all(crate::herdr_app::home(&ctx.env)).unwrap();
        assert!(plan(&ctx).is_err());
    }

    #[test]
    fn profile_apply_is_isolated_idempotent_and_preserves_shared_state() {
        let temp = tempfile::tempdir().unwrap();
        let ctx = ctx(temp.path());
        let main = ctx.env.config.join("herdr-app");
        fs::create_dir_all(&main).unwrap();
        fs::write(main.join("settings.json"), "main home sentinel").unwrap();
        let p = plan(&ctx).unwrap();
        assert!(matches!(apply(&ctx, &p, &mut vec![]), Outcome::Written(_)));
        let target = home(&ctx.env, "status").unwrap();
        let (profile, _) = read_applied(&target).unwrap();
        assert_eq!(profile.terminals().count(), 4);
        assert_eq!(profile.tabs.len(), 4);
        let keys: Value =
            serde_json::from_slice(&fs::read(target.join("keys.json")).unwrap()).unwrap();
        assert_eq!(keys["global"]["ctrl-tab"], "vega-window-tabs.next");
        assert_eq!(keys["global"]["cmd-2"], "vega-window-tabs.select-2");
        assert_eq!(keys["global"]["ctrl-s p"], "vega-window-tabs.prev");
        assert_eq!(keys["global"]["ctrl-s c"], "vega-window-tabs.new");
        assert_eq!(keys["global"]["cmd-w"], "vega-window-tabs.close");
        assert_eq!(keys["global"]["f2"], "vega-window-tabs.rename");
        assert_eq!(keys["terminal"]["cmd-c"], "terminal.copy");
        let shared: Value =
            serde_json::from_slice(&fs::read(target.join("shared.json")).unwrap()).unwrap();
        assert_eq!(shared["frame.welcomed"], "finished");
        assert_eq!(shared["layout"], "vega-window");
        fs::write(target.join("shared.json"), "app owns this, even malformed").unwrap();
        assert!(matches!(
            apply(&ctx, &plan(&ctx).unwrap(), &mut vec![]),
            Outcome::Unchanged
        ));
        assert_eq!(
            fs::read_to_string(target.join("shared.json")).unwrap(),
            "app owns this, even malformed"
        );
        assert_eq!(
            fs::read_to_string(main.join("settings.json")).unwrap(),
            "main home sentinel"
        );
        assert!(!target.join("herdr-app.sock").exists());
    }
    #[test]
    fn grouped_profile_validation_and_split_building() {
        let source = sources::default("herdr-app/profiles/status.toml").unwrap();
        for bad in [
            source.replace("name = \"io\"", "name = \"proteus\""),
            source.replace("name = \"io\"", "name = \"../io\""),
            source.replace(
                "[[tab.terminals]]\nname = \"io-btop\"",
                "[[terminals]]\nname = \"io-btop\"",
            ),
            source.replace("mods = [\"vega-window-tabs\"]", "mods = []"),
        ] {
            assert!(parse(&bad).is_err(), "accepted {bad}");
        }
        // inline-md: allow (TOML parser test fixture, not guide/help)
        let p = parse(
            r#"
layout = "status"
mods = ["vega-window-tabs"]
[[tab]]
name = "pair"
[[tab.terminals]]
name = "a"
machine = "one"
argv = ["btop"]
[[tab.terminals]]
name = "b"
machine = "two"
argv = ["btop"]
[[tab]]
name = "optional"
[[tab.terminals]]
name = "c"
machine = "three"
argv = ["btop"]
optional = true
"#,
        )
        .unwrap();
        let mut layout = json!({"id":"profile:tabs"});
        build_tabs(
            &mut layout,
            &p,
            &BTreeMap::from([("a".into(), "one/t1".into()), ("b".into(), "two/t2".into())]),
        )
        .unwrap();
        assert_eq!(layout["type"], "stack");
        assert_eq!(layout["active"], "pair");
        assert_eq!(layout["children"].as_array().unwrap().len(), 1);
        assert_eq!(
            layout["children"][0]["children"].as_array().unwrap().len(),
            2
        );
        assert_eq!(
            layout["children"][0]["children"][1]["content"]["terminal"],
            "two/t2"
        );
        assert!(build_tabs(
            &mut json!({"id":"wrong"}),
            &p,
            &BTreeMap::from([("a".into(), "t1".into())])
        )
        .is_err());
        assert!(build_tabs(&mut layout, &p, &BTreeMap::new()).is_ok());
        assert!(parse("layout = \"status\"\n[[tab]]\nname = \"empty\"\nterminals = []").is_err());
        let flat = parse("layout = \"status\"\n[[terminals]]\nname = \"legacy\"\nmachine = \"local\"\nargv = [\"btop\"]").unwrap();
        assert_eq!(flat.terminals().count(), 1);
        assert!(flat.tabs.is_empty());
    }

    #[test]
    fn invalid_profiles_fail_before_writes() {
        let source = sources::default("herdr-app/profiles/status.toml").unwrap();
        assert!(parse(&source.replace("layout = \"status\"", "layout = \"../status\"")).is_err());
        assert!(parse(&source.replace("ganymede-btop", "proteus-btop")).is_err());
        assert!(parse(&format!("unknown = true\n{source}")).is_err());
        assert!(parse(&source.replace("argv = [\"btop\"]", "argv = []")).is_err());
        let temp = tempfile::tempdir().unwrap();
        assert!(home(&ctx(temp.path()).env, "../main").is_err());
        assert!(resolve_layout(
            &mut json!({"content":{"terminal":"profile:unknown"}}),
            &BTreeMap::new()
        )
        .is_err());
    }
    #[test]
    fn local_profiles_and_mods_use_source_overrides_and_check_before_apply() {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().unwrap();
        let mut ctx = ctx(temp.path());
        fs::create_dir_all(crate::herdr_app::home(&ctx.env)).unwrap();
        let local = sources::local_dir(&ctx.env);
        fs::create_dir_all(local.join("herdr-app/profiles")).unwrap();
        fs::create_dir_all(local.join("herdr-app/mods/third-party")).unwrap();
        let source = sources::default("herdr-app/profiles/status.toml")
            .unwrap()
            .replace(
                "mods = [\"vega-window-tabs\"]",
                "mods = [\"vega-window-tabs\", \"third-party\"]",
            );
        fs::write(local.join("herdr-app/profiles/status.toml"), source).unwrap();
        fs::write(
            local.join("herdr-app/mods/third-party/main.js"),
            "export default class {};",
        )
        .unwrap();
        fs::write(local.join("herdr-app/mods/third-party/mod.json"), "{}").unwrap();
        let p = plan(&ctx).unwrap();
        ctx.herdr_app = "/usr/bin/false".into();
        assert!(matches!(apply(&ctx, &p, &mut vec![]), Outcome::Rejected(_)));
        assert!(!home(&ctx.env, "status").unwrap().exists());
        let fake = temp.path().join("fake-app");
        fs::write(&fake, "#!/bin/sh\n[ \"$1\" = check ] && [ -f \"$2/main.js\" ] && [ -n \"$HERDR_APP_HOME\" ]\n").unwrap();
        fs::set_permissions(&fake, fs::Permissions::from_mode(0o755)).unwrap();
        ctx.herdr_app = fake.into_os_string();
        assert!(matches!(apply(&ctx, &p, &mut vec![]), Outcome::Written(_)));
        assert!(home(&ctx.env, "status")
            .unwrap()
            .join("mods/third-party/main.js")
            .exists());
    }
}
