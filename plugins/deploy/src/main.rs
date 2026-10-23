//! `vega deploy`: build and install vega on other machines.
//!
//! The machine inventory is herdr's (`herdr machine list --json`): each saved
//! machine has an SSH target. vega builds on the target, so every architecture
//! works without cross-compiling: ssh in, fetch and fast-forward a pushed branch
//! (`main` unless `--from-branch` selects another), run
//! `scripts/install-cli.sh`, then copy this host's local config overrides and
//! run `vega config apply` (docs/design/config.md; never fatal). Only
//! committed and pushed code gets deployed.
//!
//! Replies from other machines go through the broker (`vega post`), so the
//! target needs no ssh access or herdr profile back to this machine. The
//! status shows whether its vegad (`vega post relay`) runs the installed binary.

mod overrides;

use std::env;
use std::io::Write;
use std::process::{Command, ExitCode, Stdio};

use clap::error::ErrorKind;
use clap::{CommandFactory, Parser, Subcommand};
use serde_json::Value;

const REPO_URL: &str = "git@github.com:durandom/vega.git";
/// Branch `apply` deploys unless `--from-branch` says otherwise; also what the status reports.
const BRANCH: &str = "main";
/// Checkout location on every machine, relative to `$HOME`.
const REMOTE_DIR: &str = "src/durandom/vega";

#[derive(Parser)]
#[command(
    name = "vega deploy",
    bin_name = "vega deploy",
    disable_version_flag = true,
    about = "Build and install vega on herdr machines (builds on the target)",
    after_help = "Examples:\n  vega deploy                  # status of every herdr machine (read-only)\n  vega deploy apply ganymede   # pull, build, install\n  vega deploy apply --all\n  vega deploy apply ganymede --from-branch my-fix"
)]
struct Cli {
    /// Print the agent guide for this plugin, SKILL.md form (read by `vega skills`)
    #[arg(long)]
    skill: bool,
    /// Print the bare version
    #[arg(short = 'V', short_alias = 'v', long)]
    version: bool,
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Pull, build and install vega on machines
    #[command(
        after_help = "Then copies this host's ~/.config/vega/config/ to the target (replacing the target's; column overrides: the file count) and runs `vega config apply` there (column config: its summary). Neither fails the deploy.\nA running vegad execs the new binary by itself (install-cli restarts one that has not after 15 s); a stopped one is started. `relay` reports current, stale or stopped.\n\nExamples:\n  vega deploy apply ganymede\n  vega deploy apply --all\n  vega deploy apply ganymede --from-branch my-fix"
    )]
    Apply {
        /// herdr machine labels (see `vega deploy`)
        machines: Vec<String>,
        /// Deploy to every enabled herdr machine
        #[arg(long, conflicts_with = "machines")]
        all: bool,
        /// Pushed branch to build and install from
        #[arg(long, value_name = "BRANCH", default_value = BRANCH, value_parser = branch_name)]
        from_branch: String,
    },
}

const SKILL: &str = include_str!("../SKILL.md");

struct Failure {
    code: String,
    message: String,
    hint: String,
}

impl Failure {
    fn new(code: &str, message: impl Into<String>, hint: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            hint: hint.into(),
        }
    }
}

fn main() -> ExitCode {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(e) if matches!(e.kind(), ErrorKind::DisplayHelp) => {
            let _ = e.print();
            return ExitCode::SUCCESS;
        }
        Err(e) => {
            let msg = e.to_string();
            let message = msg
                .lines()
                .take_while(|l| !l.trim().is_empty())
                .map(str::trim)
                .collect::<Vec<_>>()
                .join(" ");
            print_failure(&Failure::new(
                "usage",
                message.trim_start_matches("error: "),
                "valid usage below",
            ));
            let mut cmd = Cli::command();
            cmd.build();
            let sub = env::args().nth(1).filter(|a| !a.starts_with('-'));
            let help = match sub.as_deref().and_then(|s| cmd.find_subcommand_mut(s)) {
                Some(sc) => sc.render_help(),
                None => cmd.render_help(),
            };
            print!("{help}");
            return ExitCode::from(2);
        }
    };
    if cli.version {
        println!("{}", vega_build::version_line!());
        return ExitCode::SUCCESS;
    }
    if cli.skill {
        print!("{SKILL}");
        return ExitCode::SUCCESS;
    }
    let result = match cli.cmd {
        None => status(),
        Some(Cmd::Apply {
            machines,
            all,
            from_branch,
        }) => apply(&machines, all, &from_branch),
    };
    match result {
        Ok(code) => code,
        Err(f) => {
            print_failure(&f);
            ExitCode::from(if f.code == "usage" { 2 } else { 1 })
        }
    }
}

