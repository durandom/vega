//! Key-merges into JSON files other tools write too: vega changes only its
//! own keys or hook entries and keeps everything else in its order
//! (docs/design/config.md "Ownership rules"). Moved from `vega mux hook
//! install` (plugins/mux/src/hook.rs) and `scripts/install-cli.sh`.

use serde_json::{json, Map, Value};

/// The dialog tools the ask-parent hook intercepts, also its matcher. Codex
/// matches `|`-separated exact names, so the sync name alone misses the async
/// tool; changing it costs one "Hooks need review" per host (the trust hash
/// covers the matcher, docs/design/pane-control.md).
pub const CLAUDE_TOOL: &str = "AskUserQuestion";
pub const CODEX_TOOL: &str = "request_user_input|request_user_input_async";
/// Marks the ask-parent hook entry (its command ends with it).
pub const ASK_PARENT: &str = "hook ask-parent";
/// The P3 shim activation line (docs/design/agent-tooling-principles.md).
pub const ACTIVATE_LINE: &str = ". ~/.local/share/vega/shims/activate.sh";
/// Marks the activation hook entry.
const ACTIVATE_MARK: &str = "shims/activate.sh";

/// The ask-parent PreToolUse entry for `tool`, running `mux`.
pub fn ask_parent_entry(mux: &str, tool: &str) -> Value {
    json!({
        "matcher": tool,
        "hooks": [{"type": "command", "command": format!("'{mux}' {ASK_PARENT}"), "timeout": 10}],
    })
}

/// Claude's SessionStart entry that sources the activation line per tool shell.
pub fn activate_entry() -> Value {
    json!({
        "hooks": [{"type": "command", "command": format!("echo '{ACTIVATE_LINE}' >> \"$CLAUDE_ENV_FILE\"")}],
    })
}

/// Puts `entry` under `hooks.<event>`, replacing the entry whose hook
/// commands contain `mark`; every other entry stays. Returns whether it changed.
pub fn upsert_hook(
    root: &mut Value,
    event: &str,
    mark: &str,
    entry: Value,
) -> Result<bool, String> {
    let hooks = root
        .as_object_mut()
        .ok_or("not a JSON object")?
        .entry("hooks")
        .or_insert_with(|| json!({}));
    let list = hooks
        .as_object_mut()
        .ok_or("hooks is not an object")?
        .entry(event)
        .or_insert_with(|| json!([]))
        .as_array_mut()
        .ok_or(format!("hooks.{event} is not an array"))?;
    let ours = |e: &Value| {
        e["hooks"].as_array().is_some_and(|hs| {
            hs.iter()
                .any(|h| h["command"].as_str().is_some_and(|c| c.contains(mark)))
        })
    };
    match list.iter().position(ours) {
        Some(i) if list[i] == entry => Ok(false),
        Some(i) => {
            list[i] = entry;
            Ok(true)
        }
        None => {
            list.push(entry);
            Ok(true)
        }
    }
}

/// `claude.hooks`: ask-parent and the activation hook.
pub fn claude_hooks(root: &mut Value, mux: &str) -> Result<bool, String> {
    let a = upsert_hook(
        root,
        "PreToolUse",
        ASK_PARENT,
        ask_parent_entry(mux, CLAUDE_TOOL),
    )?;
    let b = upsert_hook(root, "SessionStart", ACTIVATE_MARK, activate_entry())?;
    Ok(a || b)
}

/// `codex.hooks`: ask-parent.
pub fn codex_hooks(root: &mut Value, mux: &str) -> Result<bool, String> {
    upsert_hook(
        root,
        "PreToolUse",
        ASK_PARENT,
        ask_parent_entry(mux, CODEX_TOOL),
    )
}

/// `pi.prefix`: the activation line first in `shellCommandPrefix`, an
/// existing prefix kept after it.
pub fn pi_prefix(root: &mut Value) -> Result<bool, String> {
    let o = root.as_object_mut().ok_or("not a JSON object")?;
    let old = o
        .get("shellCommandPrefix")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    if old.contains(ACTIVATE_LINE) {
        return Ok(false);
    }
    let new = if old.is_empty() {
        ACTIVATE_LINE.to_string()
    } else {
        format!("{ACTIVATE_LINE}\n{old}")
    };
    o.insert("shellCommandPrefix".into(), Value::from(new));
    Ok(true)
}

/// What [`seed`] did per key.
#[derive(Debug, Default, PartialEq)]
pub struct Seeded {
    /// Keys set to the source value.
    pub set: Vec<String>,
    /// Keys whose local value differs and stays (source unchanged since
    /// vega last set it).
    pub local: Vec<String>,
}

