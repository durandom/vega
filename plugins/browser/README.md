# vega-browser

Named agent-browser sessions (docs/design/browser.md): the shared global
Chromium on 127.0.0.1:9223 (`ab`, `chromium`), the throwaway fallback for bot
blockers (`launch`, `cleanup`), and the session inventory with owners
(`ls [--json]`, `gc`, `close`) built from agent-browser's session list, the
owner records and the browser processes. One rule: every agent-browser call
names its session.

Replaces the global `browser` skill (`~/.agents/skills/browser`); the old
script paths forward to this plugin for the migration
(`plugins/browser/forward/`, installed by `scripts/install-cli.sh`).

Design: [browser.md](../../docs/design/browser.md).

Agent guide: `vega skills browser`
