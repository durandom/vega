//! vegad's upstream check (docs/design/upgrade.md): is the vega checkout
//! this host was installed from behind its upstream? A fetch at most once per
//! 24 h, killed after 120 s, and herdr calls: all on this thread, never on the
//! relay loop. Proposes once per upstream state, by mail to one agent pane in
//! the checkout (the UAC first), else as a `$upgrade` workspace token.

use std::path::PathBuf;
use std::thread;
use std::time::Duration;

use tokio::sync::mpsc::UnboundedSender;
use vega_onboard::upgrade::{self, Check, Install};

/// How often the thread looks; the fetch has its own 24 h clock.
const EVERY: Duration = Duration::from_secs(60);

/// The token outlives two missed wakes, then herdr drops it.
const TOKEN_TTL_MS: &str = "180000";

/// The `Vega-From-Agent` of upgrade mail: a vega tool, no reply path.
pub const AGENT: &str = "tool vega-upgrade";

pub enum Event {
    /// (pane, text) for the relay loop to publish.
    Mail(String, String),
    Log(String),
}

pub fn spawn(tx: UnboundedSender<Event>, state: PathBuf) {
    thread::spawn(move || {
        let mut w = Watch::default();
        loop {
            for e in w.tick(&state, vega_bd_sync::unix_now()) {
                if tx.send(e).is_err() {
                    return;
                }
            }
            thread::sleep(EVERY);
        }
    });
}

#[derive(Default)]
struct Watch {
    /// The last state line logged: logged again only when it changes.
    said: String,
    /// Workspaces that carry the `$upgrade` token.
    tokens: Vec<String>,
}

impl Watch {
    fn say(&mut self, out: &mut Vec<Event>, line: String) {
        if line != self.said {
            out.push(Event::Log(line.clone()));
            self.said = line;
        }
    }

    fn tick(&mut self, state: &std::path::Path, now: i64) -> Vec<Event> {
        let mut out = Vec::new();
        let install = match upgrade::read_install(state) {
            None => {
                self.say(
                    &mut out,
                    "upgrade check: off (no install record; mise run install-cli writes it)".into(),
                );
                self.set_tokens(&[], "");
                return out;
            }
            Some(Err(e)) => {
                self.say(&mut out, format!("upgrade check: off ({e})"));
                self.set_tokens(&[], "");
                return out;
            }
            Some(Ok(i)) if i.deployed() => {
                self.say(&mut out, "upgrade check: off (vega deploy target)".into());
                self.set_tokens(&[], "");
                return out;
            }
            Some(Ok(i)) => i,
        };
        self.say(
            &mut out,
            format!("upgrade check: on ({})", install.checkout.display()),
        );
        let mut c = Check::load(state);
        let before = c.clone();
        if c.fetch_due(now) {
            self.fetch(&install, &mut c, now, &mut out);
        } else if c.pending() {
            // Installed by other means since: no longer behind.
            let sha = c.upstream.clone().unwrap_or_default();
            if let Ok(n) = upgrade::behind_of(&install.checkout, &install.commit, &sha) {
                c.behind = n;
            }
        }
        if !c.pending() {
            self.set_tokens(&[], "");
            save(&before, &c, state, &mut out);
            return out;
        }
        let panes = match vega_herdr::herdr(&["pane", "list"]) {
            Ok(v) => upgrade::panes(&v),
            Err(f) => {
                out.push(Event::Log(format!(
                    "upgrade check: herdr pane list: {}",
                    f.message
                )));
                save(&before, &c, state, &mut out);
                return out;
            }
        };
        let uac = match upgrade::uac_panes(
            &vega_herdr::state::project_uac_records(),
            &install.checkout,
        ) {
            Ok(panes) => {
                vega_herdr::project::diagnostic("upgrade", &install.checkout, None);
                panes
            }
            Err(e) => {
                if let Some(message) =
                    vega_herdr::project::diagnostic("upgrade", &install.checkout, Some(&e))
                {
                    out.push(Event::Log(format!("upgrade project: {message}")));
                }
                save(&before, &c, state, &mut out);
                return out;
            }
        };
        match upgrade::choose(&panes, &install.checkout, &uac) {
            Some(p) => {
                out.push(Event::Log(format!(
                    "upgrade check: proposed to {} ({} behind)",
                    p.id, c.behind
                )));
                out.push(Event::Mail(p.id.clone(), c.mail(&install)));
                c.proposed = c.upstream.clone();
                self.set_tokens(&[], "");
            }
            None => {
                let ws = upgrade::workspaces(&panes, &install.checkout);
                self.set_tokens(&ws, &c.token());
            }
        }
        save(&before, &c, state, &mut out);
        out
    }

    fn fetch(&mut self, install: &Install, c: &mut Check, now: i64, out: &mut Vec<Event>) {
        // The attempt counts even if it fails: no retry storm.
        c.fetched_at = now;
        let checkout = &install.checkout;
        let up =
            upgrade::fetch(checkout).and_then(|_| upgrade::upstream(checkout, &install.commit));
        match up {
            Ok(up) => {
                let embedded = vega_onboard::parse(vega_onboard::CHANGELOG)
                    .map(|b| vega_onboard::newest(&b))
                    .unwrap_or(0);
                let blocks = upgrade::blocks_at(checkout, &up.sha, embedded);
                c.observe(now, &up, &blocks);
                out.push(Event::Log(format!(
                    "upgrade check: fetched, {} at {}, {} behind",
                    up.name,
                    &up.sha[..up.sha.len().min(7)],
                    up.behind
                )));
            }
            Err(e) => out.push(Event::Log(format!("upgrade check: {e}; next try in 24 h"))),
        }
    }

    /// Puts `token` on `ws` and clears it from the workspaces that had it.
    fn set_tokens(&mut self, ws: &[String], token: &str) {
        for w in self.tokens.iter().filter(|w| !ws.contains(w)) {
            let _ = vega_herdr::herdr(&[
                "workspace",
                "report-metadata",
                w,
                "--source",
                "vega:upgrade",
                "--clear-token",
                "upgrade",
            ]);
        }
        let tok = format!("upgrade={token}");
        for w in ws {
            let _ = vega_herdr::herdr(&[
                "workspace",
                "report-metadata",
                w,
                "--source",
                "vega:upgrade",
                "--ttl-ms",
                TOKEN_TTL_MS,
                "--token",
                &tok,
            ]);
        }
        self.tokens = ws.to_vec();
    }
}

/// Writes the check when it changed.
fn save(before: &Check, c: &Check, state: &std::path::Path, out: &mut Vec<Event>) {
    if c == before {
        return;
    }
    if let Err(e) = c.save(state) {
        out.push(Event::Log(format!("upgrade check: cannot save state: {e}")));
    }
}
