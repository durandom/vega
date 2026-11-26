//! Minimal native hc worker. It never pulls inbox messages: only inbox owns acks.
use crate::{
    delivery::{self, Decision, Delivery, Draft},
    native_project::Projects,
    terminals,
};
use clap::Subcommand;
use serde_json::{json, Value};
#[cfg(not(windows))]
use std::process::{Command, Stdio};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File, OpenOptions},
    path::PathBuf,
    time::Duration,
};
use vega_herdr::{hc, platform, Failure};

const POLL_EVERY: Duration = Duration::from_secs(2);
const BEAT_EVERY: Duration = Duration::from_secs(10);

#[derive(Subcommand)]
pub enum Action {
    /// Ensure one detached worker is running for this user
    Start,
    /// Show whether the user-state worker lock is held
    Status {
        #[arg(long)]
        json: bool,
    },
    /// Ask the lock-owning worker to exit (idempotent)
    Stop,
    /// Run in the foreground (stop with Ctrl-C)
    Serve {
        #[arg(long, hide = true)]
        ready_file: Option<PathBuf>,
        #[cfg(windows)]
        #[arg(long, hide = true)]
        launch_file: Option<PathBuf>,
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
fn publish(path: &std::path::Path, contents: &str) -> Result<(), Failure> {
    let tmp = path.with_extension(format!("tmp-{}", nuid::next()));
    fs::write(&tmp, contents).map_err(fail)?;
    platform::replace(&tmp, path).map_err(fail)
}
pub fn run(action: Action) -> Result<(), Failure> {
    match action {
        Action::Start => start(),
        Action::Status { json } => status(json),
        Action::Stop => stop(),
        Action::Serve {
            ready_file,
            #[cfg(windows)]
            launch_file,
        } => {
            #[cfg(windows)]
            if let Some(p) = launch_file {
                scheduled::restore(&p)?;
            }
            let held = lock()?;
            let token = nuid::next().to_string();
            if held.is_some() {
                publish(
                    &root()?.join("worker.json"),
                    &json!({
                        "pid": std::process::id(), "token": token,
                        "exe": std::env::current_exe().map_err(fail)?
                    })
                    .to_string(),
                )?;
            }
            if let Some(p) = ready_file {
                publish(
                    &p,
                    &if held.is_some() {
                        format!("started\npid: {}", std::process::id())
                    } else {
                        "already running".into()
                    },
                )?;
            }
            let Some(_held) = held else {
                println!("relay: already running");
                return Ok(());
            };
            vega_herdr::bound_daemon_calls();
            let projects = Projects::start(crate::host());
            let _sidebar = crate::native_sidebar::start(projects.clone());
            let stop_file = root()?.join("stop");
            let result = crate::runtime()?.block_on(async {
                tokio::select! {
                    result = serve(&projects) => result,
                    _ = async {
                        loop {
                            if fs::read_to_string(&stop_file).ok().as_deref() == Some(&token) { break; }
                            tokio::time::sleep(Duration::from_millis(100)).await;
                        }
                    } => Ok(())
                }
            });
            let _ = fs::remove_file(root()?.join("worker.json"));
            let _ = fs::remove_file(stop_file);
            result
        }
    }
}
fn worker() -> Result<Value, Failure> {
    let p = root()?.join("worker.json");
    match fs::read(p) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(fail),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(json!({})),
        Err(e) => Err(fail(e)),
    }
}
fn status(as_json: bool) -> Result<(), Failure> {
    let running = lock()?.is_none();
    let info = if running { worker()? } else { json!({}) };
    let health = vega_herdr::daemon_health::read().and_then(|h| h.native);
    if as_json {
        println!(
            "{}",
            json!({"running": running, "pid": info["pid"], "exe": info["exe"], "native": health})
        );
    } else {
        println!("relay: {}", if running { "running" } else { "stopped" });
        if let Some(native) = health {
            print!(
                "{}",
                native.render(&crate::host(), vega_herdr::capacity::now())
            );
        }
        if let Some(pid) = info["pid"].as_u64() {
            println!("pid: {pid}");
        }
    }
    Ok(())
}
fn stop() -> Result<(), Failure> {
    if lock()?.is_some() {
        println!("relay: already stopped");
        return Ok(());
    }
    let info = worker()?;
    let token = info["token"].as_str().ok_or_else(|| {
        fail("running worker predates relay stop; stop its foreground terminal with Ctrl-C once")
    })?;
    fs::write(root()?.join("stop"), token).map_err(fail)?;
    let until = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        // Missing metadata can mean the old worker is still cleaning up while
        // holding its lock. Wait for release before an installer restarts it.
        if lock()?.is_some()
            || worker()?["token"]
                .as_str()
                .is_some_and(|current| current != token)
        {
            println!("relay: stopped");
            return Ok(());
        }
        if std::time::Instant::now() >= until {
            return Err(fail("worker did not stop within 10 seconds"));
        }
        std::thread::sleep(Duration::from_millis(50));
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
    #[cfg(windows)]
    {
        scheduled::start(&ready)
    }
    #[cfg(not(windows))]
    {
        start_direct(&ready)
    }
}
#[cfg(not(windows))]
fn start_direct(ready: &std::path::Path) -> Result<(), Failure> {
    let log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(root()?.join("worker.log"))
        .map_err(fail)?;
    let mut c = Command::new(std::env::current_exe().map_err(fail)?);
    c.args(["relay", "serve", "--ready-file"])
        .arg(ready)
        .env_remove("HC_TERMINAL_ID")
        .env_remove("HERDR_PANE_ID")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(log);
    let mut child = c.spawn().map_err(fail)?;
    let until = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        if let Ok(status) = fs::read_to_string(ready) {
            let _ = fs::remove_file(ready);
            println!("relay: {status}");
            return Ok(());
        }
        if child.try_wait().map_err(fail)?.is_some() || std::time::Instant::now() >= until {
            let _ = child.kill();
            let _ = child.wait();
            let _ = fs::remove_file(ready);
            return Err(fail(
                "worker did not acquire its lock; inspect native-relay/worker.log",
            ));
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}
async fn serve(projects: &Projects) -> Result<(), Failure> {
    let file = root()?.join("delivery.json");
    let mut d = Delivery::default();
    let mut saved = None;
    let mut hc = hc::Connection::default();
    if let Ok(v) = fs::read(&file) {
        d.import(&serde_json::from_slice::<Value>(&v).map_err(fail)?);
    }
    loop {
        if let Err(e) = session(&mut d, &file, &mut saved, &mut hc, projects).await {
            eprintln!("relay: {}: {}", e.code, e.message);
        }
        tokio::time::sleep(POLL_EVERY).await;
    }
}
async fn session(
    d: &mut Delivery,
    file: &std::path::Path,
    saved: &mut Option<Value>,
    hc: &mut hc::Connection,
    projects: &Projects,
) -> Result<(), Failure> {
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
    let mut published: BTreeMap<String, Option<Value>> = crate::subject_counts(
        &registry,
        &format!("$KV.{}.{host}.agent.>", vega_registry::BUCKET),
    )
    .await?
    .into_iter()
    .filter_map(|(s, _)| {
        s.strip_prefix(&format!("$KV.{}.", vega_registry::BUCKET))
            .map(|key| (key.to_owned(), None))
    })
    .collect();
    let mut next_beat = tokio::time::Instant::now();
    loop {
        // Failed inventory is not empty: retain state and let the heartbeat age.
        let agents = match hc.call("terminal.list", json!({})).and_then(|value| terminals::hc_agents(&value)) {
            Ok(agents) => agents,
            Err(error) => {
                // A failed scan may report health but cannot renew agent entries.
                let prior = kv.get(vega_registry::beat_key(&host)).await.map_err(fail)?;
                let beat = match prior {
                    Some(bytes) => serde_json::from_slice(&bytes).map_err(fail)?,
                    None => json!({"schema": 2, "at": 0}),
                };
                publish_beat(&kv, &host, beat).await?;
                return Err(error);
            }
        };
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
        let mut pending_projects = BTreeSet::new();
        let entries = vega_registry::agent_entries_with_project(&host, &agents, &[], |pane, cwd| {
            if cwd.as_os_str().is_empty() {
                return Ok(None);
            }
            match projects.lookup(cwd).as_deref() {
                Some(Ok(project)) => Ok(project.clone()),
                Some(Err(error)) => Err(Failure::new(&error.code, &error.message, &error.hint)),
                None => {
                    pending_projects.insert(vega_registry::agent_key(&host, pane));
                    Ok(None)
                }
            }
        });
        for a in &agents {
            let pane = a["pane_id"].as_str().expect("normalized hc pane");
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
            let mut params = json!({"terminal": pane, "lines": 0});
            if let Ok(caller) = std::env::var("HC_TERMINAL_ID") {
                if !caller.is_empty() {
                    params["caller"] = caller.into();
                }
            }
            let screen = hc.call("terminal.read", params);
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
            save(d, file, saved)?;
        }
        let stale = published
            .keys()
            .filter(|key| !entries.contains_key(*key))
            .cloned()
            .collect::<Vec<_>>();
        for (key, value) in entries {
            if !pending_projects.contains(&key)
                && published.get(&key).and_then(Option::as_ref) != Some(&value)
            {
                kv.put(&key, value.to_string().into()).await.map_err(fail)?;
                published.insert(key, Some(value));
            }
        }
        for key in stale {
            kv.delete(&key).await.map_err(fail)?;
            published.remove(&key);
        }
        if tokio::time::Instant::now() >= next_beat {
            publish_beat(&kv, &host, vega_registry::beat()).await?;
            next_beat = tokio::time::Instant::now() + BEAT_EVERY;
        }
        save(d, file, saved)?;
        tokio::time::sleep(POLL_EVERY).await;
    }
}

async fn publish_beat(
    kv: &async_nats::jetstream::kv::Store,
    host: &str,
    mut beat: Value,
) -> Result<(), Failure> {
    if let Some(native) = vega_herdr::daemon_health::read().and_then(|h| h.native) {
        beat["native"] = json!(native);
    }
    kv.put(vega_registry::beat_key(host), beat.to_string().into())
        .await
        .map_err(fail)?;
    Ok(())
}

fn save(d: &Delivery, file: &std::path::Path, saved: &mut Option<Value>) -> Result<(), Failure> {
    let contents = d.export();
    if saved.as_ref() != Some(&contents) {
        let tmp = file.with_extension("tmp");
        fs::write(&tmp, contents.to_string()).map_err(fail)?;
        platform::replace(tmp, file).map_err(fail)?;
        *saved = Some(contents);
    }
    Ok(())
}

#[cfg(windows)]
#[path = "native_relay_scheduled.rs"]
mod scheduled;
