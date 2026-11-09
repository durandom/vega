# vega notify

Raise, list, acknowledge and resolve operator asks persisted as notes. The origin
owns each ask; remote id updates use the existing vegad command transport.

See [the design](../../docs/design/notify.md) and `vega notify --help`.
vegad delivers through the shared toast/macOS channel adapters with durable receipts,
seat election and focus/draft suppression. notify.toml controls levels, project reminders and
quiet hours; action escalates after 15 minutes and reminds once at an hour, urgent reminds once
at 15 minutes. Engaged Uber receives notification-only mail for the phone step. Actual phone
adapters land separately.
## Overview JSON (schema 1)

`vega notify [list] --json [--project <project-id>]` emits one object:
`{schema: 1, asks: [...], registry_error: string|null}`. Rows have `id`
(qualified `host=id`), `host`, `level`, `project`, `pane`, `line`, `bead`,
`cmd`, and `acknowledged` (Unix seconds|null). Project matching is exact.
Registry failures retain local asks and set `registry_error`. The TOON list
also accepts `--project`. Neither read acknowledges asks.

JSON errors use `{error: {code, message, hint}}` (exit 2 usage, 1 runtime).
Empty lists are arrays. TOON remains the default; consumers should ignore
future additive fields.
