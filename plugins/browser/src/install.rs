//! Pinned agent-browser installation, respecting the caller's tool manager.
use crate::{chrome, real};
use serde_json::{json, Value};
use std::{
    env, fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};
use vega_herdr::Failure;

pub const VERSION: &str = include_str!("../agent-browser.version");

fn failure(message: impl Into<String>) -> Failure {
    Failure::new("browser_install", message, "use a login shell with node/npm on PATH; resolve conflicting installs or VEGA_AGENT_BROWSER_BIN, then vega browser install")
}

fn output(cmd: &mut Command) -> Result<String, Failure> {
    let out = cmd
        .stdin(Stdio::null())
        .output()
        .map_err(|e| failure(e.to_string()))?;
    if !out.status.success() {
        return Err(failure(format!(
            "command failed ({}): {}",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

fn version(path: &Path) -> Result<String, Failure> {
    let text = output(Command::new(path).arg("--version"))?;
    text.strip_prefix("agent-browser ")
        .filter(|v| !v.contains(char::is_whitespace) && !v.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| failure(format!("unexpected agent-browser version: {text}")))
}

fn mise_source(bin: &Path) -> Option<String> {
    let mut resolved = bin.canonicalize().ok()?;
    if bin.parent()?.ends_with("shims") && resolved.ends_with("mise") {
        // The shim dispatches by name. Ask mise which binary it dispatches to.
        let target = output(Command::new("mise").args(["which", real::TOOL])).ok()?;
        resolved = Path::new(&target).canonicalize().ok()?;
    }
    let tools =
        output(Command::new("mise").args(["ls", "--current", "--json", "npm:agent-browser"]))
            .ok()?;
    let tools: Vec<Value> = serde_json::from_str(&tools).ok()?;
    tools.iter().find_map(|tool| {
        let root = Path::new(tool["install_path"].as_str()?)
            .canonicalize()
            .ok()?;
        if tool["active"] == true && resolved.starts_with(root) {
            tool["source"]["path"].as_str().map(str::to_owned)
        } else {
            None
        }
    })
}

/// Matches upstream v0.43.0 cli/src/install.rs, without starting a browser.
pub(crate) fn chrome_binary(home: &Path) -> Option<PathBuf> {
    let suffixes: &[&str] = if cfg!(target_os = "macos") {
        &["Google Chrome for Testing.app/Contents/MacOS/Google Chrome for Testing", "chrome-mac-arm64/Google Chrome for Testing.app/Contents/MacOS/Google Chrome for Testing", "chrome-mac-x64/Google Chrome for Testing.app/Contents/MacOS/Google Chrome for Testing"]
    } else {
        &["chrome", "chrome-linux64/chrome"]
    };
    fs::read_dir(home.join(".agent-browser/browsers"))
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with("chrome-"))
        .flat_map(|e| suffixes.iter().map(move |s| e.path().join(s)))
        .filter(|p| p.is_file())
        .max_by_key(|p| {
            p.strip_prefix(home.join(".agent-browser/browsers"))
                .ok()
                .and_then(|p| p.components().next())
                .and_then(|s| s.as_os_str().to_str())
                .and_then(|s| s.strip_prefix("chrome-"))
                .map(|s| {
                    s.split('.')
                        .filter_map(|n| n.parse::<u32>().ok())
                        .collect::<Vec<_>>()
                })
        })
}

/// Whether `npm` is on PATH: a host without agent-browser and without npm
/// does not use browsers (e.g. io), so the config unit skips it.
pub fn npm_on_path() -> bool {
    env::var_os("PATH").is_some_and(|p| env::split_paths(&p).any(|d| d.join("npm").is_file()))
}

pub fn status() -> Result<Value, Failure> {
    let bin = real::real_binary();
    let found = bin.as_deref().map(version).transpose()?;
    let state = match found.as_deref() {
        None => "missing",
        Some(v) if v == VERSION.trim() => "current",
        _ => "differs",
    };
    let home = env::var_os("HOME").ok_or_else(|| failure("HOME is missing"))?;
    Ok(
        json!({"state": state, "version": found, "pinned": VERSION.trim(), "binary": bin,
        "chrome": if chrome::installed(Path::new(&home)) { "current" } else { "missing" },
        "npm": npm_on_path()}),
    )
}

pub fn print_status(s: &Value, json: bool) {
    if json {
        println!("{s}");
    } else {
        println!(
            "agent-browser: {} (installed {}, pinned {})",
            s["state"].as_str().unwrap_or("unknown"),
            s["version"].as_str().unwrap_or("none"),
            VERSION.trim()
        );
        println!(
            "chrome-for-testing: {}",
            s["chrome"].as_str().unwrap_or("unknown")
        );
    }
}

pub fn install() -> Result<(), Failure> {
    let before = status()?;
    print_status(&before, false);
    if before["state"] != "current" {
        if let Some(source) = before["binary"]
            .as_str()
            .map(Path::new)
            .and_then(mise_source)
        {
            let source = source.replace('\'', "'\\''");
            return Err(Failure::new(
                "browser_install",
                "mise-managed agent-browser differs from the pinned version",
                &format!(
                    "mise use --path '{source}' --pin npm:agent-browser@{}",
                    VERSION.trim()
                ),
            ));
        }
        let root = PathBuf::from(output(Command::new("npm").args(["root", "--global"]))?);
        let prefix = PathBuf::from(output(Command::new("npm").args(["prefix", "--global"]))?);
        if !prefix.is_absolute() || root != prefix.join("lib/node_modules") {
            return Err(failure("unsupported npm global layout"));
        }
        let package = root.join("agent-browser");
        let target = prefix.join("bin/agent-browser");
        if let Some(real) = real::real_binary() {
            let canonical = real.canonicalize().map_err(|e| failure(e.to_string()))?;
            let package = package.canonicalize().map_err(|_| {
                failure("existing agent-browser is outside the selected npm global prefix")
            })?;
            if !canonical.starts_with(package)
                || target.canonicalize().ok().as_ref() != Some(&canonical)
            {
                return Err(failure("existing agent-browser differs from npm's global binary; refusing a second install"));
            }
        } else if target.symlink_metadata().is_ok() {
            return Err(failure(
                "npm's global agent-browser exists but is not resolvable on PATH",
            ));
        }
        output(
            Command::new("npm")
                .args(["install", "--global", "--no-audit", "--no-fund", "--prefix"])
                .arg(&prefix)
                .arg(format!("agent-browser@{}", VERSION.trim())),
        )?;
        let resolved =
            real::real_binary().ok_or_else(|| failure("installed agent-browser is not on PATH"))?;
        if resolved.canonicalize().ok() != target.canonicalize().ok()
            || version(&resolved)? != VERSION.trim()
        {
            return Err(failure(
                "installed binary does not resolve to the pinned npm package",
            ));
        }
    }
    let home = env::var_os("HOME").ok_or_else(|| failure("HOME is missing"))?;
    chrome::install(Path::new(&home))?;
    let after = status()?;
    if after["state"] != "current" || after["chrome"] != "current" {
        return Err(failure(
            "installation did not converge (binary or Chrome for Testing missing)",
        ));
    }
    println!("result: current");
    Ok(())
}
