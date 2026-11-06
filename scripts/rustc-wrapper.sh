#!/bin/sh
# RUSTC_WRAPPER (mise.toml, vega-919z): sccache when installed, so worktrees
# share compiled dependencies; else rustc unchanged. sccache refuses
# incremental builds, so the main checkout (CARGO_INCREMENTAL=1, its target/
# persists) compiles without it.
if [ "${CARGO_INCREMENTAL:-0}" != 1 ] && command -v sccache >/dev/null 2>&1; then
	exec sccache "$@"
fi
exec "$@"