fn print_failure(f: &Failure) {
    println!("error{{code,message,hint}}:");
    println!("  {},{},{}", f.code, cell(&f.message), cell(&f.hint));
}

/// A saved herdr machine.
struct Machine {
    label: String,
    target: String,
}

fn machines() -> Result<Vec<Machine>, Failure> {
    let out = Command::new("herdr")
        .args(["machine", "list", "--json"])
        .output()
        .map_err(|e| Failure::new("herdr_not_found", e.to_string(), "install herdr"))?;
    let list: Value = serde_json::from_slice(&out.stdout).map_err(|_| {
        Failure::new(
            "herdr_bad_output",
            "herdr machine list returned non-JSON output",
            "check `herdr --version`",
        )
    })?;
    Ok(list
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter(|m| m["enabled"].as_bool().unwrap_or(false))
        .map(|m| Machine {
            label: m["label"].as_str().unwrap_or("-").into(),
            target: m["target"].as_str().unwrap_or("-").into(),
        })
        .collect())
}

/// The commit `apply` would deploy: the pushed `main` on GitHub.
fn source_commit() -> String {
    Command::new("git")
        .args(["ls-remote", REPO_URL, &format!("refs/heads/{BRANCH}")])
        .output()
        .ok()
        .and_then(|o| {
            String::from_utf8_lossy(&o.stdout)
                .split_whitespace()
                .next()
                .map(|s| s.chars().take(7).collect())
        })
        .unwrap_or_else(|| "unknown".into())
}

fn local_host() -> String {
    Command::new("hostname")
        .arg("-s")
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}

/// Puts vega's own tool dirs on PATH, whatever the target's dotfiles do: our
/// binaries, rustup's cargo proxy (`~/.cargo/bin`, or Homebrew's keg-only rustup).
const PATH_SETUP: &str = r#"for d in /opt/homebrew/opt/rustup/bin "$HOME/.cargo/bin" "$HOME/.local/bin"; do
  [ -d "$d" ] && PATH="$d:$PATH"
done
export PATH"#;

/// Shell function `relay_state`: vegad on the target is `current` (runs the
/// installed binary), `stale` (runs an older one: a re-exec is pending, or it
/// predates self-exec) or `stopped`. A vega from before vega-lhc.54 prints
/// its `service:` state instead.
const RELAY_STATE: &str = r#"relay_state() {
  s=$(vega post relay status 2>/dev/null || true)
  case "$s" in
    *"vegad: stopped"*) echo stopped ;;
    *"binary current"*) echo current ;;
    *"vegad: running"*) echo stale ;;
    *) echo "$s" | sed -n 's/^service: \([a-z-]*\).*/\1/p' | head -1 ;;
  esac
}"#;

/// Shell function `relay_ensure`: waits up to 10 s for vegad to run the
/// installed binary (a running vegad execs it by itself, else install-cli
/// restarted it; macOS checks a new binary on first launch) and starts a
/// stopped one once (vega-lhc.142). Prints the last `relay_state`. Needs
/// `relay_state` and PATH_SETUP: `relay start` passes PATH on to vegad.
const RELAY_ENSURE: &str = r#"relay_ensure() {
  started=
  for _ in 1 2 3 4 5 6 7 8 9 10; do
    r=$(relay_state)
    [ "$r" = current ] && break
    if [ "$r" = stopped ] && [ -z "$started" ]; then
      started=1
      vega post relay start </dev/null >/dev/null 2>&1 || true
      continue
    fi
    sleep 1
  done
  echo "$r"
}"#;

/// Shell snippet that reports the target's state as `vega-deploy:key=value` lines.
fn probe_script() -> String {
    format!(
        r#"
{PATH_SETUP}
{RELAY_STATE}
dir="$HOME/{REMOTE_DIR}"
echo "vega-deploy:arch=$(uname -m)"
echo "vega-deploy:commit=$(git -C "$dir" rev-parse --short=7 HEAD 2>/dev/null || echo missing)"
echo "vega-deploy:vega=$(vega --version 2>/dev/null || echo missing)"
echo "vega-deploy:relay=$(relay_state)"
"#
    )
}

/// Runs `script` on `target` in a login shell (so Homebrew/cargo are on PATH).
/// Returns (success, parsed `vega-deploy:` fields, last stderr line).
fn remote(target: &str, script: &str) -> (bool, Vec<(String, String)>, String) {
    let child = Command::new("ssh")
        .args([
            "-o",
            "BatchMode=yes",
            "-o",
            "ConnectTimeout=10",
            target,
            r#"exec "${SHELL:-/bin/sh}" -l -s"#,
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();
    let Ok(mut child) = child else {
        return (false, vec![], "ssh not found".into());
    };
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(script.as_bytes());
    }
    let Ok(out) = child.wait_with_output() else {
        return (false, vec![], "ssh failed".into());
    };
    let fields = String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| l.strip_prefix("vega-deploy:"))
        .filter_map(|kv| kv.split_once('='))
        .map(|(k, v)| (k.to_string(), v.trim().to_string()))
        .collect();
    let err = String::from_utf8_lossy(&out.stderr)
        .lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("")
        .trim()
        .to_string();
    (out.status.success(), fields, err)
}

