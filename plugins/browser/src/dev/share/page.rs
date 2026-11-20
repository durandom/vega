//! A remote command selects one fixed operation. Only the broker speaks CDP.
use super::{failure, policy, Browser, Project};
use crate::dev::{groups, release, wire::Cdp};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread,
    time::{Duration, Instant},
};
use vega_herdr::Failure;

struct Tab {
    target: String,
    chrome_id: i64,
    refs: BTreeMap<String, i64>,
}

pub(super) struct Pages {
    browser: Browser,
    project: Project,
    origins: policy::Origins,
    live: Arc<AtomicBool>,
    tabs: BTreeMap<u32, Tab>,
    active: u32,
    next: u32,
}

impl Pages {
    pub fn new(
        browser: Browser,
        project: Project,
        origins: policy::Origins,
        live: Arc<AtomicBool>,
    ) -> Result<Self, Failure> {
        let mut pages = Self {
            browser,
            project,
            origins,
            live,
            tabs: BTreeMap::new(),
            active: 0,
            next: 1,
        };
        pages.allocate()?;
        Ok(pages)
    }

    fn allocate(&mut self) -> Result<(), Failure> {
        if self.tabs.len() >= 12 {
            return Err(failure("Share tab limit reached"));
        }
        let mut cdp = Cdp::connect(&self.browser)?;
        let owned = self
            .tabs
            .values()
            .map(|tab| tab.chrome_id)
            .collect::<Vec<_>>();
        let (target, chrome_id) = groups::allocate(&mut cdp, &mut self.project, "share", &owned)?;
        if let Err(err) = policy::guard(
            &self.browser,
            &self.project,
            &target,
            self.origins.clone(),
            Arc::clone(&self.live),
        ) {
            let _ = release(&mut cdp, &self.project, &target);
            return Err(err);
        }
        self.active = self.next;
        self.next += 1;
        self.tabs.insert(
            self.active,
            Tab {
                target,
                chrome_id,
                refs: BTreeMap::new(),
            },
        );
        Ok(())
    }

    pub fn close(&mut self) {
        if let Ok(mut cdp) = Cdp::connect(&self.browser) {
            for tab in self.tabs.values() {
                let _ = release(&mut cdp, &self.project, &tab.target);
            }
        }
    }

