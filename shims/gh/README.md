# gh shim

Passes every `gh` call through to the real binary, with two exceptions and three additions.

- **Denied:** `gh run watch` and `gh pr checks --watch`. They poll every 3 s and 10 s and drain the GitHub rate limit
  that every agent and CI job of the user shares. The error points at `vega ci watch`, which mails the verdict.
- **Hint:** the third `gh pr checks`, `gh run view` or `gh run list` within 10 minutes gets a one-time hint (per
  session) at `vega ci watch`.
- **Hint:** `gh workflow run` gets a one-time hint (per session) at `vega ci watch --run <id>` or `--workflow`: a PR
  or branch watch mails once per head and misses a dispatched run on a head it already reported.
- **Counts:** calls per pane, shown by `vega uac status`.

Design: [ci-watch.md](../../docs/design/ci-watch.md) (gh shim).

Agent guide: `vega skills ci`
