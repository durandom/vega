//! Host primitives shared by the native clients. XDG overrides win on every OS.
//! Windows defaults stay inside USERPROFILE/AppData: private files inherit that
//! account's directory ACL. Unix mode bits are deliberately not emulated with
//! readonly flags (which provide no confidentiality). A custom XDG directory
//! must have an appropriate ACL supplied by its owner.
use std::{
    env,
    fs::File,
    io,
    path::{Path, PathBuf},
    process::{Child, Command},
};

pub fn home() -> Option<PathBuf> {
    #[cfg(unix)]
    let value = env::var_os("HOME");
    #[cfg(windows)]
    let value = env::var_os("HOME")
        .filter(|v| !v.is_empty())
        .or_else(|| env::var_os("USERPROFILE").filter(|v| !v.is_empty()));
    value.map(PathBuf::from)
}

pub fn config_home() -> Option<PathBuf> {
    base("XDG_CONFIG_HOME", ".config", "APPDATA")
}
pub fn state_home() -> Option<PathBuf> {
    base("XDG_STATE_HOME", ".local/state", "LOCALAPPDATA")
}
pub fn cache_home() -> Option<PathBuf> {
    let root = base("XDG_CACHE_HOME", ".cache", "LOCALAPPDATA")?;
    #[cfg(windows)]
    if env::var_os("XDG_CACHE_HOME").is_none_or(|v| v.is_empty()) {
        return Some(root.join("cache"));
    }
    Some(root)
}

fn base(xdg: &str, unix: &str, windows: &str) -> Option<PathBuf> {
    if let Some(p) = env::var_os(xdg).filter(|v| !v.is_empty()) {
        return Some(p.into());
    }
    #[cfg(unix)]
    {
        let _ = windows;
        home().map(|p| p.join(unix))
    }
    #[cfg(windows)]
    {
        let _ = unix;
        env::var_os(windows)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .or_else(|| {
                home().map(|p| {
                    p.join(if windows == "APPDATA" {
                        "AppData/Roaming"
                    } else {
                        "AppData/Local"
                    })
                })
            })
    }
}

/// Replace a sibling temporary file without a remove/create gap.
pub fn replace(from: impl AsRef<Path>, to: impl AsRef<Path>) -> io::Result<()> {
    #[cfg(unix)]
    {
        std::fs::rename(from, to)
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Storage::FileSystem::{
            MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
        };
        let from: Vec<_> = from
            .as_ref()
            .as_os_str()
            .encode_wide()
            .chain(Some(0))
            .collect();
        let to: Vec<_> = to
            .as_ref()
            .as_os_str()
            .encode_wide()
            .chain(Some(0))
            .collect();
        if from[..from.len() - 1].contains(&0) || to[..to.len() - 1].contains(&0) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "path contains NUL",
            ));
        }
        // SAFETY: both paths are NUL-terminated buffers alive for the call.
        if unsafe {
            MoveFileExW(
                from.as_ptr(),
                to.as_ptr(),
                MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
            )
        } == 0
        {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }
}

/// Lock the first byte until this handle closes. Unix retains flock semantics.
pub fn lock(file: &File) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::fd::AsRawFd;
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Storage::FileSystem::{LockFileEx, LOCKFILE_EXCLUSIVE_LOCK};
        // Synchronous exclusive lock; the file handle owns its lifetime.
        let mut overlapped = unsafe { std::mem::zeroed() };
        if unsafe {
            LockFileEx(
                file.as_raw_handle(),
                LOCKFILE_EXCLUSIVE_LOCK,
                0,
                1,
                0,
                &mut overlapped,
            )
        } != 0
        {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }
}

/// Apply Unix private mode; Windows inherits the directory ACL.
pub fn private_options(options: &mut std::fs::OpenOptions) -> &mut std::fs::OpenOptions {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
}

pub fn private_dir(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
    }
    #[cfg(windows)]
    {
        let _ = path;
        Ok(())
    }
}