    pub fn command(&mut self, argv: &[String]) -> Result<Value, Failure> {
        if !self.live.load(Ordering::Acquire) {
            return Err(denied());
        }
        let args = argv.iter().map(String::as_str).collect::<Vec<_>>();
        match args.as_slice() {
            ["tab", "new"] | ["tab", "new", _] => {
                if args
                    .get(2)
                    .is_some_and(|url| !policy::permits(&self.origins, url))
                {
                    return Err(denied());
                }
                self.allocate()?;
                if let Some(url) = args.get(2) {
                    return self.command(&["open".into(), url.to_string()]);
                }
                return Ok(json!(format!("t{}", self.active)));
            }
            ["tab"] | ["tab", "list"] => {
                let mut cdp = Cdp::connect(&self.browser)?;
                let mut list = Vec::new();
                for (id, tab) in &self.tabs {
                    let info = cdp.info(&self.project, &tab.target)?;
                    if !policy::safe_page(&info, &self.origins) {
                        return Err(denied());
                    }
                    list.push(json!({"tabId":format!("t{id}"),"active":*id==self.active,"url":info["url"],"title":info["title"]}));
                }
                return Ok(json!(list));
            }
            ["tab", id] => {
                let id = id
                    .strip_prefix('t')
                    .and_then(|s| s.parse().ok())
                    .ok_or_else(denied)?;
                let tab = self.tabs.get(&id).ok_or_else(denied)?;
                let mut cdp = Cdp::connect(&self.browser)?;
                if !policy::safe_page(&cdp.info(&self.project, &tab.target)?, &self.origins) {
                    return Err(denied());
                }
                cdp.call("Target.activateTarget", json!({"targetId":tab.target}))?;
                self.active = id;
                return Ok(json!(format!("t{id}")));
            }
            _ => {}
        }
        let tab = self.tabs.get_mut(&self.active).ok_or_else(denied)?;
        let mut cdp = Cdp::connect(&self.browser)?;
        let info = cdp.info(&self.project, &tab.target)?;
        if !policy::safe_page(&info, &self.origins) {
            return Err(denied());
        }
        let input = matches!(
            args.first(),
            Some(&("click" | "press" | "keyboard" | "fill" | "type"))
        );
        let _focus = if input {
            let guard = crate::dev::lock(&self.browser.profile)?;
            cdp.call("Target.activateTarget", json!({"targetId":tab.target}))?;
            Some(guard)
        } else {
            None
        };
        let session = cdp.string(
            "Target.attachToTarget",
            json!({"targetId":tab.target,"flatten":true}),
            "sessionId",
        )?;
        let mut page = |method: &str, params: Value| {
            if !self.live.load(Ordering::Acquire) {
                return Err(denied());
            }
            cdp.call_in(Some(&session), method, params)
        };
        match args.as_slice() {
            ["open", raw] => {
                let normalized = policy::normalized(&self.origins, raw).ok_or_else(denied)?;
                let url = normalized.as_str();
                tab.refs.clear();
                let reply = page("Page.navigate", json!({"url":url}))?;
                if reply.get("errorText").is_some() {
                    return Err(failure("Share navigation failed"));
                }
                Ok(json!(url))
            }
            ["get", "url"] => Ok(info["url"].clone()),
            ["get", "title"] => Ok(info["title"].clone()),
            ["snapshot"] | ["snapshot", "-i" | "--interactive"] => {
                let tree = page("Accessibility.getFullAXTree", json!({}))?;
                tab.refs.clear();
                let mut lines = Vec::new();
                for node in tree["nodes"].as_array().ok_or_else(denied)? {
                    if node["ignored"] == true {
                        continue;
                    }
                    let role = node["role"]["value"].as_str().unwrap_or("");
                    let name = node["name"]["value"].as_str().unwrap_or("");
                    let interactive = matches!(
                        role,
                        "button"
                            | "link"
                            | "textbox"
                            | "searchbox"
                            | "combobox"
                            | "checkbox"
                            | "radio"
                            | "tab"
                            | "switch"
                            | "menuitem"
                            | "spinbutton"
                            | "slider"
                            | "option"
                    );
                    if let Some(backend) = node["backendDOMNodeId"].as_i64().filter(|_| interactive)
                    {
                        let key = format!("@e{}", tab.refs.len() + 1);
                        lines.push(format!("{key} {role} {name:?}"));
                        tab.refs.insert(key, backend);
                    } else if args.len() == 1 {
                        lines.push(format!("{role} {name:?}"));
                    }
                }
                Ok(json!(lines.join("\n")))
            }
            ["screenshot"] => Ok(
                json!({"png":page("Page.captureScreenshot",json!({"format":"png","captureBeyondViewport":false}))?["data"]}),
            ),
            ["wait", ms] if ms.parse::<u64>().is_ok_and(|n| n <= 10000) => {
                let until = Instant::now() + Duration::from_millis(ms.parse().unwrap());
                while self.live.load(Ordering::Acquire) && Instant::now() < until {
                    thread::sleep(
                        until
                            .saturating_duration_since(Instant::now())
                            .min(Duration::from_millis(50)),
                    );
                }
                Ok(json!(true))
            }
            ["wait", selector] if selector.len() <= 512 => {
                let deadline = Instant::now() + Duration::from_secs(10);
                loop {
                    let result = script(
                        &mut page,
                        "function(s){return !!document.querySelector(s)}",
                        json!([{"value":selector}]),
                        None,
                    )?;
                    if result["value"] == true {
                        return Ok(json!(true));
                    }
                    if Instant::now() >= deadline {
                        return Err(failure("Share wait expired"));
                    }
                    thread::sleep(Duration::from_millis(100));
                }
            }
            ["scroll", direction] | ["scroll", direction, _] => {
                let amount = args
                    .get(2)
                    .unwrap_or(&"600")
                    .parse::<u32>()
                    .ok()
                    .filter(|n| *n <= 10000)
                    .ok_or_else(denied)? as i64;
                let (x, y) = match *direction {
                    "up" => (0, -amount),
                    "down" => (0, amount),
                    "left" => (-amount, 0),
                    "right" => (amount, 0),
                    _ => return Err(denied()),
                };
                script(
                    &mut page,
                    "function(x,y){window.scrollBy(x,y)}",
                    json!([{"value":x},{"value":y}]),
                    None,
                )?;
                Ok(json!(true))
            }
            ["press", key] | ["keyboard", "press", key] => {
                let (key, code) = match *key {
                    "Enter" => ("Enter", 13),
                    "Tab" => ("Tab", 9),
                    "Escape" => ("Escape", 27),
                    "Backspace" => ("Backspace", 8),
                    "ArrowDown" => ("ArrowDown", 40),
                    "ArrowUp" => ("ArrowUp", 38),
                    "ArrowLeft" => ("ArrowLeft", 37),
                    "ArrowRight" => ("ArrowRight", 39),
                    "Space" => (" ", 32),
                    _ => return Err(denied()),
                };
                for event in ["keyDown", "keyUp"] {
                    page(
                        "Input.dispatchKeyEvent",
                        json!({"type":event,"key":key,"windowsVirtualKeyCode":code}),
                    )?;
                }
                Ok(json!(true))
            }
            ["keyboard", "type" | "inserttext", text] => {
                page("Input.insertText", json!({"text":text}))?;
                Ok(json!(true))
            }
            ["click", _] | ["fill" | "type", _, _] => {
                let selector = args[1];
                let backend = tab.refs.get(selector).copied();
                if (selector.starts_with('@') && backend.is_none()) || selector.len() > 512 {
                    return Err(denied());
                }
                let action = if args[0] == "click" { CLICK } else { FOCUS };
                let selector_value = if backend.is_some() {
                    Value::Null
                } else {
                    json!(selector)
                };
                let result = script(
                    &mut page,
                    action,
                    json!([{"value":selector_value},{"value":args[0]=="fill"}]),
                    backend,
                )?;
                if result["value"] != true {
                    return Err(failure("No matching share element"));
                }
                if let Some(text) = args.get(2) {
                    page("Input.insertText", json!({"text":text}))?;
                }
                Ok(json!(true))
            }
            _ => Err(denied()),
        }
    }
}

