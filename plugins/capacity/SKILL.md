---
name: vega-capacity
description: Shows the subscription capacity of every provider window from token-burn's stored sample (binding window marked, forecast run-out), records the Slayer's quota resets (owned, planned, used, override) and reports token-burn's health. Use when choosing a kind or provider by headroom, when a spawn warns that a window is thin, when the Slayer reports or plans a reset, or when capacity shows unknown.
---

# vega capacity: subscription headroom

token-burn (the Slayer's quota monitor) stores a usage sample of each provider every few minutes. vega reads that sample; it never calls a provider and never starts or configures token-burn.

- `vega capacity`: every window per provider: scope, percent left, reset, `runs_out` from a confident forecast, `binding: yes` on the account window with the least left per provider (the one the spawn warning and `vega uac status` read). `model:<name>` windows (`seven_day_fable`) show, but never bind; the scopes are the config source `capacity/windows.toml` (`vega config init capacity/windows.toml` to override). Then the resets ledger and a `token_burn:` health line (version, user service, sample age).
- Ledger, only on the Slayer's word: `resets <provider> <n>` (owned), `plan <provider> --this-week|--at YYYY-MM-DD|--clear`, `used <provider>`, `override <provider> --until YYYY-MM-DD|--clear`. A plan with a reset owned, or an override, covers a thin window: no spawn warning.
- `vega uac capacity ...` is an alias.

Advisory only: vega never picks the kind. A thin binding window (under 10% or out within 2h) means: spawn another kind or provider if the run outlasts it, or ask the Slayer about a reset.

`token_burn: stale` or `missing`: say so in the report and follow its `help:` line with the user; do not install or start token-burn unasked.
