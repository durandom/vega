# git shim

Resolves the real `git` (skipping its own dir on `PATH`) and replaces itself with it via `exec`. Every command but
`git push` is a plain `exec`: the argv check is pure (git's global options skipped), with no config read and no bd
call, so stdout, stdin and exit code are git's own.

`git push` in a repo with a `.vega/gate.toml` on the push target needs a recorded `vega gate` pass for each segment
of the push. Without one the shim starts the review (`vega gate start`: a reviewer tab, verdict by mail) and fails
the push with exit 1 and one stderr line: a documented deviation from P2
([agent-tooling-principles.md](../../docs/design/agent-tooling-principles.md)). Not gated: `--dry-run`, deletes,
tag-only pushes, repos without the config or without `.beads`. The gate fails open on its own errors (one stderr
line `vega gate: <error>; push not gated`, then the push); `VEGA_GATE=skip` skips it visibly and records the skip;
a gate reviewer (`VEGA_GATE_ROLE=reviewer` or a cwd in a gate clone) may not push.

Design: [gate.md](../../docs/design/gate.md). Guide: `vega skills gate`.
