//! Upgrades (docs/design/upgrade.md): the install record, vegad's upstream
//! check (behind detection, the target pane, the proposal state) and the
//! steps of `vega upgrade`. Everything takes paths and a [`Probe`], so tests
//! run on fixture repos with fake binaries.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use serde_json::{json, Value};
use vega_config::infra::{self, Probe};

use crate::Block;

/// vegad fetches the checkout at most this often.
pub const FETCH_EVERY: i64 = 24 * 3600;
/// A `git fetch` that takes longer is killed.
pub const FETCH_TIMEOUT: Duration = Duration::from_secs(120);
/// Local git calls (rev-parse, rev-list, show, status).
const GIT_TIMEOUT: Duration = Duration::from_secs(10);
/// `moshi-hook doctor`.
const DOCTOR_TIMEOUT: Duration = Duration::from_secs(60);

/// `$XDG_STATE_HOME/vega/install.json`, written by scripts/install-cli.sh.
#[derive(Debug, Clone, PartialEq)]
pub struct Install {
    /// The main checkout the binaries were built from.
    pub checkout: PathBuf,
    /// The installed commit (full sha).
    pub commit: String,
    /// `local`, or `deploy` when `vega deploy` ran the install.
    pub by: String,
}

impl Install {
    pub fn deployed(&self) -> bool {
        self.by == "deploy"
    }

    pub fn short(&self) -> &str {
        &self.commit[..self.commit.len().min(7)]
    }
}

/// The install record under the vega state dir (`$XDG_STATE_HOME/vega`):
/// `None` when there is none, `Err` when it is unreadable.
pub fn read_install(state: &Path) -> Option<Result<Install, String>> {
    let p = state.join("install.json");
    let text = fs::read_to_string(&p).ok()?;
    let parsed = (|| {
        let v: Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
        let s = |k: &str| {
            v[k].as_str()
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .ok_or(format!("no {k}"))
        };
        Ok(Install {
            checkout: PathBuf::from(s("checkout")?),
            commit: s("commit")?,
            by: s("by").unwrap_or_else(|_| "local".into()),
        })
    })();
    Some(parsed.map_err(|e: String| format!("{}: {e}", p.display())))
}

fn git_cmd(dir: &Path) -> Command {
    let mut c = Command::new("git");
    c.arg("-C")
        .arg(dir)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("LC_ALL", "C");
    c
}

/// Runs git in `dir`; stdout trimmed, or why it failed.
fn git(dir: &Path, args: &[&str], timeout: Duration) -> Result<String, String> {
    let mut c = git_cmd(dir);
    c.args(args);
    let what = format!("git {}", args.join(" "));
    match vega_herdr::output_within(&mut c, timeout) {
        Ok(Some(o)) if o.status.success() => {
            Ok(String::from_utf8_lossy(&o.stdout).trim().to_string())
        }
        Ok(Some(o)) => Err(format!(
            "{what}: {}",
            String::from_utf8_lossy(&o.stderr)
                .lines()
                .last()
                .unwrap_or("failed")
                .trim()
        )),
        Ok(None) => Err(format!("{what}: timed out after {} s", timeout.as_secs())),
        Err(e) => Err(format!("{what}: {e}")),
    }
}

pub fn fetch(checkout: &Path) -> Result<(), String> {
    git(checkout, &["fetch", "--quiet"], FETCH_TIMEOUT).map(|_| ())
}

/// The checkout against its upstream, from local refs (fetch first).
#[derive(Debug, Clone, PartialEq)]
pub struct Upstream {
    /// `origin/main`.
    pub name: String,
    pub sha: String,
    /// Commits of the upstream the installed commit lacks.
    pub behind: u32,
    /// Commits of HEAD the upstream lacks.
    pub ahead: u32,
    /// Commits of the upstream HEAD lacks (what a pull brings).
    pub pull: u32,
}

fn count(dir: &Path, range: &str) -> Result<u32, String> {
    git(dir, &["rev-list", "--count", range], GIT_TIMEOUT)?
        .parse()
        .map_err(|e| format!("rev-list {range}: {e}"))
}

/// The installed commit if the checkout knows it, else HEAD.
fn base(checkout: &Path, installed: &str) -> String {
    let probe = format!("{installed}^{{commit}}");
    match git(
        checkout,
        &["rev-parse", "-q", "--verify", &probe],
        GIT_TIMEOUT,
    ) {
        Ok(_) => installed.to_string(),
        Err(_) => "HEAD".into(),
    }
}

