# Capacity: subscription headroom from token-burn

Status: accepted
Date: 2026-10-16
Bead: vega-lhc.117 (Slayer decision 2026-10-16); source adapters and window scopes: vega-lhc.119
Follows: [agent-tooling-principles.md](agent-tooling-principles.md), [onboarding.md](onboarding.md)

## Context

token-burn (github.com/durandom/token-burn, Go) is the Slayer's quota monitor: its daemon stores a usage
sample of every subscription window (claude, codex, xai, zai, antigravity, copilot) every few minutes and
forecasts when each runs out. vega read it piecemeal: the binding window per lane provider in
`vega uac status`, the `vega mux spawn` warning, vegad's quota attention, and the resets ledger under
`vega uac capacity`. Nobody showed every window (claude's `five_hour` and `seven_day` side by side,
model-specific allowances such as `seven_day_fable`, copilot), nor whether token-burn itself is healthy.

Capacity is its own plugin, not part of lanista: lanista is the model catalog (which model to start); capacity
is the subscription budget (how much is left). Similar use, different reason to change.

## Split

| Where | Owns |
|---|---|
| `crates/vega-herdr/src/capacity.rs` | the source-neutral model (`Reading`, `Scope`, `Sample`, the `Source` trait), the binding window per provider, the kind→provider map for spawns, the spawn check, the `vega uac status` table |
| `crates/vega-herdr/src/capacity/token_burn.rs` | the token-burn adapter: runs `status --json`, `forecast --json` (or reads `VEGA_CAPACITY_FIXTURE`) and maps token-burn's names into readings |
| `crates/vega-herdr/src/capacity/scopes.rs`, `config/capacity/windows.toml` | window scopes as data (a vega config source) |
| `crates/vega-herdr/src/resets.rs` | the ledger file `$XDG_STATE_HOME/vega/capacity.json` and its rules |
| `plugins/capacity` | the user-facing view (every window, ledger, health) and the ledger commands |
| `plugins/uac` | `vega uac status` keeps its one row per provider; `vega uac capacity [...]` execs `vega-capacity [...]` |
| `plugins/mux`, `plugins/post` | the spawn warning and vegad's quota attention, through the shared crate |
| `vega_config::infra` | `token-burn` as an optional component (on PATH, or `VEGA_TOKEN_BURN`) |

Move, don't duplicate: one reader, one ledger; the plugin renders.

## Source adapters

vega must not be chained to token-burn, and a provider new to the source must not need code in vega
(Slayer 2026-10-16, vega-lhc.119). vega's model is source-neutral:

| Field | Meaning |
|---|---|
| `provider`, `window` | the source's names, taken as they come |
| `scope` | `account` or `model:<name>`, from the window scopes below (adapters set `account`) |
| `left` | percent left; 0 when the source says the limit is reached |
| `reset_at`, `runs_out_at`, `observed_at` | reset, confident forecast run-out, sample time (Unix seconds) |

A `Source` adapter maps a monitor's stored sample into these readings; only the adapter knows the monitor's
field names (token-burn: `Provider`, `WindowName`, `RemainingPercent`, `LimitReached`, `ResetAt`,
`ObservedAt`, and the forecast's `estimated_100_at` with `confidence` ≥ 0.5). Another monitor is another
adapter. token-burn's health (version, service) stays in `plugins/capacity/src/health.rs`: it is about the
token-burn installation, not the model.

## Window scopes

`config/capacity/windows.toml` lists the windows that limit one model, not the account:

```toml
[claude]
seven_day_fable = "model:fable"

[codex]
additional_codex_bengalfox_primary = "model:bengalfox"
```

It is a vega config source (config.md "Sources"): the repo default is embedded; a local
`~/.config/vega/config/capacity/windows.toml` (`vega config init capacity/windows.toml`) replaces the whole
file and travels with `vega deploy`. It is read at each capacity read, not rendered to a target, so it is no
config unit. A window not listed is `account`. A file that does not parse makes the capacity unknown and
names the file.

## Binding

Per provider, the binding window is its `account` window with the least left (a passed reset counts as
full); on a tie the one that runs out first, then the first in the sample. `model:` windows never bind.
Every provider the source reports shows and binds; the ledger accepts any of them (and any provider already
in the ledger, so an entry stays editable when the source drops it; with the source unknown it checks none).

What stays code in vega is vega's knowledge of its own kinds: the kind→provider map for spawns (`claude`,
`codex`, `agy`→`antigravity`, `pi` by `--provider` or the `provider/` of `--model`: `xai`, `zai`).

## vega capacity

```
vega capacity                                    # every window, ledger, token-burn health (read-only)
vega capacity resets <provider> <n>              # resets owned
vega capacity plan <provider> --this-week|--at <YYYY-MM-DD>|--clear
vega capacity used <provider>
vega capacity override <provider> --until <YYYY-MM-DD>|--clear
```

```
capacity[14]{provider,window,scope,left,resets_in,runs_out,binding}:
  antigravity,claude_and_gpt,account,100%,4h57m,,yes
  ...
  claude,five_hour,account,98%,4h15m,,yes
  claude,seven_day,account,100%,6d17h,,
  claude,seven_day_fable,model:fable,100%,6d17h,,
  codex,primary,account,93%,6d1h,,yes
  copilot,premium_interactions,account,99.7%,15d16h,,yes
  ...
resets[2]{provider,owned,plan,override_until,last_used}:
  claude,0,,,2026-10-12
  codex,3,,,
token_burn: ok (v0.1.37, no service, sample 0m old)
```

- Providers and their windows in sample order (token-burn sorts them by name).
- `binding: yes` is the binding window (see "Binding"); that row also carries the Slayer's cover
  (`reset planned this week`, `override until …`) and the `(soon)` mark. `model:` windows never bind.
- `vega uac status` shows the binding window of every provider in the sample, copilot included.
- `token_burn:` is `ok`, `stale` (newest sample older than 30 min), `no sample` or `missing`, with the
  version and the user service (`token-burn service-status --json`: running, installed, none). A sample can
  be fresh without the service (the daemon run another way), so the sample age decides `ok`. Not ok adds a
  `help:` line (`token-burn install`, `token-burn once --store`); vega never runs them.

## Rejected

- A hardcoded list of binding windows per provider (`PROVIDERS`, vega-lhc.117): every new provider or
  window in token-burn needed a vega release. Replaced by "unknown windows are account-wide" plus the
  scopes file.
- Scopes reported by the adapter: token-burn does not know which window a vega lane depends on; that is
  vega's (the Slayer's) knowledge, so it is config.
- Folding capacity into lanista: different data, different reason to change.
- Keeping the ledger commands in `vega uac`: the Slayer records resets without being the UAC.
- Checking the daemon in `vega config infra`: infra checks stay cheap and local (onboarding.md); the
  service call and the sample age belong to `vega capacity`.