fn field<'a>(fields: &'a [(String, String)], key: &str) -> &'a str {
    fields
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.as_str())
        .unwrap_or("-")
}

fn status() -> Result<ExitCode, Failure> {
    let host = local_host();
    let list = machines()?;
    println!("bin: {}", bin());
    println!("description: Build and install vega on herdr machines (builds on the target)");
    println!("source: {REPO_URL} {BRANCH} {}", source_commit());
    println!("this: {host}");
    if list.is_empty() {
        println!("machines: 0 herdr machines saved");
        println!("help: herdr machine add <ssh-target> --label <name>");
        return Ok(ExitCode::SUCCESS);
    }
    println!("machines[{}]{{name,arch,commit,vega,relay}}:", list.len());
    for m in &list {
        let (ok, f, err) = remote(&m.target, &probe_script());
        if ok {
            println!(
                "  {},{},{},{},{}",
                m.label,
                field(&f, "arch"),
                field(&f, "commit"),
                field(&f, "vega"),
                field(&f, "relay")
            );
        } else {
            println!("  {},unreachable,-,-,{}", m.label, cell(&err));
        }
    }
    println!("help: vega deploy apply <machine>   # pull, build, install");
    Ok(ExitCode::SUCCESS)
}

/// Accepts only characters safe to splice into the remote shell script.
fn branch_name(s: &str) -> Result<String, String> {
    let ok = !s.is_empty()
        && !s.starts_with(['-', '/', '.'])
        && !s.contains("..")
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '/' | '-'));
    if ok {
        Ok(s.to_string())
    } else {
        Err("use letters, digits, `.`, `_`, `/` and `-` only".into())
    }
}

fn apply(names: &[String], all: bool, branch: &str) -> Result<ExitCode, Failure> {
    let list = machines()?;
    if !all && names.is_empty() {
        return Err(Failure::new(
            "usage",
            "no machine given",
            "vega deploy apply <machine> or vega deploy apply --all",
        ));
    }
    let mut chosen = Vec::new();
    for name in names {
        match list.iter().find(|m| &m.label == name) {
            Some(m) => chosen.push(m),
            None => {
                let known: Vec<&str> = list.iter().map(|m| m.label.as_str()).collect();
                return Err(Failure::new(
                    "usage",
                    format!("unknown machine: {name}"),
                    format!("known machines: {}", known.join(" ")),
                ));
            }
        }
    }
    if all {
        chosen = list.iter().collect();
    }
    let sync = overrides::script(&overrides::local_dir()).map_err(|e| {
        Failure::new(
            "overrides_unreadable",
            e,
            "fix ~/.config/vega/config/ on this host; deploy copies it to the targets",
        )
    })?;

    // inline-md: allow (format! needs a literal to interpolate)
    let script = format!(
        r#"set -e
{PATH_SETUP}
{RELAY_STATE}
{RELAY_ENSURE}
dir="$HOME/{REMOTE_DIR}"
echo "vega-deploy:before=$(vega --version 2>/dev/null || echo missing)"
if [ -d "$dir/.git" ]; then
  git -C "$dir" fetch -q origin {branch}
  current=$(git -C "$dir" symbolic-ref -q --short HEAD || true)
  if [ "$current" != "{branch}" ] && [ -n "$(git -C "$dir" status --porcelain --untracked-files=no)" ]; then
    echo "tracked local changes prevent switching to {branch}; deploy aborted before checkout or install" >&2
    exit 1
  fi
  if git -C "$dir" rev-parse -q --verify refs/heads/{branch} >/dev/null; then
    # Reject local-only commits before changing the target's checked-out branch.
    git -C "$dir" merge-base --is-ancestor refs/heads/{branch} FETCH_HEAD || {{
      echo "local branch {branch} has commits absent from origin/{branch}; deploy aborted before checkout or install" >&2
      exit 1
    }}
    git -C "$dir" checkout -q {branch}
  else
    git -C "$dir" checkout -q -b {branch} FETCH_HEAD
  fi
  git -C "$dir" merge --ff-only -q FETCH_HEAD
else
  mkdir -p "$(dirname "$dir")"
  git clone -q --branch {branch} {REPO_URL} "$dir"
fi
VEGA_INSTALL_BY=deploy "$dir/scripts/install-cli.sh" >/dev/null 2>"$dir/.deploy.log" || {{ tail -5 "$dir/.deploy.log" >&2; exit 1; }}
echo "vega-deploy:commit=$(git -C "$dir" rev-parse --short=7 HEAD)"
echo "vega-deploy:after=$("$HOME/.local/bin/vega" --version)"
# Local config overrides from the deploying host replace the target's
# (docs/design/config.md); never fails the deploy.
( set -e
{sync}
) 2>/dev/null || echo "vega-deploy:overrides=failed"
# Agent and herdr config (vega config: skills, herdr, instructions, hooks, ...);
# units chezmoi still manages stay untouched. Never fails the deploy.
c=$("$HOME/.local/bin/vega" config apply 2>&1) || true
c=$(echo "$c" | sed -n 's/^summary: //p' | tail -1)
echo "vega-deploy:config=${{c:-failed}}"
# vegad ends at the new binary: it execs it by itself, install-cli restarted it,
# or it was stopped and starts now (docs/design/vegad.md, Lifecycle).
echo "vega-deploy:relay=$(relay_ensure)"
"#
    );

    let mut failed = 0;
    println!(
        "deployed[{}]{{name,before,after,commit,relay,overrides,config,status}}:",
        chosen.len()
    );
    for m in chosen {
        let (ok, f, err) = remote(&m.target, &script);
        let status = if ok { "ok".to_string() } else { cell(&err) };
        if !ok {
            failed += 1;
        }
        println!(
            "  {},{},{},{},{},{},{},{}",
            m.label,
            field(&f, "before"),
            field(&f, "after"),
            field(&f, "commit"),
            field(&f, "relay"),
            field(&f, "overrides"),
            cell(field(&f, "config")),
            status
        );
    }
    Ok(if failed == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    })
}

