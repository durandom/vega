//! Agent-session shim for `herdr`.
//!
//! Installed under the name `herdr` in a directory that agent sessions put first
//! on `PATH`. For now it only resolves the real `herdr` binary (`HERDR_BIN_PATH`,
//! which herdr exports into its panes, then `PATH`) and replaces itself with it
//! via `exec`, so argv, stdio, TTY, signals and exit code are exactly those of
//! the original.

use std::process::ExitCode;

fn main() -> ExitCode {
    vega_shim::exec_real("herdr", Some("HERDR_BIN_PATH"))
}
