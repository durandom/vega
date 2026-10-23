//! Facts read at view time, never cached: which herdr pane works in a
//! checkout, and which tool installs come from one.

use std::env;
use std::fs;
use std::path::PathBuf;

use crate::scan::Entry;

/// (cwd, pane id) of every local herdr pane; empty without herdr.
pub fn panes() -> Vec<(String, String)> {
    let Ok(v) = vega_herdr::herdr_local(&["pane", "list"]) else {
        return Vec::new();
    };
    v["result"]["panes"]
        .as_array()
        .map(|ps| {
            ps.iter()
                .filter_map(|p| {
                    let cwd = p["foreground_cwd"].as_str().or(p["cwd"].as_str())?;
                    Some((cwd.to_string(), p["pane_id"].as_str()?.to_string()))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// (install source path, label like `uv castra` or `cargo vega`).
pub fn installs() -> Vec<(String, String)> {
    let home = env::var("HOME").unwrap_or_default();
    let mut out = Vec::new();
    let uv_tools = env::var("UV_TOOL_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(&home).join(".local/share/uv/tools"));
    if let Ok(rd) = fs::read_dir(&uv_tools) {
        for e in rd.flatten() {
            let Ok(text) = fs::read_to_string(e.path().join("uv-receipt.toml")) else {
                continue;
            };
            let tool = e.file_name().to_string_lossy().into_owned();
            for path in uv_receipt_paths(&text) {
                out.push((path, format!("uv {tool}")));
            }
        }
    }
    let mut crates = vec![
        PathBuf::from(&home).join(".cargo/.crates2.json"),
        PathBuf::from(&home).join(".local/.crates2.json"),
    ];
    if let Ok(c) = env::var("CARGO_HOME") {
        crates.push(PathBuf::from(c).join(".crates2.json"));
    }
    crates.dedup();
    for f in crates {
        if let Ok(text) = fs::read_to_string(&f) {
            out.extend(cargo_paths(&text));
        }
    }
    out.sort();
    out.dedup();
    out
}

/// `editable = "/p"` and `directory = "/p"` sources in a uv tool receipt.
fn uv_receipt_paths(receipt: &str) -> Vec<String> {
    ["editable = \"", "directory = \""]
        .iter()
        .flat_map(|key| {
            receipt.match_indices(key).filter_map(|(i, _)| {
                let rest = &receipt[i + key.len()..];
                rest.split_once('"').map(|(p, _)| p.to_string())
            })
        })
        .collect()
}

/// `name ver (path+file:///p)` keys of a cargo `.crates2.json`.
fn cargo_paths(json: &str) -> Vec<(String, String)> {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(json) else {
        return Vec::new();
    };
    v["installs"]
        .as_object()
        .map(|m| {
            m.keys()
                .filter_map(|k| {
                    let (name, rest) = k.split_once(' ')?;
                    let path = rest.split_once("(path+file://")?.1.strip_suffix(')')?;
                    Some((path.to_string(), format!("cargo {name}")))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The entry whose path holds `p`, deepest first (nested clones win).
pub fn owner<'a>(entries: &'a [Entry], p: &str) -> Option<&'a Entry> {
    entries
        .iter()
        .filter(|e| !e.prunable && (p == e.path || p.starts_with(&format!("{}/", e.path))))
        .max_by_key(|e| e.path.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_receipts() {
        let uv = "[tool]\nrequirements = [{ name = \"castra\", editable = \"/src/castra\" }]\n";
        assert_eq!(uv_receipt_paths(uv), vec!["/src/castra"]);
        let cargo = r#"{"installs":{"vega 0.1.0 (path+file:///src/vega/vega)":{},"rg 14 (registry+https://x)":{}}}"#;
        assert_eq!(
            cargo_paths(cargo),
            vec![("/src/vega/vega".to_string(), "cargo vega".to_string())]
        );
    }
}
