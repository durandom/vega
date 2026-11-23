//! The versioned Chrome for Testing cache shared with agent-browser.
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};
use vega_herdr::Failure;

// Google's last-known-good Stable build on 2026-11-23.
pub const VERSION: &str = "158.0.8127.36";

const PLATFORM: &str = match (
    std::env::consts::OS.as_bytes(),
    std::env::consts::ARCH.as_bytes(),
) {
    (b"macos", b"aarch64") => "mac-arm64",
    (b"macos", b"x86_64") => "mac-x64",
    (b"linux", b"aarch64") => "linux-arm64",
    (b"linux", b"x86_64") => "linux64",
    _ => "",
};
const EXECUTABLE: &str = if cfg!(target_os = "macos") {
    "Google Chrome for Testing.app/Contents/MacOS/Google Chrome for Testing"
} else {
    "chrome"
};

fn directory(home: &Path) -> PathBuf {
    home.join(format!(".agent-browser/browsers/chrome-{VERSION}"))
}

fn executable(path: &Path) -> bool {
    path.metadata()
        .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

pub fn binary(home: &Path) -> PathBuf {
    let dest = directory(home);
    let direct = dest.join(EXECUTABLE);
    if executable(&direct) {
        direct
    } else {
        dest.join(format!("chrome-{PLATFORM}")).join(EXECUTABLE)
    }
}

pub fn installed(home: &Path) -> bool {
    executable(&binary(home))
}

fn failure(message: impl Into<String>) -> Failure {
    Failure::new(
        "chrome_install",
        message,
        "check curl, unzip and the Chrome for Testing cache, then rerun vega browser install",
    )
}

fn run(command: &mut Command) -> Result<(), Failure> {
    let output = command
        .stdin(Stdio::null())
        .output()
        .map_err(|e| failure(format!("{}: {e}", command.get_program().to_string_lossy())))?;
    if !output.status.success() {
        return Err(failure(format!(
            "{} failed ({}): {}",
            command.get_program().to_string_lossy(),
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(())
}

pub fn install(home: &Path) -> Result<(), Failure> {
    if installed(home) {
        return Ok(());
    }
    if PLATFORM.is_empty() {
        return Err(failure(
            "no pinned Chrome for Testing build for this platform",
        ));
    }
    let dest = directory(home);
    let cache = dest.parent().expect("cache directory");
    fs::create_dir_all(cache).map_err(|e| failure(e.to_string()))?;
    let stage = tempfile::tempdir_in(cache).map_err(|e| failure(e.to_string()))?;
    let archive = stage.path().join("chrome.zip");
    let build = format!("chrome-{PLATFORM}");
    let url = format!(
        "https://storage.googleapis.com/chrome-for-testing-public/{VERSION}/{PLATFORM}/{build}.zip"
    );
    run(Command::new("curl")
        .args([
            "--disable",
            "--fail",
            "--silent",
            "--show-error",
            "--output",
        ])
        .arg(&archive)
        .arg(url))?;
    run(Command::new("unzip")
        .arg("-q")
        .arg(&archive)
        .arg("-d")
        .arg(stage.path()))?;
    let extracted = stage.path().join(build);
    if !executable(&extracted.join(EXECUTABLE)) {
        return Err(failure(
            "pinned archive does not contain an executable Chrome",
        ));
    }
    // Publish all of Chrome at once. A concurrent installer can win this rename.
    if let Err(e) = fs::rename(extracted, &dest) {
        if !installed(home) {
            return Err(failure(format!("{}: {e}", dest.display())));
        }
    }
    Ok(())
}
