//! Minimal native hc worker. It never pulls inbox messages: only inbox owns acks.
use crate::{
    delivery::{self, Decision, Delivery, Draft},
    terminals,
};
use clap::Subcommand;
use serde_json::{json, Value};
use std::{
    collections::BTreeSet,
    fs::{self, File, OpenOptions},
    path::PathBuf,
    process::{Command, Stdio},
    time::Duration,
};
use vega_herdr::{platform, Failure};

#[derive(Subcommand)]
pub enum Action {
    /// Ensure one detached worker is running for this user
    Start,
    /// Run in the foreground (stop with Ctrl-C)
    Serve {
        #[arg(long, hide = true)]
        ready_file: Option<PathBuf>,
    },
}
fn fail(e: impl std::fmt::Display) -> Failure {
    Failure::new(
        "relay_failed",
        e.to_string(),
        "check vega post status; run vega post relay serve for diagnostics",
    )
}
fn root() -> Result<PathBuf, Failure> {
    let p = platform::state_home()
        .ok_or_else(|| fail("no state home"))?
        .join("vega/native-relay");
    fs::create_dir_all(&p).map_err(fail)?;
    Ok(p)
}
fn lock() -> Result<Option<File>, Failure> {
    let p = root()?.join("worker.lock");
    let f = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(p)
        .map_err(fail)?;
    Ok(platform::try_lock(&f).map_err(fail)?.then_some(f))
}
pub fn run(action: Action) -> Result<(), Failure> {
    match action {
        Action::Start => start(),
        Action::Serve { ready_file } => {
            let held = lock()?;
            if let Some(p) = ready_file {
                fs::write(
                    p,
                    if held.is_some() {
                        "started"
                    } else {
                        "already running"
                    },
                )
                .map_err(fail)?;
            }
            let Some(_held) = held else {
                println!("relay: already running");
                return Ok(());
            };
            vega_herdr::bound_daemon_calls();
            crate::runtime()?.block_on(serve())
        }
    }
}
fn start() -> Result<(), Failure> {
    // Probe without retaining the lock: serve arbitrates concurrent starters.
    let Some(probe) = lock()? else {
        println!("relay: already running");
        return Ok(());
    };
    drop(probe);
    let ready = root()?.join(format!("ready-{}", nuid::next()));
    let log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(root()?.join("worker.log"))
        .map_err(fail)?;
    let mut c = Command::new(std::env::current_exe().map_err(fail)?);
    c.args(["relay", "serve", "--ready-file"])
        .arg(&ready)
        .env_remove("HC_TERMINAL_ID")
        .env_remove("HERDR_PANE_ID")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(log);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP: no console/session reexec.
        c.creation_flags(0x00000008 | 0x00000200);
    }
    let mut child = {
        #[cfg(windows)]
        let _handles = DetachedHandles::new().map_err(fail)?;
        c.spawn().map_err(fail)?
    };
    let until = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        if let Ok(status) = fs::read_to_string(&ready) {
            let _ = fs::remove_file(&ready);
            println!("relay: {status}");
            if status == "started" {
                println!("pid: {}", child.id());
            }
            return Ok(());
        }
        if child.try_wait().map_err(fail)?.is_some() || std::time::Instant::now() >= until {
            let _ = child.kill();
            let _ = child.wait();
            let _ = fs::remove_file(&ready);
            return Err(fail(
                "worker did not acquire its lock; inspect native-relay/worker.log",
            ));
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}
async fn serve() -> Result<(), Failure> {
    let file = root()?.join("delivery.json");
    let mut d = Delivery::default();
    if let Ok(v) = fs::read(&file) {
        d.import(&serde_json::from_slice::<Value>(&v).map_err(fail)?);
    }
    loop {
        if let Err(e) = session(&mut d, &file).await {
            eprintln!("relay: {}: {}", e.code, e.message);
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}
async fn session(d: &mut Delivery, file: &std::path::Path) -> Result<(), Failure> {
    crate::client_identity::verify_local()?;
    let host = crate::host();
    let client = crate::connect("native relay", None).await?;
    let stream = crate::inbox_stream(&vega_registry::broker_context(client.clone()), &host).await?;
    let kv = vega_registry::bucket(&client).await.map_err(fail)?;
    // Read only our agent keys. Preserve identity and all other registry kinds.
    let registry = vega_registry::broker_context(client.clone())
        .get_stream(format!("KV_{}", vega_registry::BUCKET))
        .await
        .map_err(fail)?;
    let mut published: BTreeSet<String> = crate::subject_counts(
        &registry,
        &format!("$KV.{}.{host}.agent.>", vega_registry::BUCKET),
    )
    .await?
    .into_iter()
    .filter_map(|(s, _)| {
        s.strip_prefix(&format!("$KV.{}.", vega_registry::BUCKET))
            .map(str::to_owned)
    })
    .collect();
    loop {
        // Failed inventory is not empty: retain state and let the heartbeat age.
        let agents = terminals::hc_list()?;
        terminals::native_parents(&agents);
        d.snapshot_backend(
            agents
                .iter()
                .filter_map(|a| {
                    Some((
                        a["pane_id"].as_str()?.into(),
                        a["agent_status"].as_str()?.into(),
                    ))
                })
                .collect(),
            true,
        );
        let mut entries = std::collections::BTreeMap::new();
        for a in &agents {
            let pane = a["pane_id"].as_str().expect("normalized hc pane");
            entries.insert(vega_registry::agent_key(&host, pane), json!({
                "schema":2,"pane":pane,"backend":"hc","address":a["address"],"name":a["name"],
                "kind":a["agent"],"status":a["agent_status"],"turns":a["completion_seq"],"cwd":a["cwd"]
            }));
            let (count, floor) = crate::outage::bounded(
                "relay unread",
                crate::unread_and_floor(&stream, &host, pane),
            )
            .await?;
            if count == 0 {
                d.decide(pane, count, floor);
                continue;
            }
            // Polling is the arrival/reconnect event source. Touching a pointed
            // pane rechecks its ack floor without typing a second pointer.
            d.rescanned(pane);
            if !matches!(
                a["agent_status"].as_str(),
                Some("idle" | "done" | "working")
            ) {
                continue;
            }
            if !d.candidates(false).iter().any(|p| p == pane)
                || d.decide(pane, count, floor) != Decision::Type
            {
                continue;
            }
            // Fail closed for drafts, unknown screens, failed reads and status races.
            let screen = vega_herdr::hc::hc_local(&["read", "--lines", "0", pane]);
            if !matches!(
                screen
                    .ok()
                    .and_then(|v| v["text"].as_str().map(delivery::draft)),
                Some(Draft::None)
            ) {
                continue;
            }
            let what = if count == 1 {
                "1 new message".into()
            } else {
                format!("{count} new messages")
            };
            let line = format!("[vega-post] {what} for you. Run: {} inbox", crate::cmd());
            let typed = terminals::send_hc(pane, &line, d.may_queue(pane)).is_ok();
            d.typed(pane, floor, typed);
            save(d, file)?;
        }
        for (k, v) in &entries {
            kv.put(k, v.to_string().into()).await.map_err(fail)?;
        }
        let current = entries.keys().cloned().collect::<BTreeSet<_>>();
        for k in published.difference(&current) {
            kv.delete(k).await.map_err(fail)?;
        }
        published = current;
        vega_registry::heartbeat(&kv, &host).await.map_err(fail)?;
        save(d, file)?;
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

fn save(d: &Delivery, file: &std::path::Path) -> Result<(), Failure> {
    let tmp = file.with_extension("tmp");
    fs::write(&tmp, d.export().to_string()).map_err(fail)?;
    platform::replace(tmp, file).map_err(fail)
}

// Even with replacement stdio, Windows may inherit the starter's original pipe
// handles. Keeping those open prevents hc exec/capture from observing EOF.
// start runs before any worker threads; restore flags immediately after spawn.
#[cfg(windows)]
struct DetachedHandles(Vec<(windows_sys::Win32::Foundation::HANDLE, u32)>);
#[cfg(windows)]
impl DetachedHandles {
    fn new() -> std::io::Result<Self> {
        use windows_sys::Win32::{
            Foundation::{
                GetHandleInformation, SetHandleInformation, HANDLE_FLAG_INHERIT,
                INVALID_HANDLE_VALUE,
            },
            System::Console::{
                GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE,
            },
        };
        let mut guard = Self(Vec::new());
        for kind in [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
            // SAFETY: GetStdHandle returns borrowed process handles; never close them.
            let handle = unsafe { GetStdHandle(kind) };
            let mut flags = 0;
            if handle.is_null()
                || handle == INVALID_HANDLE_VALUE
                || unsafe { GetHandleInformation(handle, &mut flags) } == 0
            {
                continue;
            }
            if flags & HANDLE_FLAG_INHERIT != 0 {
                if unsafe { SetHandleInformation(handle, HANDLE_FLAG_INHERIT, 0) } == 0 {
                    return Err(std::io::Error::last_os_error());
                }
                guard.0.push((handle, flags));
            }
        }
        Ok(guard)
    }
}
#[cfg(windows)]
impl Drop for DetachedHandles {
    fn drop(&mut self) {
        for (handle, flags) in &self.0 {
            // SAFETY: borrowed handles stay alive throughout the synchronous spawn.
            unsafe {
                windows_sys::Win32::Foundation::SetHandleInformation(
                    *handle,
                    windows_sys::Win32::Foundation::HANDLE_FLAG_INHERIT,
                    *flags,
                );
            }
        }
    }
}
