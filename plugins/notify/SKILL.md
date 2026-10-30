---
name: vega-notify
description: Raises and settles blocking asks for the operator. Use when a UAC waits for a go, approval, login, command or device test, or to inspect, acknowledge or resolve those asks.
---

# vega notify

Raise: `vega notify action "<project>: <what> -> <pane>" [--bead <id>] [--cmd "<cmd>"]`.
Use `urgent` only for production or data at risk. Never put secrets in the line or command.
`--pane` defaults to the caller, `--project` to the checkout; `--key` defaults to `<project>/<pane>`.
An open key keeps its original clock on re-raise. A lower level changes nothing.

`vega notify` or `vega notify list` lists open asks from every available host.
`vega notify resolve` settles this checkout/pane's default key when the UAC continues.
Resolve a specific ask with `<host>=<id>`, `--key <key>` or `--pane <pane>`.
Key/pane selectors are local to the origin host; qualified ids support remote updates.
`vega notify ack <host>=<id>` records that it was seen, keeping it open.
Pane/bead closure resolves asks on the origin's next metadata refresh.

vegad delivers on the host with the newest human input; failed or suppressed sends retry.
With no config, delivery uses toast only. notify.toml controls levels, channels and project reminders.
Action escalates to configured phone channels at 15m and reminds once at 1h; urgent reminds at 15m.
Quiet hours 22:30–07:30 defer phone sends, except PagerDuty high. Ack stops both later steps.
An engaged Uber receives notification-only mail instead of phone delivery; answer in the origin pane.
`vega notify test toast|macos` probes the local desktop without creating an ask.
Actual phone adapters land separately.
Each command accepts `--help`.
