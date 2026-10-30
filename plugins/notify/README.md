# vega notify

Raise, list, acknowledge and resolve operator asks persisted as notes. The origin
owns each ask; remote id updates use the existing vegad command transport.

See [the design](../../docs/design/notify.md) and `vega notify --help`.
vegad delivers through the shared toast/macOS channel adapters with durable receipts,
seat election and focus/draft suppression. notify.toml controls levels, project reminders and
quiet hours; action escalates after 15 minutes and reminds once at an hour, urgent reminds once
at 15 minutes. Engaged Uber receives notification-only mail for the phone step. Actual phone
adapters land separately.
