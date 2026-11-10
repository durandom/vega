//! Read-only macOS login-keychain probe and the app host's registry snapshot.
use serde_json::{json, Value};
use std::{fs, path::Path, process::Command, sync::Mutex, time::Duration};

static LOCAL: Mutex<Option<Value>> = Mutex::new(None);
const EVERY: Duration = Duration::from_secs(60);

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn probe(program: &Path, keychain: &Path) -> &'static str {
    match Command::new(program)
        .arg("show-keychain-info")
        .arg(keychain)
        .output()
    {
        Ok(o) if o.status.success() => "unlocked",
        Ok(o)
            if o.status.code() == Some(36)
                && String::from_utf8_lossy(&o.stderr)
                    .contains("User interaction is not allowed") =>
        {
            "locked"
        }
        _ => "unknown",
    }
}

fn advance(old: Option<&Value>, state: &str, at: u64) -> Value {
    let since = old
        .filter(|o| o["state"] == state)
        .and_then(|o| o["since"].as_u64())
        .unwrap_or(at);
    json!({"state":state,"since":since,"at":at})
}

pub fn spawn() {
    if !cfg!(target_os = "macos") {
        return;
    }
    std::thread::spawn(|| {
        let Ok(dir) = crate::service::state_dir() else {
            return;
        };
        let Ok(home) = std::env::var("HOME") else {
            return;
        };
        let path = dir.join("keychain-local.json");
        let mut old: Option<Value> = fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok());
        loop {
            let state = probe(
                Path::new("/usr/bin/security"),
                &Path::new(&home).join("Library/Keychains/login.keychain-db"),
            );
            let value = advance(old.as_ref(), state, now());
            if old.as_ref().is_none_or(|o| o["state"] != state) {
                crate::log(&format!("keychain: {state}"));
            }
            if let Ok(mut local) = LOCAL.lock() {
                *local = Some(value.clone());
            }
            let _ = write(&path, &value);
            old = Some(value);
            std::thread::sleep(EVERY);
        }
    });
}

pub fn registry_entry(host: &str) -> Option<(String, Value)> {
    Some((
        format!("{}.keychain", vega_registry::token(host)),
        LOCAL.lock().ok()?.clone()?,
    ))
}

fn write(path: &Path, value: &Value) -> Result<(), String> {
    fs::create_dir_all(path.parent().ok_or("no parent")?).map_err(|e| e.to_string())?;
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, value.to_string()).map_err(|e| e.to_string())?;
    fs::rename(tmp, path).map_err(|e| e.to_string())
}

fn snapshot(view: &vega_registry::View, at: u64) -> Value {
    let rows: Vec<_> = view.entries.iter().filter_map(|(k, v)| {
        let host = k.strip_suffix(".keychain").filter(|h| !h.contains('.'))?;
        let fresh = view.fresh(host) && v["at"].as_u64().is_some_and(|t| at.saturating_sub(t) <= 180);
        Some(json!({"host":host,"state":if fresh {v["state"].as_str().unwrap_or("unknown")} else {"unknown"},"since":v["since"],"at":v["at"]}))
    }).collect();
    json!({"at":at,"rows":rows})
}

pub fn cache(view: &vega_registry::View, host: &str) {
    if let Ok(dir) = crate::service::state_dir() {
        // Cache failure must not block registry reconciliation. The mod ages out old data.
        let mut data = snapshot(view, now());
        data["local"] = json!(vega_registry::token(host));
        let _ = write(&dir.join("keychains.json"), &data);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn fake_security_codes_and_exact_argv() {
        let dir = tempfile::tempdir().unwrap();
        let security = dir.path().join("security");
        for (code, message, state) in [
            (0, "", "unlocked"),
            (36, "User interaction is not allowed", "locked"),
            (36, "other", "unknown"),
            (1, "User interaction is not allowed", "unknown"),
        ] {
            fs::write(&security, format!("#!/bin/sh\n[ \"$#\" = 2 ] && [ \"$1\" = show-keychain-info ] && [ \"$2\" = '/home/a b/login.keychain-db' ] || exit 99\nprintf '%s' '{message}' >&2\nexit {code}\n")).unwrap();
            fs::set_permissions(&security, fs::Permissions::from_mode(0o700)).unwrap();
            assert_eq!(
                probe(&security, Path::new("/home/a b/login.keychain-db")),
                state
            );
        }
        assert_eq!(
            probe(&dir.path().join("missing"), Path::new("unused")),
            "unknown"
        );
    }

    #[test]
    fn since_survives_repeated_samples_and_restart() {
        let a = advance(None, "locked", 10);
        let b = advance(Some(&a), "locked", 70);
        assert_eq!(b, json!({"state":"locked","since":10,"at":70}));
        assert_eq!(advance(Some(&b), "unlocked", 130)["since"], 130);
    }

    #[test]
    fn fleet_cache_keeps_stale_hosts_unknown_and_writes_atomically() {
        let at = now();
        let mut v = vega_registry::View::default();
        v.entries.insert("mac.beat".into(), json!({"at":at}));
        v.entries
            .insert("mac.keychain".into(), advance(None, "locked", at));
        v.entries
            .insert("old.keychain".into(), advance(None, "unlocked", at - 600));
        v.entries.insert("linux.beat".into(), json!({"at":at}));
        let data = snapshot(&v, at);
        assert_eq!(data["rows"].as_array().unwrap().len(), 2);
        assert_eq!(data["rows"][0]["state"], "locked");
        assert_eq!(data["rows"][1]["state"], "unknown");
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("keychains.json");
        write(&path, &data).unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&fs::read(path).unwrap()).unwrap(),
            data
        );
    }
}
