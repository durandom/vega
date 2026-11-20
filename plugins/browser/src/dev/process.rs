use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use vega_herdr::Failure;

use super::{failure, stale, wire::Cdp, Browser};

/// Fixed flags of the managed dev Chrome for Testing.
const LAUNCH_ARGS: &[&str] = &[
    "--remote-debugging-port=0",
    "--remote-debugging-address=127.0.0.1",
    "--no-first-run",
    "--no-default-browser-check",
    // Never touch the macOS login keychain: without this CfT asks for a
    // "Chromium Safe Storage" keychain and offers to reset the user's keychain.
    "--use-mock-keychain",
];

fn ps(pid: u32, field: &str) -> Result<Option<String>, Failure> {
    let result = Command::new("ps")
        .args(["-ww", "-p", &pid.to_string(), "-o", field])
        .env("LC_ALL", "C")
        .output()
        .map_err(failure)?;
    if result.status.success() {
        return Ok(Some(
            String::from_utf8_lossy(&result.stdout).trim().to_owned(),
        ));
    }
    if result.status.code() == Some(1) && result.stderr.is_empty() {
        return Ok(None);
    }
    Err(failure("Could not verify Chrome's process identity"))
}

fn endpoint(profile: &Path) -> Result<(u16, String), Failure> {
    let raw = fs::read_to_string(profile.join("DevToolsActivePort")).map_err(failure)?;
    let mut lines = raw.lines();
    let port = lines
        .next()
        .and_then(|s| s.parse::<u16>().ok())
        .filter(|p| *p > 0)
        .ok_or_else(|| failure("Invalid CfT debug port"))?;
    let path = lines
        .next()
        .and_then(|s| s.strip_prefix("/devtools/browser/").map(|id| (id, s)))
        .filter(|(id, _)| !id.is_empty() && id.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-'))
        .map(|(_, path)| path.to_owned())
        .ok_or_else(|| failure("Invalid CfT browser generation"))?;
    Ok((port, path))
}

pub(super) fn running(browser: &Browser, profile: &Path) -> Result<bool, Failure> {
    let Some(started) = ps(browser.pid, "lstart=")? else {
        return Ok(false);
    };
    if started != browser.started {
        return Ok(false);
    }
    let argv = ps(browser.pid, "command=")?.unwrap_or_default();
    let expected = format!(
        "{} --user-data-dir={} --remote-debugging-port=0 --remote-debugging-address=127.0.0.1 ",
        browser.executable.display(),
        profile.display()
    );
    if browser.profile != profile
        || !argv.starts_with(&expected)
        || endpoint(profile)? != (browser.port, browser.path.clone())
    {
        return Err(stale("Could not revalidate the managed CfT generation"));
    }
    Ok(true)
}

pub(super) fn launch(profile: &Path) -> Result<Browser, Failure> {
    let exe = crate::install::chrome_binary(&vega_herdr::platform::home().unwrap_or_default())
        .ok_or_else(|| {
            Failure::new(
                "missing_cft",
                "Chrome for Testing is missing",
                "vega browser install",
            )
        })?
        .canonicalize()
        .map_err(failure)?;
    let profile_arg = format!("--user-data-dir={}", profile.display());
    let processes = Command::new("ps")
        .args(["-ww", "-eo", "command="])
        .output()
        .map_err(failure)?;
    if !processes.status.success() {
        return Err(failure("Could not inspect the CfT profile owner"));
    }
    if String::from_utf8_lossy(&processes.stdout)
        .lines()
        .any(|line| {
            line.match_indices(&profile_arg).any(|(offset, arg)| {
                line.as_bytes().get(offset.wrapping_sub(1)) == Some(&b' ')
                    && matches!(line.as_bytes().get(offset + arg.len()), None | Some(b' '))
            })
        })
        || profile.join("SingletonLock").symlink_metadata().is_ok()
    {
        return Err(failure(
            "The CfT profile is occupied by an unverified process",
        ));
    }
    // A leftover DevToolsActivePort is never used to discover ownership.
    match fs::remove_file(profile.join("DevToolsActivePort")) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(failure(e)),
    }
    let extension = super::groups::package(profile)?;
    let mut child = Command::new("nohup")
        .arg(&exe)
        .arg(&profile_arg)
        .args(LAUNCH_ARGS)
        .arg(format!("--load-extension={}", extension.display()))
        .arg("about:blank")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(failure)?;
    let result = (|| {
        let deadline = Instant::now() + Duration::from_secs(15);
        while Instant::now() < deadline {
            if child.try_wait().map_err(failure)?.is_some() {
                return Err(failure("Chrome exited before publishing its dev endpoint"));
            }
            if let Ok((port, path)) = endpoint(profile) {
                let browser = Browser {
                    pid: child.id(),
                    started: ps(child.id(), "lstart=")?.ok_or_else(|| failure("Chrome exited"))?,
                    executable: exe,
                    profile: profile.to_owned(),
                    port,
                    path,
                };
                if running(&browser, profile)? {
                    Cdp::connect(&browser)?;
                    return Ok(browser);
                }
                return Err(failure("Chrome exited during startup"));
            }
            thread::sleep(Duration::from_millis(50));
        }
        Err(failure("Chrome did not publish its dev endpoint"))
    })();
    if result.is_err() {
        let _ = child.kill();
        let _ = child.wait();
    }
    result
}

#[cfg(test)]
mod launch_args_tests {
    #[test]
    fn dev_cft_never_uses_the_login_keychain() {
        assert!(super::LAUNCH_ARGS.contains(&"--use-mock-keychain"));
    }
}
