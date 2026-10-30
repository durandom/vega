//! Opt-in real-browser regression; no global browser or profile is used.
use super::*;
use std::sync::atomic::{AtomicBool, Ordering};

struct Server {
    port: u16,
    running: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}

impl Server {
    fn new(mut respond: impl FnMut(&mut TcpStream) + Send + 'static) -> Self {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        listener.set_nonblocking(true).unwrap();
        let running = Arc::new(AtomicBool::new(true));
        let live = running.clone();
        let thread = thread::spawn(move || {
            while live.load(Ordering::Relaxed) {
                if let Ok((mut stream, _)) = listener.accept() {
                    stream
                        .set_read_timeout(Some(Duration::from_secs(2)))
                        .unwrap();
                    respond(&mut stream);
                } else {
                    thread::sleep(Duration::from_millis(10));
                }
            }
        });
        Self {
            port,
            running,
            thread: Some(thread),
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Relaxed);
        self.thread.take().unwrap().join().unwrap();
    }
}

struct Browser(String);
impl Browser {
    fn run(&self, args: &[&str]) -> String {
        agent_browser(&self.0, args).unwrap()
    }
    fn click(&self, selector: &str) {
        let coords = self.run(&["eval", &format!(
            "(() => {{ const r = document.querySelector('vega-review').shadowRoot.querySelector({}).getBoundingClientRect(); return [r.x+r.width/2,r.y+r.height/2]; }})()",
            json!(selector)
        )]);
        let coords: Vec<f64> = serde_json::from_str(&coords).unwrap();
        self.run(&[
            "mouse",
            "move",
            &coords[0].round().to_string(),
            &coords[1].round().to_string(),
        ]);
        self.run(&["mouse", "down"]);
        self.run(&["mouse", "up"]);
    }
}
impl Drop for Browser {
    fn drop(&mut self) {
        let _ = agent_browser(&self.0, &["close"]);
    }
}

#[test]
#[ignore = "requires agent-browser and its Chromium; run with --ignored"]
fn strict_csp_notes_survive_reload_and_navigation() {
    let dir = tempfile::tempdir().unwrap();
    let app = Server::new(|stream| {
        let _ = read_request(stream);
        let html = "<!doctype html><title>Strict CSP</title><h1>Review me</h1>";
        write!(stream, "HTTP/1.1 200 OK\r\nContent-Security-Policy: default-src 'self'; connect-src 'self'\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{html}", html.len()).unwrap();
    });
    let origin = format!("http://127.0.0.1:{}", app.port);
    let secret = "csp-test-secret";
    let state = Arc::new(Mutex::new(Inbox::new(secret, &origin)));
    let inbox_state = state.clone();
    let inbox = Server::new(move |stream| {
        if let Some(req) = read_request(stream) {
            let response = inbox_state.lock().unwrap().handle(&req, Instant::now());
            write_response(stream, &response);
        }
    });
    let cfg = json!({"blank": format!("about:blank#vega-csp-test-{}", random_hex(8).unwrap()), "port": inbox.port, "secret": secret});
    let script = dir.path().join("overlay.js");
    write_private(&script, &OVERLAY.replace(CFG_PLACEHOLDER, &json!({
        "origin": origin, "secret": secret, "endpoint": format!("http://127.0.0.1:{}", inbox.port),
    }).to_string())).unwrap();
    let browser = Browser(format!("vega-csp-test-{}", random_hex(8).unwrap()));
    let path = dir.path().to_path_buf();
    let blank = cfg["blank"].as_str().unwrap().to_string();
    let attachment = thread::spawn(move || csp::listen(&path, &blank).unwrap());
    open_review(
        &browser.0,
        script.to_str().unwrap(),
        &origin,
        true,
        dir.path(),
        &cfg,
    )
    .unwrap();
    // Like serve, keep the attachment alive for the full review lifetime.
    let _csp = attachment.join().unwrap();
    for (index, action) in ["initial", "reload", "navigate", "return"]
        .iter()
        .enumerate()
    {
        match *action {
            "reload" => {
                browser.run(&["reload"]);
            }
            "navigate" => {
                browser.run(&["open", &format!("{origin}/second")]);
            }
            "return" => {
                // Force a renderer/site change, then return to the reviewed origin.
                browser.run(&["open", &format!("http://localhost:{}/outside", app.port)]);
                assert_eq!(
                    browser.run(&["eval", "typeof window.__vegaReview"]),
                    "\"undefined\""
                );
                browser.run(&["open", &origin]);
            }
            _ => {}
        }
        browser.run(&[
            "wait",
            "--fn",
            "!!document.querySelector('vega-review')?.shadowRoot.querySelector('.page-note')",
        ]);
        browser.click(".page-note");
        browser.run(&["keyboard", "inserttext", action]);
        browser.run(&["press", "Enter"]);
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let notes = cli_get(&cfg, "/notes").unwrap();
            if notes["notes"].as_array().unwrap().len() == index + 1 {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "{action}: note failed to reach the inbox"
            );
            thread::sleep(Duration::from_millis(50));
        }
        browser.run(&["wait", "--fn", "document.querySelector('vega-review').shadowRoot.querySelector('.row .primary').textContent === 'Send 1 note(s)'"]);
        browser.click(".row .primary");
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let notes = cli_get(&cfg, "/notes").unwrap();
            if notes["notes"][index]["status"] == "queued" {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "{action}: Send failed to queue the note: {notes}; overlay: {}",
                browser.run(&[
                    "eval",
                    "document.querySelector('vega-review').shadowRoot.textContent"
                ])
            );
            thread::sleep(Duration::from_millis(50));
        }
    }
    let batch = state
        .lock()
        .unwrap()
        .take_batch(Instant::now() + Duration::from_secs(5))
        .unwrap();
    assert_eq!(batch.len(), 4);
}