pub fn upstream(checkout: &Path, installed: &str) -> Result<Upstream, String> {
    let name = git(
        checkout,
        &["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}"],
        GIT_TIMEOUT,
    )
    .map_err(|_| "the checkout's branch has no upstream".to_string())?;
    let sha = git(checkout, &["rev-parse", "@{u}"], GIT_TIMEOUT)?;
    let base = base(checkout, installed);
    Ok(Upstream {
        behind: count(checkout, &format!("{base}..{sha}"))?,
        ahead: count(checkout, &format!("{sha}..HEAD"))?,
        pull: count(checkout, &format!("HEAD..{sha}"))?,
        name,
        sha,
    })
}

/// Commits of `sha` the installed commit lacks, from local refs only.
pub fn behind_of(checkout: &Path, installed: &str, sha: &str) -> Result<u32, String> {
    count(checkout, &format!("{}..{sha}", base(checkout, installed)))
}

/// The changelog blocks of `rev` newer than block `newer_than`.
pub fn blocks_at(checkout: &Path, rev: &str, newer_than: u32) -> Vec<Block> {
    git(
        checkout,
        &["show", &format!("{rev}:CHANGELOG.md")],
        GIT_TIMEOUT,
    )
    .ok()
    .and_then(|t| crate::parse(&t).ok())
    .unwrap_or_default()
    .into_iter()
    .filter(|b| b.n > newer_than)
    .collect()
}

/// Tracked files differ from HEAD (untracked ones do not block a pull).
pub fn dirty(checkout: &Path) -> Result<bool, String> {
    git(
        checkout,
        &["status", "--porcelain", "--untracked-files=no"],
        GIT_TIMEOUT,
    )
    .map(|s| !s.is_empty())
}

/// vegad's memory of the check: `$XDG_STATE_HOME/vega/upgrade.json`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Check {
    /// Unix time of the last fetch attempt.
    pub fetched_at: i64,
    pub upstream: Option<String>,
    pub upstream_name: String,
    pub behind: u32,
    /// `<n> · <date> <kind>: <text>` per new changelog entry.
    pub changes: Vec<String>,
    /// The upstream sha a session was told about.
    pub proposed: Option<String>,
}

