//! Browser-side request interception stays attached for the lifetime of a share tab.
use super::super::{failure, wire::Cdp, Browser, Project};
use base64::{engine::general_purpose::STANDARD, Engine};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::thread;
use url::Url;
use vega_herdr::Failure;

pub(super) type Origins = BTreeSet<String>;

pub(super) fn origin(raw: &str, debug_port: u16) -> Result<String, Failure> {
    let url = parse(raw).ok_or_else(|| failure("Use an exact HTTP(S) origin"))?;
    if url.path() != "/"
        || url.query().is_some()
        || url.fragment().is_some()
        || matches!(url.port_or_known_default(), Some(9223 | 9224))
        || url.port_or_known_default() == Some(debug_port)
    {
        return Err(failure(
            "Share origins must exclude paths and debugger ports",
        ));
    }
    Ok(url.origin().ascii_serialization())
}

fn parse(raw: &str) -> Option<Url> {
    let url = Url::parse(raw).ok()?;
    (matches!(url.scheme(), "http" | "https")
        && url.username().is_empty()
        && url.password().is_none()
        && url.host_str().is_some_and(|host| {
            host.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b".-:[]".contains(&b))
        }))
    .then_some(url)
}

pub(super) fn permits(origins: &Origins, raw: &str) -> bool {
    normalized(origins, raw).is_some()
}

pub(super) fn normalized(origins: &Origins, raw: &str) -> Option<Url> {
    parse(raw).filter(|url| origins.contains(&url.origin().ascii_serialization()))
}

fn policy(origins: &Origins) -> String {
    let hosts = origins.iter().cloned().collect::<Vec<_>>().join(" ");
    let sockets = origins
        .iter()
        .map(|origin| origin.replacen("http", "ws", 1))
        .collect::<Vec<_>>()
        .join(" ");
    // A separate CSP header intersects with the application's own policy.
    // Disallow popups, child documents, downloads and worker creation.
    format!("sandbox allow-scripts allow-forms allow-same-origin; default-src {hosts}; script-src {hosts} 'unsafe-inline' 'unsafe-eval'; style-src {hosts} 'unsafe-inline'; connect-src {hosts} {sockets}; worker-src 'none'; frame-src 'none'; object-src 'none'; base-uri 'self'; form-action {hosts}")
}

fn downloadable(params: &Value) -> bool {
    let Some(status) = params["responseStatusCode"].as_u64() else {
        return false;
    };
    if (300..400).contains(&status) {
        return false;
    }
    let mut html = false;
    for header in params["responseHeaders"].as_array().into_iter().flatten() {
        let name = header["name"].as_str().unwrap_or("");
        let value = header["value"].as_str().unwrap_or("");
        if name.eq_ignore_ascii_case("content-disposition")
            && !value
                .split(';')
                .next()
                .unwrap_or("")
                .trim()
                .eq_ignore_ascii_case("inline")
        {
            return true;
        }
        if name.eq_ignore_ascii_case("content-type") {
            html = value
                .split(';')
                .next()
                .is_some_and(|t| t.trim().eq_ignore_ascii_case("text/html"));
        }
    }
    !html
}

// Only an actual HTTP redirect can skip the HTML body path. A 300 or a
// malformed 302 may otherwise render an unprotected document.
fn redirect(params: &Value, origins: &Origins) -> Option<Value> {
    let status = params["responseStatusCode"].as_u64()?;
    if !matches!(status, 301 | 302 | 303 | 307 | 308) {
        return None;
    }
    let mut locations = params["responseHeaders"].as_array()?.iter().filter(|h| {
        h["name"]
            .as_str()
            .is_some_and(|n| n.eq_ignore_ascii_case("location"))
    });
    let location = locations.next()?["value"].as_str()?;
    let dest = parse(params["request"]["url"].as_str()?)?
        .join(location)
        .ok()?;
    if locations.next().is_some() || !permits(origins, dest.as_str()) {
        return None;
    }
    // An empty synthetic body cannot execute application code.
    Some(
        json!({"requestId":params["requestId"],"responseCode":status,"body":"",
        "responseHeaders":[{"name":"Location","value":dest.as_str()},{"name":"Cache-Control","value":"no-store"}]}),
    )
}

