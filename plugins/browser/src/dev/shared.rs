//! Global and VPN Chromium share their login context. Only a createTarget
//! response from this browser generation can introduce a tab into the registry.
use super::*;
use proxy::{Binding, Scope};

#[derive(Default, Serialize, Deserialize)]
struct Owned {
    port: u16,
    generation: String,
    active: u64,
    next: u64,
    tabs: BTreeMap<u64, Tab>,
}

fn directory(name: &str) -> Result<PathBuf, Failure> {
    let root = owner::file().ok_or_else(|| failure("Missing browser state directory"))?;
    Ok(root
        .with_file_name("shared")
        .join(project::encode_component(name)))
}

fn read(dir: &Path) -> Result<Option<Owned>, Failure> {
    match fs::read(dir.join("tabs.json")) {
        Ok(raw) => serde_json::from_slice(&raw).map(Some).map_err(failure),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(failure(e)),
    }
}

pub fn bound(name: &str) -> Result<Option<u16>, Failure> {
    Ok(read(&directory(name)?)?.map(|s| s.port))
}

/// Recorded adapter ports can be excluded without contacting a login browser.
pub(super) fn adapter_ports() -> Result<std::collections::BTreeSet<u16>, Failure> {
    let root = owner::file()
        .ok_or_else(|| failure("Missing browser state directory"))?
        .with_file_name("shared");
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Default::default()),
        Err(e) => return Err(failure(e)),
    };
    let mut ports = std::collections::BTreeSet::new();
    for entry in entries {
        let entry = entry.map_err(failure)?;
        if entry.file_type().map_err(failure)?.is_dir() {
            if let Some(session) = read(&entry.path())? {
                ports.extend(session.tabs.values().map(|tab| tab.port));
            }
        }
    }
    Ok(ports)
}

fn persist(dir: &Path, state: &Owned) -> Result<(), Failure> {
    let mut tmp = tempfile::NamedTempFile::new_in(dir).map_err(failure)?;
    serde_json::to_writer(&mut tmp, state).map_err(failure)?;
    tmp.persist(dir.join("tabs.json")).map_err(failure)?;
    Ok(())
}

