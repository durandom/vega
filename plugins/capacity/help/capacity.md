vega capacity: subscription capacity from token-burn, the resets ledger, token-burn's health

Usage: vega capacity [command]

With no command: every window of every provider in token-burn's stored sample
(scope, percent left, reset, when a confident forecast runs it out before the
reset; `binding: yes` marks the account window with the least left per provider,
the one the spawn warning reads), the Slayer's resets ledger, and token-burn's
health (version, user service, sample age).
Read-only; vega never calls a provider or starts token-burn.

Window scopes come from the vega config source capacity/windows.toml (repo
default; local override in ~/.config/vega/config/, `vega config init
capacity/windows.toml`). A window not listed there is `account` and may bind;
`model:<name>` windows show but never bind.

Commands (the resets ledger, $XDG_STATE_HOME/vega/capacity.json; the Slayer owns
and triggers resets himself, vega only records them):
  resets <provider> <n>     resets owned (any provider in the sample)
  plan <provider> --this-week|--at <YYYY-MM-DD>|--clear
                            a reset he plans to use by that UTC day
                            (--this-week: the coming seven days); needs one owned
  used <provider>           one reset used: one fewer owned, plan done
  override <provider> --until <YYYY-MM-DD>|--clear
                            no spawn warning through that day
A planned reset (with one owned) or an override covers a thin binding window:
`out in ~22h; reset planned this week`, no spawn warning. Each command prints the ledger.

Flags:
  --skill      the agent guide (also `vega skills capacity`)
  -h, --help   this help
  -V, --version

Env: VEGA_TOKEN_BURN names the binary; VEGA_CAPACITY_FIXTURE is a dir with recorded
status.json, forecast.json (and service-status.json) instead of token-burn.

Examples:
  vega capacity                           # every window, ledger, health
  vega capacity resets claude 2           # the Slayer holds two claude resets
  vega capacity plan claude --this-week   # he uses one within seven days
  vega capacity used claude
