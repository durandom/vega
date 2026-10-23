# vega capacity

Shows how much subscription quota each provider window has left, so the UAC can pick an agent kind or provider
with headroom. It reads the sample that token-burn (the Slayer's quota monitor) stores and never calls a provider
itself. It also keeps a ledger of the Slayer's quota resets. Advisory only: vega never picks the kind.

Commands: `vega capacity`, `resets`, `plan`, `used`, `override`.

Design: [capacity.md](../../docs/design/capacity.md).

Agent guide: `vega skills capacity`