fn bin() -> String {
    let path = env::current_exe()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "unknown".into());
    match env::var("HOME") {
        Ok(home) if path.starts_with(&home) => format!("~{}", &path[home.len()..]),
        _ => path,
    }
}

/// Quotes a TOON cell when it contains a delimiter or quote.
fn cell(s: &str) -> String {
    if s.contains([',', '"', '\n']) {
        format!("\"{}\"", s.replace('"', "\\\"").replace('\n', " "))
    } else {
        s.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skill_has_valid_frontmatter() {
        vega_plugin::assert_guide(SKILL, "deploy");
    }

    #[test]
    fn branch_name_accepts_plain_and_rejects_shell_syntax() {
        for ok in ["main", "feat/x-1", "release_1.2"] {
            assert!(branch_name(ok).is_ok(), "{ok}");
        }
        for bad in ["", "-x", "/x", ".x", "a..b", "a;b", "a b", "$(x)", "a`b`"] {
            assert!(branch_name(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn from_branch_defaults_to_main() {
        let cli = Cli::try_parse_from(["vega deploy", "apply", "--all"]).unwrap();
        let Some(Cmd::Apply { from_branch, .. }) = cli.cmd else {
            panic!("apply expected")
        };
        assert_eq!(from_branch, "main");
    }

    #[test]
    fn field_reads_parsed_pairs() {
        let f = vec![("arch".to_string(), "arm64".to_string())];
        assert_eq!(field(&f, "arch"), "arm64");
        assert_eq!(field(&f, "vega"), "-");
    }

    /// Runs `relay_ensure` against a fake `vega` that reports `status`
    /// (stopped until `relay start` ran, then current). Returns (output, starts).
    fn ensure_with(status: &str) -> (String, usize) {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path().display();
        let fake = format!(
            r#"#!/bin/sh
case "$*" in
  "post relay start") echo x >> "{d}/starts" ;;
  "post relay status")
    if [ -f "{d}/starts" ]; then echo "vegad: running, binary current"
    else echo "{status}"; fi ;;
esac
"#
        );
        let vega = dir.path().join("vega");
        std::fs::write(&vega, fake).unwrap();
        std::fs::set_permissions(&vega, std::os::unix::fs::PermissionsExt::from_mode(0o755))
            .unwrap();
        let out = Command::new("sh")
            .arg("-c")
            .arg(format!("{RELAY_STATE}\n{RELAY_ENSURE}\nrelay_ensure"))
            .env("PATH", format!("{d}:/usr/bin:/bin"))
            .output()
            .unwrap();
        let starts = std::fs::read_to_string(dir.path().join("starts"))
            .map(|s| s.lines().count())
            .unwrap_or(0);
        (
            String::from_utf8_lossy(&out.stdout).trim().to_string(),
            starts,
        )
    }

    #[test]
    fn relay_ensure_starts_a_stopped_vegad() {
        assert_eq!(ensure_with("vegad: stopped"), ("current".into(), 1));
    }

    #[test]
    fn relay_ensure_leaves_a_current_vegad_alone() {
        assert_eq!(
            ensure_with("vegad: running, binary current"),
            ("current".into(), 0)
        );
    }
}