// Only these broker-owned functions run in the isolated world. User strings
// are CDP CallArgument values and never become JavaScript source.
const CLICK: &str = r#"function(selector) {
    const element = selector === null ? this : document.querySelector(selector);
    if (!element || element.matches('input[type=file]')) return false;
    element.click();
    return true;
}"#;

const FOCUS: &str = r#"function(selector, clear) {
    const element = selector === null ? this : document.querySelector(selector);
    if (!element || !(element.matches('input:not([type=file]),textarea') || element.isContentEditable)) return false;
    element.focus();
    if (clear) {
        if (element.isContentEditable) element.textContent = '';
        else element.select();
    }
    return true;
}"#;

fn denied() -> Failure {
    failure("Command or destination is outside this browser share")
}

fn script(
    page: &mut impl FnMut(&str, Value) -> Result<Value, Failure>,
    function: &str,
    args: Value,
    node: Option<i64>,
) -> Result<Value, Failure> {
    let frame = page("Page.getFrameTree", json!({}))?["frameTree"]["frame"]["id"].clone();
    let world = page(
        "Page.createIsolatedWorld",
        json!({"frameId":frame,"worldName":"vega-share"}),
    )?["executionContextId"]
        .clone();
    let mut params = json!({"functionDeclaration":function,"arguments":args,"returnByValue":true});
    if let Some(id) = node {
        params["objectId"] = page(
            "DOM.resolveNode",
            json!({"backendNodeId":id,"executionContextId":world}),
        )?["object"]["objectId"]
            .clone();
    } else {
        params["executionContextId"] = world;
    }
    let reply = page("Runtime.callFunctionOn", params)?;
    if reply.get("exceptionDetails").is_some() {
        return Err(failure("Share page action failed"));
    }
    Ok(reply["result"].clone())
}
