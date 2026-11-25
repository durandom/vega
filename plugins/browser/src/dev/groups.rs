//! The extension creates tabs in its own window. CDP independently verifies
//! their ownership before the adapter can expose them to agent-browser.
use std::fs;
use std::io::Write;
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use vega_herdr::Failure;

use super::{failure, wire::Cdp, Project};

const ID: &str = "ddhpkjdjaccmkefgalpmnbnpibiimapm";
const ALLOCATION_TIMEOUT: Duration = Duration::from_secs(3);
const EXTENSION: &[(&str, &str)] = &[
    (
        "manifest.json",
        include_str!("../../extension/manifest.json"),
    ),
    ("control.html", include_str!("../../extension/control.html")),
    ("control.js", include_str!("../../extension/control.js")),
];

pub(super) fn package(profile: &Path) -> Result<PathBuf, Failure> {
    let path = profile.join("vega-extension");
    match fs::DirBuilder::new().mode(0o700).create(&path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            if !path.symlink_metadata().is_ok_and(|meta| meta.is_dir()) {
                return Err(failure(
                    "The managed CfT extension path must be a directory",
                ));
            }
        }
        Err(e) => return Err(failure(e)),
    }
    for (name, body) in EXTENSION {
        let mut file = tempfile::NamedTempFile::new_in(&path).map_err(failure)?;
        file.write_all(body.as_bytes()).map_err(failure)?;
        file.persist(path.join(name)).map_err(failure)?;
    }
    Ok(path)
}

fn control_page(
    cdp: &mut Cdp,
    project: Option<&Project>,
    background: bool,
) -> Result<String, Failure> {
    let url = format!("chrome-extension://{ID}/control.html");
    // The regular window may have been closed while a project is active.
    // A hidden target checks the dedicated profile without opening a window.
    cdp.create_target(&url, project, background, project.is_none())
        .map_err(|error| {
            let scope = if project.is_some() {
                "project"
            } else {
                "regular"
            };
            failure(format!(
                "Could not open the {scope} extension control page: {}",
                error.message
            ))
        })
}

fn attach(cdp: &mut Cdp, target: &str) -> Result<String, Failure> {
    cdp.string(
        "Target.attachToTarget",
        json!({"targetId": target, "flatten": true}),
        "sessionId",
    )
}

fn wait_for_control(cdp: &mut Cdp, session: &str) -> Result<Option<Value>, Failure> {
    let deadline = Instant::now() + Duration::from_secs(2);
    let expression = format!(
        "location.href === 'chrome-extension://{ID}/control.html' &&          typeof vegaAllocate === 'function' &&          {{nonce: crypto.randomUUID(), incognito: chrome.extension.inIncognitoContext}}"
    );
    while Instant::now() < deadline {
        let response = cdp.call_in(
            Some(session),
            "Runtime.evaluate",
            json!({"expression": expression, "returnByValue": true}),
        )?;
        let value = &response["result"]["value"];
        if value.is_object() {
            return Ok(Some(value.clone()));
        }
        thread::sleep(Duration::from_millis(25));
    }
    Ok(None)
}

fn close(cdp: &mut Cdp, target: &str) -> Result<(), Failure> {
    if cdp.call("Target.closeTarget", json!({"targetId": target}))?["success"] != true {
        return Err(failure("Chrome did not close a Vega setup tab"));
    }
    // CDP acknowledges a close request before Chrome removes the tab.
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        if !page_targets(cdp, deadline)?
            .iter()
            .any(|info| info["targetId"] == target)
        {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(25));
    }
    Err(failure("Chrome did not remove a closed Vega setup tab"))
}

fn has_access(cdp: &mut Cdp) -> Result<bool, Failure> {
    let target = control_page(cdp, None, true)?;
    let access = (|| {
        let session = attach(cdp, &target)?;
        if wait_for_control(cdp, &session)?.is_none() {
            return Err(Failure::new(
                "dev_extension_unavailable",
                "Vega Dev Tab Groups is unavailable in the managed Chrome for Testing window",
                "Restart the managed Chrome for Testing app and retry",
            ));
        }
        let response = cdp.call_in(
            Some(&session),
            "Runtime.evaluate",
            json!({
                "expression": "chrome.extension.isAllowedIncognitoAccess()",
                "awaitPromise": true,
                "returnByValue": true
            }),
        )?;
        Ok(response["result"]["value"] == true)
    })();
    let cleanup = close(cdp, &target);
    let allowed = access?;
    cleanup?;
    Ok(allowed)
}

