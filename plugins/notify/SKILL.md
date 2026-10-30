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

Delivery, escalation and reminders land in later beads. This build stores asks only;
`vega notify test <channel>` reports `channel_unavailable` until channel support lands.
Each command accepts `--help`.
