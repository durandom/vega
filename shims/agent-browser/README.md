# agent-browser shim

Passes every agent-browser call with a session name (`--session <name>` anywhere in the args, or a
non-empty `AGENT_BROWSER_SESSION`) through to the real binary unchanged, and upserts the owner record
(`$XDG_STATE_HOME/vega/browser/sessions.json`: pane, host, project, cwd, first_seen, last_seen) so
`vega browser ls`/`gc` can attribute sessions.

- **Denied:** calls without a session name (exit 1, a stderr hint pointing at `vega skills browser`).
  Default sessions are the leftovers nobody can attribute, and `close` without a name would close one
  of them. Documented deviation from P2 (agent-tooling-principles).
- **Exempt without a name:** `--help`/`--version`, `skills …`, `session list`.
- **No recursion:** the shim `exec`s the real binary by absolute path (`VEGA_AGENT_BROWSER_BIN`, then
  PATH skipping itself); `vega-browser` resolves it the same way.

Design: [browser.md](../../docs/design/browser.md) (Shim).

Agent guide: `vega skills browser`