fn enable_access(cdp: &mut Cdp) -> Result<(), Failure> {
    let target = cdp.string(
        "Target.createTarget",
        json!({"url": format!("chrome://extensions/?id={ID}"), "hidden": true, "background": true}),
        "targetId",
    )?;
    let result = (|| {
        let session = attach(cdp, &target)?;
        let deadline = Instant::now() + Duration::from_secs(3);
        let script = include_str!("enable-incognito.js");
        let expression = format!("({script})({})", json!(ID));
        while Instant::now() < deadline {
            let response = cdp.call_in_until(
                Some(&session),
                "Runtime.evaluate",
                json!({"expression": expression, "returnByValue": true, "awaitPromise": true}),
                deadline,
            )?;
            if response["result"]["value"] == true {
                return Ok(());
            }
            thread::sleep(Duration::from_millis(25));
        }
        Err(failure(
            "Chrome did not show the Vega extension incognito toggle",
        ))
    })();
    let cleanup = close(cdp, &target);
    result.and(cleanup)
}

pub(super) fn require_access(cdp: &mut Cdp) -> Result<(), Failure> {
    // A windowless startup has no debuggable frame, so Chrome rejects hidden
    // targets until a page exists. Open the regular window without activating it.
    if page_targets(cdp, Instant::now() + ALLOCATION_TIMEOUT)?.is_empty() {
        cdp.create_target("about:blank", None, true, false)?;
    }
    if has_access(cdp)? {
        return Ok(());
    }
    if enable_access(cdp).is_ok() && has_access(cdp).unwrap_or(false) {
        return Ok(());
    }
    Err(Failure::new(
        "dev_extension_incognito_required",
        "Vega Dev Tab Groups needs incognito access in the managed Chrome for Testing profile",
        &format!(
            "In the regular managed Chrome for Testing window, enable Developer mode on chrome://extensions, \
             then open chrome://extensions/?id={ID}, enable Allow in Incognito and retry. \
             Set VEGA_BROWSER_MARK=0 to open unmarked tabs."
        ),
    ))
}

fn allocation_timeout(step: &str) -> Failure {
    Failure::new(
        "dev_tab_allocation_timeout",
        format!("Timed out allocating a Vega dev tab: {step}"),
        "Retry the dev command, or set VEGA_BROWSER_MARK=0 to open unmarked tabs",
    )
}

pub(super) fn allocate(
    cdp: &mut Cdp,
    project: &mut Project,
    name: &str,
    owned: &[i64],
    background: bool,
) -> Result<(String, i64), Failure> {
    let control = control_page(cdp, Some(project), background)?;
    let mut nonce_url = None;
    let mut verified_target = None;
    let mut window = project.window;
    let result = (|| {
        if window == 0 {
            window = cdp.window(&control)?;
        }
        let verified = Project {
            window,
            ..project.clone()
        };
        cdp.info(&verified, &control)?;
        let session = attach(cdp, &control)?;
        let ready = wait_for_control(cdp, &session)?
            .filter(|value| value["incognito"] == true)
            .ok_or_else(|| failure("The Vega extension did not load in the project context"))?;
        let nonce = ready["nonce"]
            .as_str()
            .ok_or_else(|| failure("The Vega extension returned no allocation nonce"))?;
        let url = nonce_url.insert(format!("about:blank#vega-{nonce}"));
        let title = format!("{} · {name}", project.label);
        let colour = crate::mark::session_colour(name, Some(&project.label));
        let object = control_object(cdp, &session)?;
        let deadline = Instant::now() + ALLOCATION_TIMEOUT;
        let response = cdp.call_in_until(
            Some(&session),
            "Runtime.callFunctionOn",
            json!({
                "objectId": object,
                "functionDeclaration": "function(url, title, colour, ids) { return this.vegaAllocate(url, title, colour, ids); }",
                "arguments": [{"value": url}, {"value": title}, {"value": group_colour(&colour)}, {"value": owned}],
                "awaitPromise": true,
                "returnByValue": true
            }),
            deadline,
        ).map_err(|error| {
            if error.code == "dev_cdp_read_timeout" {
                allocation_timeout("creating and grouping the tab")
            } else {
                error
            }
        })?;
        let tab_id = response["result"]["value"].as_i64().ok_or_else(|| {
            failure(format!(
                "The Vega extension could not group the new tab: {}",
                response["exceptionDetails"]["exception"]["description"]
                    .as_str()
                    .and_then(|description| description.lines().next())
                    .unwrap_or("missing tab ID")
            ))
        })?;
        loop {
            if Instant::now() >= deadline {
                return Err(allocation_timeout("locating the new Chrome target"));
            }
            match matching_targets(cdp, project, url, deadline)
                .map_err(|error| match error.code.as_str() {
                    "dev_cdp_read_timeout" => allocation_timeout("locating the new Chrome target"),
                    _ => error,
                })?
                .as_slice()
            {
                [target] => {
                    cdp.info(&verified, target)?;
                    verified_target = Some(target.clone());
                    let session = attach(cdp, target)?;
                    let reply = cdp.call_in(
                        Some(&session),
                        "Page.navigate",
                        json!({"url": "about:blank"}),
                    )?;
                    if !reply["errorText"].is_null() {
                        return Err(failure("Could not clear the Vega allocation nonce"));
                    }
                    cdp.call("Target.detachFromTarget", json!({"sessionId": session}))?;
                    return Ok((target.clone(), tab_id));
                }
                [] => thread::sleep(Duration::from_millis(25)),
                _ => {
                    return Err(failure(
                        "Multiple CDP targets matched the Vega extension tab",
                    ))
                }
            }
        }
    })();
    let cleanup = close(cdp, &control);
    match (result, cleanup) {
        (Ok(binding), Ok(())) => {
            project.window = window;
            Ok(binding)
        }
        (result, cleanup) => {
            if let Some(target) = verified_target {
                let _ = close(cdp, &target);
            } else if let Some(url) = nonce_url {
                let deadline = Instant::now() + ALLOCATION_TIMEOUT;
                if let Ok(matches) = matching_targets(cdp, project, &url, deadline) {
                    if let [target] = matches.as_slice() {
                        if cdp.window(target).ok() == Some(window) {
                            let _ = close(cdp, target);
                        }
                    }
                }
            }
            Err(result
                .err()
                .or_else(|| cleanup.err())
                .expect("allocation or cleanup failed"))
        }
    }
}

