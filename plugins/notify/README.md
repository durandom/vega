# vega notify

Raise, list, acknowledge and resolve operator asks persisted as notes. The origin
owns each ask; remote id updates use the existing vegad command transport.

See [the design](../../docs/design/notify.md) and `vega notify --help`.
vegad delivers through the shared toast/macOS channel adapters with durable receipts,
seat election and focus/draft suppression. Phone channels and scheduling land separately.