/// Nonblocking lock for the bounded history outbox retry loop.
pub fn try_lock(file: &File) -> io::Result<bool> {
    #[cfg(unix)]
    {
        use std::os::fd::AsRawFd;
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
            return Ok(true);
        }
        let err = io::Error::last_os_error();
        if err.kind() == io::ErrorKind::WouldBlock {
            Ok(false)
        } else {
            Err(err)
        }
    }
    #[cfg(windows)]
    {
        match file.try_lock() {
            Ok(()) => Ok(true),
            Err(std::fs::TryLockError::WouldBlock) => Ok(false),
            Err(std::fs::TryLockError::Error(e)) => Err(e),
        }
    }
}

/// Windows cannot fsync a directory through File::open. Writers sync file data
/// and replace with WRITE_THROUGH instead; Unix retains its directory fsync.
pub fn sync_dir(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        File::open(path)?.sync_all()
    }
    #[cfg(windows)]
    {
        let _ = path;
        Ok(())
    }
}

pub(crate) fn isolate(cmd: &mut Command) {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(windows_sys::Win32::System::Threading::CREATE_SUSPENDED);
    }
}
pub(crate) fn kill_tree(child: &mut Child) {
    #[cfg(unix)]
    unsafe {
        libc::kill(-(child.id() as i32), libc::SIGKILL);
    }
    #[cfg(windows)]
    {
        let _ = child.kill();
    } // The ProcessTree job closes on return, killing descendants.
}

#[cfg(unix)]
pub(crate) struct ProcessTree;
#[cfg(unix)]
impl ProcessTree {
    pub fn attach(_: &mut Child) -> io::Result<Self> {
        Ok(Self)
    }
}

#[cfg(windows)]
pub(crate) use windows::ProcessTree;
#[cfg(windows)]
pub use windows::{parent_of, processes, session_id};

#[cfg(windows)]
mod windows {
    use super::*;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::{
        Foundation::*,
        System::{Diagnostics::ToolHelp::*, JobObjects::*, Threading::*},
    };