fn control_object(cdp: &mut Cdp, session: &str) -> Result<String, Failure> {
    cdp.call_in(
        Some(session),
        "Runtime.evaluate",
        json!({"expression": "globalThis"}),
    )?["result"]["objectId"]
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| failure("Chrome returned no Vega extension execution object"))
}

fn page_targets(cdp: &mut Cdp, deadline: Instant) -> Result<Vec<Value>, Failure> {
    let mut response = cdp.call_in_until(
        None,
        "Target.getTargets",
        json!({"filter": [{"type": "page"}]}),
        deadline,
    )?;
    match response["targetInfos"].take() {
        Value::Array(items) => Ok(items),
        _ => Err(failure("Chrome returned no target list")),
    }
}

fn matching_targets(
    cdp: &mut Cdp,
    project: &Project,
    url: &str,
    deadline: Instant,
) -> Result<Vec<String>, Failure> {
    page_targets(cdp, deadline)?
        .iter()
        .filter(|info| info["url"] == url && info["browserContextId"] == project.context)
        .map(|info| {
            info["targetId"]
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| failure("Chrome returned no target ID"))
        })
        .collect()
}

fn group_colour(hex: &str) -> &'static str {
    // Chrome's light tab-strip palette. Pick the closest RGB match for both
    // custom project colours and the stable mark.rs session palette.
    const PALETTE: [(&str, [i32; 3]); 9] = [
        ("grey", [95, 99, 104]),
        ("blue", [26, 115, 232]),
        ("red", [217, 48, 37]),
        ("yellow", [249, 171, 0]),
        ("green", [24, 128, 56]),
        ("pink", [255, 99, 199]),
        ("purple", [161, 66, 244]),
        ("cyan", [0, 123, 131]),
        ("orange", [250, 144, 62]),
    ];
    let Ok(rgb) = u32::from_str_radix(hex.trim_start_matches('#'), 16) else {
        return "grey";
    };
    let rgb = [
        (rgb >> 16) as i32,
        ((rgb >> 8) & 255) as i32,
        (rgb & 255) as i32,
    ];
    PALETTE
        .into_iter()
        .min_by_key(|(_, colour)| {
            rgb.iter()
                .zip(colour)
                .map(|(a, b)| (a - b).pow(2))
                .sum::<i32>()
        })
        .map_or("grey", |(name, _)| name)
}

#[cfg(test)]
mod tests {
    use base64::{engine::general_purpose::STANDARD, Engine};
    use sha2::{Digest, Sha256};

    use super::*;

    #[test]
    fn packaged_key_matches_the_extension_id() {
        let manifest: Value = serde_json::from_str(EXTENSION[0].1).unwrap();
        let key = STANDARD.decode(manifest["key"].as_str().unwrap()).unwrap();
        let id: String = Sha256::digest(key)[..16]
            .iter()
            .flat_map(|byte| {
                [
                    char::from(b'a' + (byte >> 4)),
                    char::from(b'a' + (byte & 15)),
                ]
            })
            .collect();
        assert_eq!(id, ID);
    }
}
