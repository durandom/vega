# vega-browser

Named agent-browser sessions (docs/design/browser.md): the shared global
Chromium on 127.0.0.1:9223 (`ab`, `chromium`), the throwaway fallback for bot
blockers (`launch`, `cleanup`), and the session inventory with owners
(`ls [--json]`, `gc`, `close`) built from agent-browser's session list, the
owner records and the browser processes. One rule: every agent-browser call
names its session.

`vpn start|stop|status|trust [--json]` manages a dedicated headed Chromium via autossh
SOCKS to puck, separate from the global browser. After explicit start, agents
attach with `ab --profile vpn --session <project>-vpn-<task>`. Defaults: CDP 9224,
SOCKS 1080, persistent ~/.local/share/vega-chromium-vpn; optional configuration
in ~/.config/vega/browser-vpn.json. Full-profile SOCKS uses remote DNS without
PAC or direct fallback. Status probes the corp host once; stop preserves data.
Requires autossh and noninteractive SSH access to puck. Start imports the pinned
corp root into the stopped VPN profile; `vpn trust` imports without starting.
`ca_sha256` config overrides the default SHA-256 pin; mismatches fail before
import. Already trusted is a read-only no-op. Status includes `ca_trusted`.
The macOS keychain and browser policies are unchanged.

Replaces the global `browser` skill (`~/.agents/skills/browser`); the old
script paths forward to this plugin for the migration
(`plugins/browser/forward/`, installed by `scripts/install-cli.sh`).

Design: [browser.md](../../docs/design/browser.md).

Agent guide: `vega skills browser`
