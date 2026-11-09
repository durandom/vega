# vega capacity

Shows how much subscription quota each provider window has left, so the UAC can pick an agent kind or provider
with headroom. It reads the sample that token-burn (the Slayer's quota monitor) stores and never calls a provider
itself. It also keeps a ledger of the Slayer's quota resets. Advisory only: vega never picks the kind.

Commands: `vega capacity`, `resets`, `plan`, `used`, `override`.

Design: [capacity.md](../../docs/design/capacity.md).

Agent guide: `vega skills capacity`
## Overview JSON (schema 1)

`vega capacity --json` emits `{schema: 1, windows: [...], source_error:
string|null, ledger_error: string|null}`. Each window has `provider`, `window`,
`scope` (`account` or `model:<name>`), `left_percent`, `used_percent`, `binding`,
`resets_in` and `runs_out_in` (seconds|null), `sample_age` (seconds), `stale`,
`override` (effective planned-reset/override cover text|null), and `warns`
(binding windows only). Percentages are numbers; binding, stale and warns
are booleans. A passed reset makes effective headroom 100%, as in TOON.
Source failure gives an empty array and `source_error`; an unreadable ledger
sets `ledger_error`. JSON does not probe or start the token-burn service.
Credits await token-burn-cu9's final remote-read CLI contract (vega-lweq).

Errors use `{error: {code, message, hint}}` (exit 2 usage, 1 runtime).
TOON remains the default; consumers should ignore future additive fields.