    struct Handle(HANDLE);
    impl Drop for Handle {
        fn drop(&mut self) {
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
    pub(crate) struct ProcessTree {
        _job: Handle,
    }
    impl ProcessTree {
        pub fn attach(child: &mut Child) -> io::Result<Self> {
            let result = (|| {
                let job = Handle(unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) });
                if job.0.is_null() {
                    return Err(io::Error::last_os_error());
                }
                let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
                info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                if unsafe {
                    SetInformationJobObject(
                        job.0,
                        JobObjectExtendedLimitInformation,
                        &info as *const _ as _,
                        std::mem::size_of_val(&info) as u32,
                    )
                } == 0
                    || unsafe { AssignProcessToJobObject(job.0, child.as_raw_handle()) } == 0
                {
                    return Err(io::Error::last_os_error());
                }
                // Spawn suspended, assign the job before any user code can fork,
                // then resume its primary thread. No descendant can escape the job.
                let snapshot = Handle(unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) });
                if snapshot.0 == INVALID_HANDLE_VALUE {
                    return Err(io::Error::last_os_error());
                }
                let mut entry: THREADENTRY32 = unsafe { std::mem::zeroed() };
                entry.dwSize = std::mem::size_of_val(&entry) as u32;
                if unsafe { Thread32First(snapshot.0, &mut entry) } == 0 {
                    return Err(io::Error::last_os_error());
                }
                loop {
                    if entry.th32OwnerProcessID == child.id() {
                        let thread = Handle(unsafe {
                            OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID)
                        });
                        if thread.0.is_null() || unsafe { ResumeThread(thread.0) } == u32::MAX {
                            return Err(io::Error::last_os_error());
                        }
                        break;
                    }
                    if unsafe { Thread32Next(snapshot.0, &mut entry) } == 0 {
                        return Err(io::Error::last_os_error());
                    }
                }
                Ok(Self { _job: job })
            })();
            if result.is_err() {
                let _ = child.kill();
                let _ = child.wait();
            }
            result
        }
    }

    /// Windows login session of a known process (0 is the Services session).
    pub fn session_id(pid: u32) -> io::Result<u32> {
        use windows_sys::Win32::System::RemoteDesktop::ProcessIdToSessionId;
        let mut session = 0;
        // SAFETY: session is a valid writable DWORD for the duration of the call.
        if unsafe { ProcessIdToSessionId(pid, &mut session) } == 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(session)
        }
    }

    /// ToolHelp provides PIDs and executable names without shell/locale parsing.
    pub fn processes() -> io::Result<Vec<(u32, u32, String)>> {
        let snapshot = Handle(unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) });
        if snapshot.0 == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        let mut entry: PROCESSENTRY32W = unsafe { std::mem::zeroed() };
        entry.dwSize = std::mem::size_of_val(&entry) as u32;
        let mut rows = Vec::new();
        if unsafe { Process32FirstW(snapshot.0, &mut entry) } == 0 {
            return Err(io::Error::last_os_error());
        }
        loop {
            let len = entry
                .szExeFile
                .iter()
                .position(|c| *c == 0)
                .unwrap_or(entry.szExeFile.len());
            rows.push((
                entry.th32ProcessID,
                entry.th32ParentProcessID,
                String::from_utf16_lossy(&entry.szExeFile[..len]),
            ));
            if unsafe { Process32NextW(snapshot.0, &mut entry) } == 0 {
                break;
            }
        }
        Ok(rows)
    }
    fn created(pid: u32) -> Option<u64> {
        let handle = Handle(unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) });
        if handle.0.is_null() {
            return None;
        }
        let mut times = [FILETIME {
            dwLowDateTime: 0,
            dwHighDateTime: 0,
        }; 4];
        if unsafe {
            GetProcessTimes(
                handle.0,
                &mut times[0],
                &mut times[1],
                &mut times[2],
                &mut times[3],
            )
        } == 0
        {
            return None;
        }
        Some((u64::from(times[0].dwHighDateTime) << 32) | u64::from(times[0].dwLowDateTime))
    }
    /// Reject an exited/reused parent PID: a real parent predates its child.
    pub fn parent_of(pid: u32) -> Option<u32> {
        let child_start = created(pid)?;
        let parent = processes().ok()?.into_iter().find(|(p, _, _)| *p == pid)?.1;
        (created(parent)? <= child_start).then_some(parent)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn replaces_existing_file_without_leaving_temp() {
        let d = tempfile::tempdir().unwrap();
        let a = d.path().join("a");
        let b = d.path().join("b");
        std::fs::write(&a, "new").unwrap();
        std::fs::write(&b, "old").unwrap();
        replace(&a, &b).unwrap();
        assert_eq!(std::fs::read_to_string(b).unwrap(), "new");
        assert!(!a.exists());
    }
    #[test]
    fn user_roots_respect_overrides_and_platform_defaults() {
        const CHILD: &str = "VEGA_PLATFORM_ROOTS_TEST";
        if env::var_os(CHILD).is_none() {
            let root = tempfile::tempdir().unwrap();
            let mut cmd = Command::new(env::current_exe().unwrap());
            cmd.args([
                "--exact",
                "platform::tests::user_roots_respect_overrides_and_platform_defaults",
            ])
            .env(CHILD, root.path())
            .env("HOME", root.path())
            .env("USERPROFILE", root.path())
            .env("APPDATA", root.path().join("roaming"))
            .env("LOCALAPPDATA", root.path().join("local"));
            for key in ["XDG_CONFIG_HOME", "XDG_STATE_HOME", "XDG_CACHE_HOME"] {
                cmd.env_remove(key);
            }
            let out = cmd.output().unwrap();
            assert!(
                out.status.success(),
                "{}{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
            return;
        }
        let root = PathBuf::from(env::var_os(CHILD).unwrap());
        assert_eq!(home(), Some(root.clone()));
        #[cfg(unix)]
        {
            assert_eq!(config_home(), Some(root.join(".config")));
            assert_eq!(state_home(), Some(root.join(".local/state")));
            assert_eq!(cache_home(), Some(root.join(".cache")));
        }
        #[cfg(windows)]
        {
            env::remove_var("HOME");
            assert_eq!(home(), Some(root.clone()));
            env::set_var("HOME", root.join("home-override"));
            assert_eq!(home(), Some(root.join("home-override")));
            env::set_var("HOME", &root);
            assert_eq!(config_home(), Some(root.join("roaming")));
            assert_eq!(state_home(), Some(root.join("local")));
            assert_eq!(cache_home(), Some(root.join("local/cache")));
            env::remove_var("APPDATA");
            env::remove_var("LOCALAPPDATA");
            assert_eq!(config_home(), Some(root.join("AppData/Roaming")));
            assert_eq!(state_home(), Some(root.join("AppData/Local")));
        }
        for key in ["XDG_CONFIG_HOME", "XDG_STATE_HOME", "XDG_CACHE_HOME"] {
            env::set_var(key, root.join("override"));
        }
        assert_eq!(config_home(), Some(root.join("override")));
        assert_eq!(state_home(), Some(root.join("override")));
        assert_eq!(cache_home(), Some(root.join("override")));
    }

    #[cfg(windows)]
    #[test]
    fn deadline_kills_owned_descendants_and_drains_pipes() {
        use windows_sys::Win32::{Foundation::*, System::Threading::*};
        const MODE: &str = "VEGA_PROCESS_TREE_TEST";
        if let Ok(mode) = env::var(MODE) {
            if mode == "child" {
                Command::new(env::current_exe().unwrap())
                    .args([
                        "--exact",
                        "platform::tests::deadline_kills_owned_descendants_and_drains_pipes",
                    ])
                    .env(MODE, "grandchild")
                    .spawn()
                    .unwrap()
                    .wait()
                    .unwrap();
            } else {
                std::fs::write(
                    env::var_os("VEGA_PROCESS_TREE_PID").unwrap(),
                    std::process::id().to_string(),
                )
                .unwrap();
            }
            std::thread::sleep(std::time::Duration::from_secs(30));
            return;
        }
        let root = tempfile::tempdir().unwrap();
        let pidfile = root.path().join("pid");
        let mut cmd = Command::new(env::current_exe().unwrap());
        cmd.args([
            "--exact",
            "platform::tests::deadline_kills_owned_descendants_and_drains_pipes",
        ])
        .env(MODE, "child")
        .env("VEGA_PROCESS_TREE_PID", &pidfile);
        let start = std::time::Instant::now();
        assert!(
            crate::output_within(&mut cmd, std::time::Duration::from_secs(3))
                .unwrap()
                .is_none()
        );
        assert!(start.elapsed() < std::time::Duration::from_secs(6));
        let pid: u32 = std::fs::read_to_string(pidfile).unwrap().parse().unwrap();
        let process = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
        if !process.is_null() {
            assert_eq!(unsafe { WaitForSingleObject(process, 2000) }, WAIT_OBJECT_0);
            unsafe {
                CloseHandle(process);
            }
        }
        let out = crate::output_within(
            Command::new("cmd.exe").args(["/d", "/c", "echo ok"]),
            std::time::Duration::from_secs(3),
        )
        .unwrap()
        .unwrap();
        assert!(out.status.success());
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "ok");
    }

    #[cfg(windows)]
    #[test]
    fn native_parent_matches_spawned_child() {
        let mut cmd = Command::new("cmd.exe");
        cmd.args(["/d", "/c", "ping -n 10 127.0.0.1 >nul"]);
        isolate(&mut cmd);
        let mut child = cmd.spawn().unwrap();
        let tree = ProcessTree::attach(&mut child).unwrap();
        assert_eq!(parent_of(child.id()), Some(std::process::id()));
        drop(tree);
        child.wait().unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn native_rpc_keeps_stdin_open_until_unicode_reply() {
        let mut cmd = Command::new("powershell.exe");
        cmd.args(["-NoProfile", "-NonInteractive", "-Command",
            "[Console]::InputEncoding = [System.Text.UTF8Encoding]::new($false); [Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false); $line = [Console]::ReadLine(); [Console]::WriteLine($line); Start-Sleep 30"]);
        let input = "{\"request\":\"Grüße\"}\n".as_bytes().to_vec();
        let reply = crate::rpc_line_within(
            &mut cmd,
            input.clone(),
            Some(std::time::Duration::from_secs(10)),
        )
        .unwrap()
        .unwrap();
        // Console.WriteLine uses CRLF; JSON accepts either line ending.
        assert_eq!(
            String::from_utf8(reply.line.unwrap()).unwrap().trim(),
            String::from_utf8(input).unwrap().trim()
        );
    }
}
