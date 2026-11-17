Broker: VEGA_NATS_URL, else the first line of %APPDATA%\vega\nats.url.
Key: VEGA_NATS_NKEY, else %APPDATA%\vega\nats.nk. XDG overrides are retained.
State: %LOCALAPPDATA%\vega. Use a private user profile or owner-restricted XDG directories.

Examples:
  vega post status
  vega post doctor                 # Windows: local core session, no broker needed
  vega post send --host proteus t7 "<text>"
  vega post inbox
  vega post log --since 2h
  vega post done --outcome succeeded "<summary>"

Replies use send --re: copy the timestamped reply command from inbox.
Fresh-host setup: keygen, operator provisioning, then identity enroll --checkout <path>.
Only the public key leaves this host. Never reuse another host's seed or an admin seed.
`vega post relay start` starts the single-instance local hc pointer worker, surviving hc exec/terminal exit; `relay status [--json]` reports it and `relay stop` stops it. `relay serve` runs it in the foreground. Windows uses a temporary limited interactive-user task (requires a logged-in user). Legacy cutover and admin commands stay Unix-only.
Runbook: docs/design/windows.md.
