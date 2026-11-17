//! Pinned npm install, in the same global prefix the caller already uses.
use crate::real;
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

/// Matches upstream v0.43.0 cli/src/install.rs, without starting a browser.
pub fn chrome(home: &Path) -> bool {
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
        .any(|e| suffixes.iter().any(|s| e.path().join(s).is_file()))
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
        "chrome": if chrome(Path::new(&home)) { "current" } else { "missing" }}),
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
    if before["chrome"] == "missing" {
        let bin =
            real::real_binary().ok_or_else(|| failure("agent-browser missing after install"))?;
        output(Command::new(bin).arg("install"))?;
    }
    let after = status()?;
    if after["state"] != "current" || after["chrome"] != "current" {
        return Err(failure(
            "installation did not converge (binary or Chrome for Testing missing)",
        ));
    }
    println!("result: current");
    Ok(())
}