fn generation(port: u16) -> Result<String, Failure> {
    let agent = ureq::AgentBuilder::new()
        .redirects(0)
        .timeout(std::time::Duration::from_secs(3))
        .build();
    let version: Value = agent
        .get(&format!("http://127.0.0.1:{port}/json/version"))
        .call()
        .map_err(failure)?
        .into_json()
        .map_err(failure)?;
    let url = version["webSocketDebuggerUrl"].as_str().unwrap_or_default();
    let prefix = format!("ws://127.0.0.1:{port}/devtools/browser/");
    let id = url
        .strip_prefix(&prefix)
        .filter(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-'))
        .ok_or_else(|| failure("Invalid shared Chromium browser generation"))?;
    Ok(format!("/devtools/browser/{id}"))
}

fn prune(state: &mut Owned, cdp: &mut Cdp) -> Result<(), Failure> {
    let targets = cdp.call("Target.getTargets", json!({"filter":[{"type":"page"}]}))?;
    let pages = targets["targetInfos"]
        .as_array()
        .ok_or_else(|| failure("Missing CDP target list"))?;
    let info = |target: &str| {
        pages
            .iter()
            .find(|p| p["type"] == "page" && p["targetId"] == target)
    };
    state.tabs.retain(|_, tab| info(&tab.target).is_some());
    Ok(())
}

fn close(cdp: &mut Cdp, tab: &Tab) -> Result<(), Failure> {
    if cdp.call("Target.closeTarget", json!({"targetId":tab.target}))?["success"] != true {
        return Err(failure("Chromium did not close the owned tab"));
    }
    Ok(())
}

fn release(dir: &Path, state: &Owned, cdp: &mut Cdp) -> Result<(), Failure> {
    for tab in state.tabs.values() {
        close(cdp, tab)?;
    }
    if dir.join("tabs.json").exists() {
        fs::remove_file(dir.join("tabs.json")).map_err(failure)?;
    }
    Ok(())
}

/// Recheck both the binding and the owner under the same locks as foreground calls.
pub fn gc_close(name: &str) -> Result<bool, Failure> {
    let dir = directory(name)?;
    let _guard = lock(&dir)?;
    let Some(mut state) = read(&dir)? else {
        return Ok(false);
    };
    let owners = owner::Locked::acquire().map_err(failure)?;
    let Some(record) = owners.get(name).map_err(failure)? else {
        return Ok(false);
    };
    let Some(pane) = record.pane else {
        return Ok(false);
    };
    let local = vega_herdr::local_host();
    if local.is_empty()
        || local == "unknown"
        || record.host != local
        || crate::pane::alive(&pane) != Some(false)
    {
        return Ok(false);
    }
    let mut cdp = Cdp::endpoint(state.port, &state.generation)?;
    prune(&mut state, &mut cdp)?;
    release(&dir, &state, &mut cdp)?;
    owners.remove(name).map_err(failure)?;
    Ok(true)
}

fn allocate(cdp: &mut Cdp, dir: &Path, state: &mut Owned, background: bool) -> Result<(), Failure> {
    let target = cdp.create_target("about:blank", None, background, false)?;
    state.next += 1;
    state.active = state.next;
    state.tabs.insert(
        state.active,
        Tab {
            target: target.clone(),
            port: 0,
            chrome_tab_id: None,
            init_script: None,
        },
    );
    if let Err(e) = persist(dir, state) {
        let _ = cdp.call("Target.closeTarget", json!({"targetId":target}));
        return Err(e);
    }
    Ok(())
}

pub fn background_tab_new(args: &[String]) -> bool {
    Invocation::parse(args).is_ok_and(|call| call.background_tab_new())
}

pub fn command(args: &[String], port: u16) -> Result<ExitCode, Failure> {
    execute(args, port, false)
}

/// A plain named shim call may reconnect only to the generation verified by ab.
pub fn resume(args: &[String], port: u16) -> Result<ExitCode, Failure> {
    execute(args, port, true)
}

fn execute(args: &[String], port: u16, resume: bool) -> Result<ExitCode, Failure> {
    let call = Invocation::parse(args)?;
    let dir = directory(&call.name)?;
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&dir)
        .map_err(failure)?;
    let _guard = lock(&dir)?;
    let mut state = read(&dir)?.unwrap_or_default();
    if state.port != 0 && state.port != port {
        return Err(failure(
            "Session already belongs to another Chromium profile",
        ));
    }
    if resume && state.port == 0 {
        return Err(failure("Shared session was closed"));
    }
    let path = generation(port)?;
    if resume && state.generation != path {
        return Err(failure(
            "Shared Chromium restarted. Reopen this session with vega browser ab",
        ));
    }
    if state.generation != path {
        state.tabs.clear();
        state.generation = path.clone();
        state.port = port;
    }
    let mut cdp = Cdp::endpoint(port, &path)?;
    prune(&mut state, &mut cdp)?;
    if !state.tabs.contains_key(&state.active) {
        state.active = state.tabs.keys().next().copied().unwrap_or_default();
    }
    let words = call.words.as_slice();
    let new = matches!(words, ["tab", "new", ..]);
    match words {
        ["close"] => {
            release(&dir, &state, &mut cdp)?;
            owner::remove(&call.name);
            call.output(json!({"closed":true}), "✓ Browser closed");
            return passed();
        }
        ["tab", "close", rest @ ..] => {
            let id = rest
                .first()
                .map(|s| tab_id(s))
                .transpose()?
                .unwrap_or(state.active);
            if !state.tabs.contains_key(&id) {
                return Err(usage("Tab is not owned by this session"));
            }
            if state.active == id {
                // Give Chrome an owned successor before it closes the active
                // target. Its normal adjacent-tab fallback could focus a user tab.
                if let Some((&next, tab)) = state.tabs.iter().find(|(other, _)| **other != id) {
                    cdp.activate_target(&tab.target)?;
                    state.active = next;
                } else {
                    allocate(&mut cdp, &dir, &mut state, false)?;
                }
            }
            close(&mut cdp, &state.tabs[&id])?;
            state.tabs.remove(&id);
            persist(&dir, &state)?;
            call.output(
                json!({"closed":true,"tabId":format!("t{id}")}),
                format!("✓ Tab [t{id}] closed"),
            );
            return passed();
        }
        ["tab", id] if !matches!(*id, "list" | "new") => {
            let id = tab_id(id)?;
            let tab = state
                .tabs
                .get(&id)
                .ok_or_else(|| usage("Tab is not owned by this session"))?;
            cdp.activate_target(&tab.target)?;
            state.active = id;
            persist(&dir, &state)?;
            call.output(
                json!({"tabId":format!("t{id}")}),
                format!("✓ Switched to tab [t{id}]"),
            );
            return passed();
        }
        _ => {}
    }
    if new || state.tabs.is_empty() {
        allocate(&mut cdp, &dir, &mut state, call.background_tab_new())?;
    }
    let record = owner::record_from_env(vega_shim::unix_now());
    let project = record.project.clone();
    owner::upsert(&call.name, record);
    if matches!(words, ["tab"] | ["tab", "list"]) {
        let mut tabs = Vec::new();
        let mut lines = Vec::new();
        for (id, tab) in &state.tabs {
            let result = cdp.call("Target.getTargetInfo", json!({"targetId":tab.target}))?;
            let page = &result["targetInfo"];
            let title = page["title"].as_str().unwrap_or_default();
            let url = page["url"].as_str().unwrap_or_default();
            let active = *id == state.active;
            tabs.push(json!({"tabId":format!("t{id}"),"title":title,"url":url,"active":active}));
            lines.push(format!(
                "{} [t{id}] {title} - {url}",
                if active { "→" } else { " " }
            ));
        }
        call.output(json!({"tabs":tabs}), lines.join("\n"));
        return passed();
    }
    let active = state.active;
    let total = state.tabs.len();
    let tab = state.tabs.get_mut(&active).expect("owned active tab");
    let binding = Binding {
        scope: Scope::Shared { port, path },
        target: tab.target.clone(),
    };
    if tab.port == 0 || !proxy::alive(tab.port, &binding) {
        tab.port = proxy::start(&binding)?;
    }
    let relay = tab.port;
    persist(&dir, &state)?;
    if words == ["get", "cdp-url"] {
        let url = format!("ws://127.0.0.1:{relay}/devtools/browser/{}", binding.target);
        call.output(json!({"url":url}), &url);
        return passed();
    }
    if words != ["tab", "new"] {
        let status = run_agent(
            dir.parent().expect("shared state directory"),
            &runtime_config(&dir)?,
            &binding,
            &call,
            relay,
            (call.init_script.is_none() && mark::wanted(args))
                .then(|| mark::mark_for(&call.name, project.as_deref()))
                .flatten(),
        )?;
        if !status.success() {
            return Ok(ExitCode::from(status.code().unwrap_or(1) as u8));
        }
    }
    if new {
        call.output(
            json!({"tabId":format!("t{active}"),"total":total}),
            format!("✓ Tab opened [t{active}] ({total} total)"),
        );
    }
    passed()
}
