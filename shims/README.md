# shims

Agent-only wrappers for known CLIs. Each shim is installed under the original name in
`~/.local/share/vega/shims/bin` (`mise run install-cli`). `activate.sh` puts that directory first on `PATH` only in
non-interactive shells that carry an agent marker (`CLAUDECODE`, `CODEX_THREAD_ID`, `PI_CODING_AGENT`, `AI_AGENT`), so
humans, scripts and CI always see the original tools.

A shim keeps the original stdout, stdin and exit code (it `exec`s the real binary) and only adds `stderr` hints;
denials are documented exceptions. Principles: [agent-tooling-principles.md](../docs/design/agent-tooling-principles.md)
(P2, P3).

| Shim | Purpose |
| --- | --- |
| [`bd`](bd/) | Per-worktree claim identity (`BEADS_ACTOR`) |
| [`git`](git/) | Gates `git push` in repos with `.vega/gate.toml` (LLM review, `vega gate`); everything else is a plain exec |
| [`gh`](gh/) | Denies the CI watch loops, hints at `vega ci watch`, counts calls |
| [`herdr`](herdr/) | Plain passthrough, the hook point for later additions |
| [`tea`](tea/) | Hints at `vega ci watch`, counts calls, install hint |

`activate.test.sh` tests the activation rules (`mise run check`).