impl Check {
    pub fn load(state: &Path) -> Check {
        let v: Value = fs::read_to_string(state.join("upgrade.json"))
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or(Value::Null);
        let s = |k: &str| v[k].as_str().map(str::to_string);
        Check {
            fetched_at: v["fetched_at"].as_i64().unwrap_or(0),
            upstream: s("upstream"),
            upstream_name: s("upstream_name").unwrap_or_default(),
            behind: v["behind"].as_u64().unwrap_or(0) as u32,
            changes: v["changes"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|x| x.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default(),
            proposed: s("proposed"),
        }
    }

    pub fn save(&self, state: &Path) -> Result<(), String> {
        fs::create_dir_all(state).map_err(|e| format!("{}: {e}", state.display()))?;
        let v = json!({
            "fetched_at": self.fetched_at,
            "upstream": self.upstream,
            "upstream_name": self.upstream_name,
            "behind": self.behind,
            "changes": self.changes,
            "proposed": self.proposed,
        });
        let p = state.join("upgrade.json");
        fs::write(
            &p,
            serde_json::to_string_pretty(&v).expect("serializes") + "\n",
        )
        .map_err(|e| format!("{}: {e}", p.display()))
    }

    pub fn fetch_due(&self, now: i64) -> bool {
        now - self.fetched_at >= FETCH_EVERY
    }

    /// Behind, and this upstream state was not proposed yet.
    pub fn pending(&self) -> bool {
        self.behind > 0 && self.upstream.is_some() && self.upstream != self.proposed
    }

    /// After a fetch: the new upstream state and its changelog entries.
    pub fn observe(&mut self, now: i64, up: &Upstream, blocks: &[Block]) {
        self.fetched_at = now;
        self.upstream = Some(up.sha.clone());
        self.upstream_name = up.name.clone();
        self.behind = up.behind;
        self.changes = blocks
            .iter()
            .flat_map(|b| {
                b.entries
                    .iter()
                    .map(move |e| format!("{} · {} {}: {}", b.n, b.date, e.kind, e.text))
            })
            .collect();
    }

    /// The proposal mail for the vega session.
    pub fn mail(&self, installed: &Install) -> String {
        let sha = self.upstream.as_deref().unwrap_or("");
        let mut m = format!(
            "vega upgrade available: {} commit{} upstream of the installed {} ({} is {}).\n",
            self.behind,
            if self.behind == 1 { "" } else { "s" },
            installed.short(),
            self.upstream_name,
            &sha[..sha.len().min(7)],
        );
        // The instruction before the changelog: local mode types one line and
        // cuts a long one at the end.
        m.push_str(
            "Ask the user whether to upgrade now. On yes: run vega upgrade (refuses a dirty or \
             diverged checkout), summarise the changes for the user, act on migration entries, and \
             run vega onboard apply on their OK.\n",
        );
        if self.changes.is_empty() {
            m.push_str("Changelog: no new entries.");
        } else {
            m.push_str("Changelog:");
            for c in &self.changes {
                m.push_str(&format!("\n  {c}"));
            }
        }
        m
    }

    /// The `$upgrade` workspace token while no session got the mail.
    pub fn token(&self) -> String {
        format!("vega \u{2193}{}", self.behind)
    }
}

/// An agent pane on this host, from `herdr pane list`.
#[derive(Debug, Clone, PartialEq)]
pub struct Pane {
    pub id: String,
    pub workspace: String,
    pub cwd: PathBuf,
    pub agent: bool,
}

/// The panes of a `herdr pane list` reply.
pub fn panes(list: &Value) -> Vec<Pane> {
    list["result"]["panes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|p| {
            Some(Pane {
                id: p["pane_id"].as_str()?.to_string(),
                workspace: p["workspace_id"].as_str().unwrap_or("").to_string(),
                cwd: PathBuf::from(p["cwd"].as_str()?),
                agent: p["agent"].as_str().is_some_and(|a| !a.is_empty()),
            })
        })
        .collect()
}

fn real(p: &Path) -> PathBuf {
    fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
}

/// `cwd` is the checkout or below it, by path components.
pub fn inside(checkout: &Path, cwd: &Path) -> bool {
    real(cwd).starts_with(real(checkout))
}

/// The pane that hears about an upgrade: an agent pane in the checkout; of
/// several, a UAC pane (`uac`), else the lowest pane id.
pub fn choose<'a>(panes: &'a [Pane], checkout: &Path, uac: &[String]) -> Option<&'a Pane> {
    let mut c: Vec<&Pane> = panes
        .iter()
        .filter(|p| p.agent && inside(checkout, &p.cwd))
        .collect();
    c.sort_by(|a, b| a.id.cmp(&b.id));
    c.iter()
        .find(|p| uac.contains(&p.id))
        .or(c.first())
        .copied()
}

/// The UAC panes of the checkout's project, from `vega uac claim` records
/// (full project ID, pane, time). Delivery still stays inside the upgraded checkout.
pub fn uac_panes(
    records: &[(String, String, i64)],
    checkout: &Path,
) -> Result<Vec<String>, vega_herdr::Failure> {
    let project = vega_herdr::project::resolve(checkout, &vega_herdr::state::post_host())?;
    Ok(records
        .iter()
        .filter(|(key, _, _)| key == project.id.as_str())
        .map(|(_, pane, _)| pane.clone())
        .collect())
}

/// The workspaces holding a pane in the checkout, for the `$upgrade` token.
pub fn workspaces(panes: &[Pane], checkout: &Path) -> Vec<String> {
    let mut w: Vec<String> = panes
        .iter()
        .filter(|p| !p.workspace.is_empty() && inside(checkout, &p.cwd))
        .map(|p| p.workspace.clone())
        .collect();
    w.sort();
    w.dedup();
    w
}

/// Why `vega upgrade` stops before changing anything.
#[derive(Debug, Clone, PartialEq)]
pub struct Refusal {
    pub code: &'static str,
    pub message: String,
    pub hint: String,
}

fn refuse(code: &'static str, message: impl Into<String>, hint: impl Into<String>) -> Refusal {
    Refusal {
        code,
        message: message.into(),
        hint: hint.into(),
    }
}

