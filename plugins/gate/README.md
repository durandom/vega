# vega gate

An LLM review before an agent pushes. A repo opts in with `.vega/gate.toml` (which rubric reviews which paths),
read from the push target. The `git` shim checks a recorded pass for each segment of a push (a run of direct
commits, each merged branch) and otherwise starts a reviewer agent in a herdr tab and fails the push until its
verdict. Records are beads (label `gate`), synced across hosts.

Commands: `vega gate`, `check`, `start`, `verdict`, `log`, `gc`. The library (`vega_gate`) is shared with
[`shims/git`](../../shims/git/).

Reviewer selection excludes exact author models and prefers other harnesses; a different model on an
author harness is a fallback. Any unknown author model excludes that entire harness. Reviewer prompts
name immutable target/base/head SHAs, since clone remote refs can differ from the push target. After new
commits run `vega gate start` again; never re-point a reviewer by mail. The old reviewer closes and a
fresh review is bound to the new head and generation.

Design: [gate.md](../../docs/design/gate.md).

Agent guide: `vega skills gate`
