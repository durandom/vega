//! Escape caller jobs and handle inheritance through the interactive task scheduler.
use super::*;
use std::{
    collections::BTreeMap,
    process::{Command, Stdio},
};

pub(super) fn start(ready: &std::path::Path) -> Result<(), Failure> {
    let id = nuid::next();
    let dir = root()?;
    let script = dir.join(format!("launch-{id}.ps1"));
    let launch = dir.join(format!("launch-{id}.json"));
    // Same inherited environment as a direct spawn, but never persist secrets in
    // Task Scheduler's task definition. The worker consumes this profile-local
    // file before readiness; the starter cleans up unsuccessful launches too.
    let environment = std::env::vars_os()
        .filter(|(k, _)| k != "HC_TERMINAL_ID" && k != "HERDR_PANE_ID")
        .map(|(k, v)| {
            (
                k.to_string_lossy().into_owned(),
                v.to_string_lossy().into_owned(),
            )
        })
        .collect::<BTreeMap<_, _>>();
    fs::write(&launch, serde_json::to_vec(&environment).map_err(fail)?).map_err(fail)?;
    let result = (|| {
        fs::write(&script, include_str!("../windows/relay-task.ps1")).map_err(fail)?;
        let mut command = Command::new("powershell.exe");
        command
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-ExecutionPolicy",
                "Bypass",
                "-File",
            ])
            .arg(&script)
            .arg("-Exe")
            .arg(std::env::current_exe().map_err(fail)?)
            .arg("-Ready")
            .arg(ready)
            .arg("-Launch")
            .arg(&launch)
            .arg("-TaskName")
            .arg(format!("VegaRelay-{id}"))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000); // CREATE_NO_WINDOW
        let output = command.output().map_err(fail)?;
        if !output.status.success() {
            return Err(fail(format!(
                "interactive user task launch failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }
        let status = fs::read_to_string(ready).map_err(fail)?;
        println!("relay: {status}");
        Ok(())
    })();
    for p in [&script, &launch, ready] {
        let _ = fs::remove_file(p);
    }
    result
}

pub(super) fn restore(path: &std::path::Path) -> Result<(), Failure> {
    let bytes = fs::read(path).map_err(fail)?;
    fs::remove_file(path).map_err(fail)?;
    let environment: BTreeMap<String, String> = serde_json::from_slice(&bytes).map_err(fail)?;
    // Called before constructing the runtime or any worker threads.
    for (key, value) in environment {
        std::env::set_var(key, value);
    }
    let log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(root()?.join("worker.log"))
        .map_err(fail)?;
    use std::os::windows::io::IntoRawHandle;
    use windows_sys::Win32::System::Console::{SetStdHandle, STD_ERROR_HANDLE};
    // The owned handle becomes the process stderr and remains open until exit.
    if unsafe { SetStdHandle(STD_ERROR_HANDLE, log.into_raw_handle()) } == 0 {
        return Err(fail(std::io::Error::last_os_error()));
    }
    Ok(())
}
