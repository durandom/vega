//! Answering a decision: `vega uac answer <bead> "<text>"` adds the comment
//! (author Slayer) and mails the project's UAC. It runs in the repo the board
//! shows (the working directory, see `main::apply_working_dir`).

use crate::bd::output_within;
use anyhow::{bail, Result};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

/// Comment plus mail; the mail goes through vegad, so allow a little more
/// than a bd call.
const ANSWER_TIMEOUT: Duration = Duration::from_secs(45);

/// The `vega` binary: PATH first, then `~/.local/bin`, where `mise run
/// install` puts it. herdr launches plugins with a minimal PATH.
pub(crate) fn resolve_vega() -> Option<PathBuf> {
    let on_path = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect::<Vec<_>>())
        .unwrap_or_default()
        .into_iter()
        .map(|d| d.join("vega"));
    let home = std::env::var_os("HOME").map(|h| Path::new(&h).join(".local/bin/vega"));
    on_path.chain(home).find(|p| p.is_file())
}

/// Answer `id` with `text`. Ok holds the status line: `vega uac answer`'s
/// `uac:` line, which says whether the UAC was told.
pub fn answer(id: &str, text: &str) -> Result<String> {
    let Some(vega) = resolve_vega() else {
        bail!("vega not found (PATH, ~/.local/bin): run mise run install in the vega repo");
    };
    let mut cmd = Command::new(vega);
    cmd.args(["uac", "answer", id, text]);
    let out = output_within(cmd, ANSWER_TIMEOUT)?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    if !out.status.success() {
        bail!(
            "vega uac answer: {}",
            failure(&stdout, &String::from_utf8_lossy(&out.stderr))
        );
    }
    Ok(status_line(id, &stdout))
}

/// The `uac:` line, else the last line, else a plain confirmation.
fn status_line(id: &str, stdout: &str) -> String {
    let lines: Vec<&str> = stdout
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    lines
        .iter()
        .find(|l| l.starts_with("uac:"))
        .or(lines.last())
        .map(|l| format!("answered {id} · {l}"))
        .unwrap_or_else(|| format!("answered {id}"))
}

/// One line for the status bar from a failed run: AXI errors go to stdout
/// as `error{code,message,hint}:` plus a row; fall back to stderr.
fn failure(stdout: &str, stderr: &str) -> String {
    let mut lines = stdout.lines().map(str::trim).filter(|l| !l.is_empty());
    let first = lines.next().unwrap_or("");
    let line = if first.starts_with("error") {
        lines.next().unwrap_or(first)
    } else if !first.is_empty() {
        first
    } else {
        stderr
            .lines()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("failed")
    };
    line.trim().chars().take(120).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_line_prefers_the_uac_line() {
        let out = "answered: t-1 (comment by Slayer)\nuac: w3V:p1 on proteus, told by mail\n";
        assert_eq!(
            status_line("t-1", out),
            "answered t-1 · uac: w3V:p1 on proteus, told by mail"
        );
        assert_eq!(status_line("t-1", "only\n"), "answered t-1 · only");
        assert_eq!(status_line("t-1", ""), "answered t-1");
    }

    #[test]
    fn failure_takes_the_error_row_or_stderr() {
        let axi = "error{code,message,hint}:\n  unknown_bead,no bead x in this repo,see status\n";
        assert_eq!(
            failure(axi, ""),
            "unknown_bead,no bead x in this repo,see status"
        );
        assert_eq!(failure("", "boom\n"), "boom");
    }
}
