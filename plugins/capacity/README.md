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
Rows also carry `account_id`, `credits` (fresh credit state or null), and
`on_credits` (boolean, binding windows only). `credits` has `has_credits`,
`unlimited`, `overage_limit_reached`, `balance` (exact decimal string|null,
in provider credit units), and `observed_at` (Unix seconds). Credits require
all three flags and their own timestamp, at most 120 seconds old and never
in the future. Quota timestamps cannot establish credit freshness.

A thin binding window with fresh `(has_credits || unlimited)` and no overage
limit reached shows `on credits <balance>` (or `on credits unlimited`) in the
TOON runs_out cell and suppresses the spawn warning. Missing/stale credits
show `credits unknown` and retain the warning. Known unavailable credits or
an overage limit also retain it. Percentages still describe quota headroom;
credits do not turn 0% into a full subscription. TOON remains the default.

Errors use `{error: {code, message, hint}}` (exit 2 usage, 1 runtime).
TOON remains the default; consumers should ignore future additive fields.
