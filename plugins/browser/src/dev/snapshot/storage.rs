use std::time::{Duration, Instant};

use serde_json::{json, Value};
use vega_herdr::Failure;

use super::{failure, Cdp, LocalStorage, Project};

pub(super) fn access(
    cdp: &mut Cdp,
    project: &Project,
    origin: &str,
    entries: Option<&LocalStorage>,
) -> Result<LocalStorage, Failure> {
    let denied = || {
        failure(format!(
            "Could not access first-party localStorage for {origin}"
        ))
    };
    let target = cdp
        .string(
            "Target.createTarget",
            json!({"url": "about:blank", "browserContextId": project.context, "newWindow": true}),
            "targetId",
        )
        .map_err(|_| denied())?;
    let result = (|| {
        let info = cdp.call("Target.getTargetInfo", json!({"targetId": target}))?;
        if info["targetInfo"]["browserContextId"] != project.context
            || info["targetInfo"]["type"] != "page"
        {
            return Err(denied());
        }
        let session = cdp.string(
            "Target.attachToTarget",
            json!({"targetId": target, "flatten": true}),
            "sessionId",
        )?;
        let storage = prepare(cdp, &session, origin)?;
        if let Some(entries) = entries {
            for (key, value) in entries {
                cdp.call_in(
                    Some(&session),
                    "DOMStorage.setDOMStorageItem",
                    json!({
                        "storageId": storage, "key": key, "value": value,
                    }),
                )?;
            }
        }
        let result = cdp.call_in(
            Some(&session),
            "DOMStorage.getDOMStorageItems",
            json!({"storageId": storage}),
        )?;
        let mut read_back: LocalStorage =
            serde_json::from_value(result["entries"].clone()).map_err(failure)?;
        if let Some(entries) = entries {
            let mut expected = entries.clone();
            expected.sort_unstable();
            read_back.sort_unstable();
            if read_back != expected {
                return Err(denied());
            }
        }
        Ok(read_back)
    })();

    let closed = cdp
        .call("Target.closeTarget", json!({"targetId": target}))
        .is_ok_and(|result| result["success"] == true);
    match result {
        Ok(entries) if closed => Ok(entries),
        _ => Err(denied()),
    }
}

// Intercept the top-level request before it reaches the network. This gives
// Chrome a real first-party storage key without starting the app or sending cookies.
fn prepare(cdp: &mut Cdp, session: &str, origin: &str) -> Result<Value, Failure> {
    let session = Some(session);
    let url = format!("{origin}/.well-known/vega-dev-state");
    cdp.call_in(
        session,
        "Network.setBypassServiceWorker",
        json!({"bypass": true}),
    )?;
    cdp.call_in(
        session,
        "Fetch.enable",
        json!({"patterns": [{"urlPattern": "*"}]}),
    )?;
    let navigate = cdp.send(session, "Page.navigate", json!({"url": url}))?;
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut fulfilled = false;
    let frame = loop {
        if Instant::now() >= deadline {
            return Err(failure("Dev storage navigation timed out"));
        }
        let reply = cdp.receive()?;
        if reply["id"] == navigate {
            let result = Cdp::result("Page.navigate", reply)?;
            if !fulfilled || !result["errorText"].is_null() {
                return Err(failure("Dev storage navigation was not intercepted"));
            }
            break result["frameId"].clone();
        }
        if reply["sessionId"].as_str() == session && reply["method"] == "Fetch.requestPaused" {
            let params = &reply["params"];
            if fulfilled || params["resourceType"] != "Document" || params["request"]["url"] != url
            {
                return Err(failure("Unexpected request during dev storage navigation"));
            }
            cdp.send(
                session,
                "Fetch.fulfillRequest",
                json!({
                    "requestId": params["requestId"], "responseCode": 200,
                    "responseHeaders": [
                        {"name": "Content-Type", "value": "text/html"},
                        {"name": "Cache-Control", "value": "no-store"},
                        {"name": "Content-Security-Policy", "value": "default-src 'none'"}
                    ],
                    "body": "PCFkb2N0eXBlIGh0bWw+"
                }),
            )?;
            fulfilled = true;
        }
    };
    loop {
        let result = cdp.call_in(session, "Page.getFrameTree", json!({}))?;
        let active = &result["frameTree"]["frame"];
        if active["id"] == frame && active["url"] == url && active["securityOrigin"] == origin {
            break;
        }
        if Instant::now() >= deadline {
            return Err(failure(
                "Chrome did not commit the expected dev storage origin",
            ));
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let result = cdp.call_in(
        session,
        "Storage.getStorageKeyForFrame",
        json!({"frameId": frame}),
    )?;
    let key = result["storageKey"]
        .as_str()
        .filter(|key| !key.is_empty())
        .ok_or_else(|| failure("Chrome returned no dev storage key"))?;
    Ok(json!({"storageKey": key, "isLocalStorage": true}))
}
