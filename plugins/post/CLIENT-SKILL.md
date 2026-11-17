---
name: vega-post
description: Sends and reads agent mail through a NATS broker on native Windows. Use when a post pointer arrives or an agent must send, reply or report completion.
---

# vega post: native Windows client

- `vega post status`: broker, verified caller, unread count.
- `vega post doctor`: Windows core PID/session and recovery hint; no broker or caller identity required. This check never starts a core.
- `vega post send [--host <host>] <pane> "<text>"`: send mail. Reply by copying the timestamped `--re` command from inbox; never reply to acknowledgements.
- `vega post inbox`: read and ack; `--keep` leaves unread, `--show <id>` rereads without ack.
- `vega post log`: archive without ack; `--json` emits full bodies.
- `vega post done --outcome succeeded|failed "<summary>"`: report once, then stop.
- `vega post keygen`: own user nkey. Only its public key leaves this host.
- `vega post identity enroll --checkout <path>`: fresh v2 owner-local enrollment after operator provisioning; see docs/design/windows.md.

Config defaults to `%APPDATA%\vega`; state to `%LOCALAPPDATA%\vega`. XDG overrides remain supported and require private directory ACLs. Set `VEGA_POST_HOST=io-win`; never use io's WSL seed or an admin seed. `vega post relay start` ensures one native local hc pointer worker, surviving hc exec/terminal exit through a temporary limited interactive-user task (requires a logged-in user). `relay status [--json]` reports it; `relay stop` stops it; `relay serve` runs in the foreground. Cutover/admin commands stay Unix-only.