/// One step of `vega upgrade` and its result.
#[derive(Debug, Clone, PartialEq)]
pub struct Step {
    pub step: &'static str,
    pub result: String,
    pub ok: bool,
}

fn step(step: &'static str, ok: bool, result: impl Into<String>) -> Step {
    Step {
        step,
        result: result.into(),
        ok,
    }
}

/// What `vega upgrade` did.
#[derive(Debug, Default)]
pub struct Report {
    pub checkout: PathBuf,
    pub steps: Vec<Step>,
    /// `moshi-hook doctor`: exit code and output.
    pub doctor: Option<(i32, String)>,
}

impl Report {
    pub fn failed(&self) -> bool {
        self.steps.iter().any(|s| !s.ok)
    }
}

/// Last `n` lines of a log file, joined with `; `.
fn tail(path: &Path, n: usize) -> String {
    let t = fs::read_to_string(path).unwrap_or_default();
    let lines: Vec<&str> = t.lines().filter(|l| !l.trim().is_empty()).collect();
    lines[lines.len().saturating_sub(n)..].join("; ")
}

/// Runs `cmd` with stdout and stderr appended to `log`.
fn logged(cmd: &mut Command, log: &Path) -> Result<bool, String> {
    let open = || {
        fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(log)
            .map_err(|e| format!("{}: {e}", log.display()))
    };
    cmd.stdin(Stdio::null())
        .stdout(open()?)
        .stderr(open()?)
        .status()
        .map(|s| s.success())
        .map_err(|e| e.to_string())
}

/// `vega upgrade` up to the changes view (docs/design/upgrade.md): checks
/// the checkout, pulls, installs, upgrades moshi-hook. `state` is the vega
/// state dir; its `upgrade.log` takes the install output.
pub fn run(state: &Path, probe: &Probe) -> Result<Report, Refusal> {
    let install = match read_install(state) {
        None => return Err(refuse(
            "no_install_record",
            "this host has no install record (installed before vega upgrade existed, or by hand)",
            "run `mise run install-cli` once in your vega checkout",
        )),
        Some(Err(e)) => {
            return Err(refuse(
                "install_record_invalid",
                e,
                "rerun `mise run install-cli` in your vega checkout",
            ))
        }
        Some(Ok(i)) => i,
    };
    if install.deployed() {
        return Err(refuse(
            "deploy_target",
            "this host gets vega by `vega deploy` from another host",
            "upgrade the deploying host, then run `vega deploy apply` there",
        ));
    }
    let checkout = install.checkout.clone();
    match dirty(&checkout) {
        Ok(false) => {}
        Ok(true) => {
            return Err(refuse(
                "dirty",
                format!("{} has uncommitted changes", checkout.display()),
                "commit or stash them in the checkout, then rerun",
            ))
        }
        Err(e) => {
            return Err(refuse(
                "no_checkout",
                e,
                "check the checkout in $XDG_STATE_HOME/vega/install.json",
            ))
        }
    }
    fetch(&checkout).map_err(|e| {
        refuse(
            "fetch_failed",
            e,
            "check the network and git remote access, then rerun",
        )
    })?;
    let up = upstream(&checkout, &install.commit).map_err(|e| {
        refuse(
            "no_upstream",
            e,
            "set one: git -C <checkout> branch --set-upstream-to origin/main",
        )
    })?;
    if up.ahead > 0 && up.pull > 0 {
        return Err(refuse(
            "diverged",
            format!(
                "the checkout has {} commit(s) {} lacks and lacks {}",
                up.ahead, up.name, up.pull
            ),
            "rebase or merge in the checkout yourself, then rerun",
        ));
    }
    let mut report = Report {
        checkout: checkout.clone(),
        ..Default::default()
    };
    if up.pull > 0 {
        match git(&checkout, &["pull", "--ff-only", "--quiet"], FETCH_TIMEOUT) {
            Ok(_) => report.steps.push(step(
                "pull",
                true,
                format!("{} commit(s) from {}", up.pull, up.name),
            )),
            Err(e) => {
                report.steps.push(step("pull", false, e));
                return Ok(report);
            }
        }
    } else {
        report
            .steps
            .push(step("pull", true, format!("level with {}", up.name)));
    }
    let head = git(&checkout, &["rev-parse", "HEAD"], GIT_TIMEOUT).unwrap_or_default();
    if head == install.commit {
        report.steps.push(step(
            "install",
            true,
            format!("current ({} installed)", install.short()),
        ));
    } else {
        report.steps.push(install_cli(state, probe, &checkout));
        if !report.steps.last().is_some_and(|s| s.ok) {
            return Ok(report);
        }
    }
    moshi(probe, &mut report);
    Ok(report)
}