/// `claude.settings`: sets each key of `src` that is missing in `root` or
/// whose source value differs from `last` (the value vega last set).
pub fn seed(
    root: &mut Value,
    src: &Map<String, Value>,
    last: &Map<String, Value>,
) -> Result<Seeded, String> {
    let o = root.as_object_mut().ok_or("not a JSON object")?;
    let mut out = Seeded::default();
    for (k, v) in src {
        let live = o.get(k);
        if live == Some(v) {
            continue;
        }
        if live.is_none() || last.get(k) != Some(v) {
            o.insert(k.clone(), v.clone());
            out.set.push(k.clone());
        } else {
            out.local.push(k.clone());
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upsert_keeps_others_and_is_idempotent() {
        let other = json!({"matcher": "Bash", "hooks": [{"type": "command", "command": "dcg"}]});
        let mut root = json!({"model": "x", "hooks": {"PreToolUse": [other.clone()], "Stop": []}});
        assert!(claude_hooks(&mut root, "/a/vega-mux").unwrap());
        assert!(!claude_hooks(&mut root, "/a/vega-mux").unwrap());
        assert!(claude_hooks(&mut root, "/b/vega-mux").unwrap());
        let pre = root["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!(pre.len(), 2);
        assert_eq!(pre[0], other);
        assert_eq!(pre[1]["matcher"], CLAUDE_TOOL);
        assert_eq!(
            pre[1]["hooks"][0]["command"],
            "'/b/vega-mux' hook ask-parent"
        );
        assert_eq!(root["hooks"]["SessionStart"].as_array().unwrap().len(), 1);
        assert!(root["hooks"]["Stop"].is_array());
        let keys: Vec<&String> = root.as_object().unwrap().keys().collect();
        assert_eq!(keys, ["model", "hooks"]);
    }

    #[test]
    fn install_cli_activation_entry_is_recognised() {
        // The entry scripts/install-cli.sh wrote (jq) is the same JSON.
        let mut root = json!({"hooks": {"SessionStart": [{"hooks": [{"type": "command",
            "command": "echo '. ~/.local/share/vega/shims/activate.sh' >> \"$CLAUDE_ENV_FILE\""}]}]}});
        assert!(!upsert_hook(&mut root, "SessionStart", ACTIVATE_MARK, activate_entry()).unwrap());
    }

    #[test]
    fn codex_creates_sections_and_rejects_non_objects() {
        let mut root = json!({});
        assert!(codex_hooks(&mut root, "x").unwrap());
        assert_eq!(root["hooks"]["PreToolUse"][0]["matcher"], CODEX_TOOL);
        assert!(codex_hooks(&mut json!([]), "x").is_err());
        assert!(codex_hooks(&mut json!({"hooks": []}), "x").is_err());
    }

    #[test]
    fn pi_prefix_goes_first_once() {
        let mut root = json!({"shellCommandPrefix": "set -e"});
        assert!(pi_prefix(&mut root).unwrap());
        assert_eq!(
            root["shellCommandPrefix"],
            format!("{ACTIVATE_LINE}\nset -e")
        );
        assert!(!pi_prefix(&mut root).unwrap());
        let mut root = json!({});
        assert!(pi_prefix(&mut root).unwrap());
        assert_eq!(root["shellCommandPrefix"], ACTIVATE_LINE);
    }

    #[test]
    fn seed_sets_missing_and_changed_keeps_local_edits() {
        let src = json!({"outputStyle": "A", "cleanupPeriodDays": 10});
        let src = src.as_object().unwrap();
        // First run: no record, set what differs.
        let mut root = json!({"outputStyle": "B", "theme": "dark"});
        let s = seed(&mut root, src, &Map::new()).unwrap();
        assert_eq!(s.set, ["outputStyle", "cleanupPeriodDays"]);
        assert_eq!(root["outputStyle"], "A");
        // The user switches the style: it survives while the source stays.
        root["outputStyle"] = json!("B");
        let s = seed(&mut root, src, src).unwrap();
        assert_eq!(
            s,
            Seeded {
                set: vec![],
                local: vec!["outputStyle".into()]
            }
        );
        assert_eq!(root["outputStyle"], "B");
        // The source changes: vega asserts the new value.
        let new = json!({"outputStyle": "C", "cleanupPeriodDays": 10});
        let s = seed(&mut root, new.as_object().unwrap(), src).unwrap();
        assert_eq!(s.set, ["outputStyle"]);
        assert_eq!(root["outputStyle"], "C");
        assert_eq!(root["theme"], "dark");
    }
}
