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
        format!("{}.status.keychain", vega_registry::token(host)),
        LOCAL.lock().ok()?.clone()?,
    ))
}

/// Keychain status is a bounded display record, never caller-selected keys,
/// operations or identity metadata. Reject unknown fields and numeric forms.
#[derive(serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
struct Sample {
    state: State,
    since: u64,
    at: u64,
}

#[derive(serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "lowercase")]
enum State {
    Locked,
    Unlocked,
    Unknown,
}

const MAX_STATUS: usize = 256;

fn validate(from: &str, payload: &[u8], at: u64) -> Result<Sample, String> {
    if from != vega_registry::token(from) || !vega_herdr::state::is_inbox_only(from) {
        return Err("sender is not an inbox-only host".into());
    }
    if payload.len() > MAX_STATUS {
        return Err("keychain payload exceeds 256 bytes".into());
    }
    // Serde structs also accept positional arrays; the wire contract is
    // strictly an object containing these three named fields.
    if payload.iter().copied().find(|b| !b.is_ascii_whitespace()) != Some(b'{') {
        return Err("keychain sample must be an object".into());
    }
    let sample: Sample = serde_json::from_slice(payload).map_err(|_| "invalid keychain sample")?;
    if sample.since == 0
        || sample.since > sample.at
        || sample.at > at.saturating_add(30)
        || at.saturating_sub(sample.at) > 180
    {
        return Err("invalid or stale keychain timestamps".into());
    }
    Ok(sample)
}

/// Only a private daemon relays a validated inbox-only sender's status.
/// Construct a fresh publish: never forward caller headers such as Rollup.
pub async fn receive(
    client: &async_nats::Client,
    receiver: &str,
    from: &str,
    payload: &[u8],
) -> Result<(), (&'static str, String)> {
    if vega_herdr::state::is_inbox_only(receiver) {
        return Err((
            "denied",
            "keychain relay requires a private receiver".into(),
        ));
    }
    let sample = validate(from, payload, now()).map_err(|why| ("denied", why))?;
    let result = tokio::time::timeout(Duration::from_secs(5), async {
        async_nats::jetstream::new(client.clone())
            .publish(
                format!("$KV.{}.{}.status.keychain", vega_registry::BUCKET, from),
                serde_json::to_vec(&sample)
                    .map_err(|e| e.to_string())?
                    .into(),
            )
            .await
            .map_err(|e| e.to_string())?
            .await
            .map_err(|e| e.to_string())?;
        Ok::<_, String>(())
    })
    .await
    .unwrap_or_else(|_| Err("publish timed out".into()));
    result.map_err(|why| ("handler_failed", why))
}

/// Inbox-only vegad uses its theme control host as the app/relay host.
/// Independent of theme fetches and full registry reconciliation.
#[derive(Default)]
pub struct Publisher {
    last: Option<Value>,
    next: Option<std::time::Instant>,
    failed_state: Option<String>,
}

impl Publisher {
    pub async fn run(&mut self, client: &async_nats::Client, host: &str) {
        let Some((_, value)) = registry_entry(host) else {
            return;
        };
        let control = vega_theme_sync::Env::from_env()
            .ok_or_else(|| "no theme config environment".to_string())
            .and_then(|env| vega_theme_sync::prefs::load(&env).map(|p| p.control));
        self.publish(client, control.as_deref(), host, value).await;
    }

    pub async fn publish(
        &mut self,
        client: &async_nats::Client,
        control: Result<&str, &String>,
        host: &str,
        value: Value,
    ) {
        let now = std::time::Instant::now();
        let changed = self
            .last
            .as_ref()
            .is_none_or(|v| v["state"] != value["state"]);
        if !changed && self.next.is_some_and(|next| now < next) {
            return;
        }
        if changed {
            self.failed_state = None;
        }
        self.last = Some(value.clone());
        self.next = Some(now + EVERY);
        let result = tokio::time::timeout(Duration::from_secs(5), async {
            let control = control.map_err(|e| e.to_string())?;
            if control != vega_registry::token(control) {
                return Err("invalid control host".to_string());
            }
            let answer = client
                .request(
                    crate::cmd::subject(control, "keychain", host),
                    value.to_string().into(),
                )
                .await
                .map_err(|e| e.to_string())?;
            let reply: Value =
                serde_json::from_slice(&answer.payload).map_err(|e| e.to_string())?;
            if reply["ok"] != true {
                return Err("keychain relay refused status".into());
            }
            Ok::<_, String>(())
        })
        .await
        .unwrap_or_else(|_| Err("relay timed out".into()));
        if let Err(why) = result {
            let state = value["state"].as_str().unwrap_or("unknown");
            if self.failed_state.as_deref() != Some(state) {
                crate::log(&format!("keychain: status relay failed ({why})"));
                self.failed_state = Some(state.to_string());
            }
        }
    }
}

fn write(path: &Path, value: &Value) -> Result<(), String> {
    fs::create_dir_all(path.parent().ok_or("no parent")?).map_err(|e| e.to_string())?;
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, value.to_string()).map_err(|e| e.to_string())?;
    fs::rename(tmp, path).map_err(|e| e.to_string())
}

fn snapshot(view: &vega_registry::View, at: u64) -> Value {
    let rows: Vec<_> = view.entries.iter().filter_map(|(k, v)| {
        let host = k.strip_suffix(".status.keychain").filter(|h| !h.contains('.'))?;
        let fresh = v["at"].as_u64().is_some_and(|t| at.saturating_sub(t) <= 180);
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
    fn relay_accepts_only_bounded_recent_display_samples() {
        let at = now();
        for state in ["locked", "unlocked", "unknown"] {
            let payload = json!({"state":state,"since":at-100,"at":at}).to_string();
            assert!(validate("puck", payload.as_bytes(), at).is_ok());
            assert!(validate("proteus", payload.as_bytes(), at).is_err());
            assert!(validate("puck.extra", payload.as_bytes(), at).is_err());
        }
        for value in [
            json!({"state":"open","since":at,"at":at}),
            json!({"state":"locked","since":0,"at":at}),
            json!({"state":"locked","since":at+1,"at":at}),
            json!({"state":"locked","since":at,"at":at+31}),
            json!({"state":"locked","since":at-200,"at":at-181}),
            json!({"state":"locked","since":-1,"at":at}),
            json!({"state":"locked","since":1.5,"at":at}),
            json!({"state":"locked","since":at,"at":at,"key":"proteus.beat"}),
            json!({"state":"locked","since":at,"at":at,"operation":"PURGE"}),
            json!({"state":"locked","at":at}),
            json!(["locked", at, at]),
        ] {
            assert!(
                validate("puck", value.to_string().as_bytes(), at).is_err(),
                "{value}"
            );
        }
        assert!(validate("puck", &[b' '; MAX_STATUS + 1], at).is_err());
        assert!(validate("puck", b"not json", at).is_err());
        assert!(validate(
            "puck",
            b"{\"state\":\"locked\",\"state\":\"unlocked\",\"since\":1,\"at\":1}",
            at
        )
        .is_err());
    }

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
        // Inbox-only hosts have no beat; the sample carries its own freshness.
        v.entries
            .insert("mac.status.keychain".into(), advance(None, "locked", at));
        v.entries.insert(
            "old.status.keychain".into(),
            advance(None, "unlocked", at - 600),
        );
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