pub(super) fn guard(
    browser: &Browser,
    project: &Project,
    target: &str,
    origins: Origins,
    live: Arc<AtomicBool>,
) -> Result<(), Failure> {
    let mut cdp = Cdp::connect(browser)?;
    cdp.info(project, target)?;
    let session = cdp.string(
        "Target.attachToTarget",
        json!({"targetId":target,"flatten":true}),
        "sessionId",
    )?;
    for (method, params) in [
        ("Page.enable", json!({})),
        (
            "Page.setInterceptFileChooserDialog",
            json!({"enabled":true,"cancel":true}),
        ),
        ("Network.enable", json!({})),
        ("Network.setCacheDisabled", json!({"cacheDisabled":true})),
        ("Network.setBypassServiceWorker", json!({"bypass":true})),
        (
            "Fetch.enable",
            json!({"patterns":[{"urlPattern":"*","requestStage":"Request"}]}),
        ),
    ] {
        cdp.call_in(Some(&session), method, params)?;
    }
    cdp.socket
        .get_mut()
        .set_read_timeout(None)
        .map_err(failure)?;
    thread::spawn(move || {
        let csp = policy(&origins);
        let mut documents = BTreeMap::<u64, Value>::new();
        while let Ok(msg) = cdp.receive() {
            if msg["method"] == "Target.detachedFromTarget" && msg["params"]["sessionId"] == session
                || msg.get("error").is_some()
            {
                break;
            }
            if msg["method"] == "Page.frameNavigated" && msg["sessionId"] == session {
                let frame = &msg["params"]["frame"];
                if frame.get("parentId").is_none() && !safe_page(frame, &origins) {
                    break;
                }
            }
            if let Some(mut response) = msg["id"].as_u64().and_then(|id| documents.remove(&id)) {
                let Some(body) = msg["result"]["body"]
                    .as_str()
                    .filter(|s| s.len() <= 8 * 1024 * 1024)
                else {
                    break;
                };
                response["body"] = if msg["result"]["base64Encoded"] == true {
                    json!(body)
                } else {
                    json!(STANDARD.encode(body))
                };
                if cdp
                    .send(Some(&session), "Fetch.fulfillRequest", response)
                    .is_err()
                {
                    break;
                }
                continue;
            }
            if msg["sessionId"] != session || msg["method"] != "Fetch.requestPaused" {
                continue;
            }
            let p = &msg["params"];
            let request = &p["requestId"];
            let (method, args) = if !p["request"]["url"]
                .as_str()
                .is_some_and(|u| permits(&origins, u))
                || p.get("responseErrorReason").is_some()
                || downloadable(p)
            {
                (
                    "Fetch.failRequest",
                    json!({"requestId":request,"errorReason":"BlockedByClient"}),
                )
            } else if let Some(status) = p["responseStatusCode"].as_u64() {
                if (300..400).contains(&status) {
                    match redirect(p, &origins) {
                        Some(response) => ("Fetch.fulfillRequest", response),
                        None => (
                            "Fetch.failRequest",
                            json!({"requestId":request,"errorReason":"BlockedByClient"}),
                        ),
                    }
                } else {
                    let mut headers = p["responseHeaders"].as_array().cloned().unwrap_or_default();
                    headers.retain(|h| {
                        !matches!(
                            h["name"]
                                .as_str()
                                .unwrap_or("")
                                .to_ascii_lowercase()
                                .as_str(),
                            "content-length"
                                | "transfer-encoding"
                                | "content-encoding"
                                | "cache-control"
                        )
                    });
                    headers.push(json!({"name":"Content-Security-Policy","value":csp}));
                    headers.push(json!({"name":"Cache-Control","value":"no-store"}));
                    let response = json!({"requestId":request,"responseCode":status,"responseHeaders":headers});
                    let Ok(id) = cdp.send(
                        Some(&session),
                        "Fetch.getResponseBody",
                        json!({"requestId":request}),
                    ) else {
                        break;
                    };
                    documents.insert(id, response);
                    continue;
                }
            } else {
                (
                    "Fetch.continueRequest",
                    json!({"requestId":request,"interceptResponse":p["resourceType"]=="Document"}),
                )
            };
            if cdp.send(Some(&session), method, args).is_err() {
                break;
            }
        }
        live.store(false, Ordering::Release);
    });
    Ok(())
}

pub(super) fn safe_page(info: &Value, origins: &Origins) -> bool {
    info["url"] == "about:blank"
        || info["url"]
            .as_str()
            .is_some_and(|url| permits(origins, url))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use tungstenite::Message;

    #[test]
    fn exact_origins_cannot_inject_csp_or_allow_a_debugger() {
        for raw in [
            "http://*.example",
            "http://localhost:9223",
            "http://localhost:41327",
            "file:///tmp/x",
            "https://app.example/path",
            "https://user@app.example",
        ] {
            assert!(origin(raw, 41327).is_err(), "{raw}");
        }
        let origins = Origins::from([origin("http://127.0.0.1:8790", 41327).unwrap()]);
        assert!(permits(&origins, "http://127.0.0.1:8790/app"));
        for url in [
            "http://127.0.0.1:41327/json",
            "ws://127.0.0.1:41327/devtools/browser/fixture",
            "https://127.0.0.1:8790",
            "file:///tmp/state",
        ] {
            assert!(!permits(&origins, url));
        }
    }

    #[test]
    fn foreign_context_never_receives_an_attachment() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let fake = thread::spawn(move || {
            let mut ws = tungstenite::accept(listener.accept().unwrap().0).unwrap();
            let req: Value = serde_json::from_str(ws.read().unwrap().to_text().unwrap()).unwrap();
            assert_eq!(req["method"], "Target.getTargets");
            ws.send(Message::text(json!({"id":req["id"],"result":{"targetInfos":[{"type":"page","targetId":"tab","browserContextId":"other"}]}}).to_string())).unwrap();
            assert!(ws.read().is_err());
        });
        let browser = Browser {
            pid: 0,
            started: String::new(),
            executable: Default::default(),
            profile: Default::default(),
            port,
            path: "/fixture".into(),
        };
        let project = Project {
            label: "alpha".into(),
            context: "own".into(),
            window: 1,
            origins: Default::default(),
        };
        assert!(guard(
            &browser,
            &project,
            "tab",
            Origins::new(),
            Arc::new(AtomicBool::new(true))
        )
        .is_err());
        fake.join().unwrap();
    }
}
