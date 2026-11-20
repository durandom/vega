# agent-browser shim

Every call names a session with `--session <name>` or `AGENT_BROWSER_SESSION`.
Ordinary headed calls use shared Chrome for Testing in the Git project's context.
Start with `vega browser dev --session <name> open <url>` and use
`agent-browser --session <name>` for subsequent commands. The shim keeps explicit
attachments, existing legacy bindings and headless launches on the upstream path.
Named calls record the owner in `$XDG_STATE_HOME/vega/browser/sessions.json`.

- **Denied:** calls without a session name (exit 1, a stderr hint pointing at `vega skills browser`).
  Default sessions are the leftovers nobody can attribute, and `close` without a name would close one
  of them. Documented deviation from P2 (agent-tooling-principles).
- **Exempt without a name:** `--help`/`--version`, `skills …`, `session list`.
- The shim `exec`s the real binary by absolute path (`VEGA_AGENT_BROWSER_BIN`, then PATH),
  skipping vega shim builds.

Design: [browser.md](../../docs/design/browser.md) (Shim).

Agent guide: `vega skills browser`