fn install_cli(state: &Path, probe: &Probe, checkout: &Path) -> Step {
    let Some(mise) = probe.find("mise") else {
        return step(
            "install",
            false,
            "mise missing: https://mise.jdx.dev/installing-mise.html",
        );
    };
    let log = state.join("upgrade.log");
    if let Err(e) = fs::create_dir_all(state) {
        return step("install", false, format!("{}: {e}", state.display()));
    }
    let mut c = Command::new(mise);
    c.args(["run", "install-cli"]).current_dir(checkout);
    let shown = vega_herdr::collapse_home(&log.display().to_string());
    match logged(&mut c, &log) {
        Ok(true) => step(
            "install",
            true,
            format!("mise run install-cli (log {shown})"),
        ),
        Ok(false) => step(
            "install",
            false,
            format!(
                "mise run install-cli failed: {} (log {shown})",
                tail(&log, 5)
            ),
        ),
        Err(e) => step("install", false, format!("mise run install-cli: {e}")),
    }
}

/// brew upgrade, service restart and doctor where moshi-hook is installed.
fn moshi(probe: &Probe, report: &mut Report) {
    let Some(m) = infra::moshi(probe) else {
        report
            .steps
            .push(step("moshi-hook", true, "not installed: skipped"));
        return;
    };
    if m.brew {
        match probe.find("brew") {
            None => report.steps.push(step(
                "moshi-hook upgrade",
                false,
                "installed by Homebrew, but brew is not on PATH",
            )),
            Some(brew) => {
                let r = brew_run(&brew, &["upgrade", "moshi-hook"]);
                report.steps.push(match r {
                    Ok(out) => step("moshi-hook upgrade", true, out),
                    Err(e) => step("moshi-hook upgrade", false, e),
                });
                if m.service.is_some() {
                    let r = brew_run(&brew, &["services", "restart", "moshi-hook"]);
                    report.steps.push(match r {
                        Ok(_) => step("moshi-hook service", true, "restarted"),
                        Err(e) => step("moshi-hook service", false, e),
                    });
                } else {
                    report.steps.push(step(
                        "moshi-hook service",
                        true,
                        "not registered: brew services start moshi-hook starts it",
                    ));
                }
            }
        }
    } else {
        report.steps.push(step(
            "moshi-hook upgrade",
            true,
            "not from Homebrew: skipped (moshi-hook update upgrades it)",
        ));
    }
    // stdin is /dev/null: doctor asks before fixing, no answer fixes nothing.
    let mut c = Command::new(&m.bin);
    c.arg("doctor");
    match vega_herdr::output_within(&mut c, DOCTOR_TIMEOUT) {
        Ok(Some(o)) => {
            let code = o.status.code().unwrap_or(-1);
            let mut text = String::from_utf8_lossy(&o.stdout).into_owned();
            text.push_str(&String::from_utf8_lossy(&o.stderr));
            report
                .steps
                .push(step("moshi-hook doctor", code == 0, format!("exit {code}")));
            report.doctor = Some((code, text));
        }
        Ok(None) => report.steps.push(step(
            "moshi-hook doctor",
            false,
            format!("timed out after {} s", DOCTOR_TIMEOUT.as_secs()),
        )),
        Err(e) => report
            .steps
            .push(step("moshi-hook doctor", false, e.to_string())),
    }
}

/// A brew call: its last output line, or why it failed.
fn brew_run(brew: &Path, args: &[&str]) -> Result<String, String> {
    let what = format!("brew {}", args.join(" "));
    let out = Command::new(brew)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("{what}: {e}"))?;
    let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    let last = text
        .lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("")
        .trim()
        .to_string();
    if out.status.success() {
        Ok(if last.is_empty() { "ok".into() } else { last })
    } else {
        Err(format!("{what} failed: {last}"))
    }
}

#[cfg(test)]
#[path = "upgrade_tests.rs"]
mod tests;
