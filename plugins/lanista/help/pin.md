Resolve this project's override first, then the global seats shipped by vega.
Project identity is the normalized Git fetch remote ID, as for vega uac.
Pins use <host>.lanista.v2.<hex(id)>.<hex(seat)>.<hex(lane)>.<hex(tier)>
and schema 2. Coordinated rollout: docs/design/project-registry-cutover.md.

vega lanista pin <lane> [--tier flagship|balanced|budget] [--seat <seat>]
  --set <model>       Override this project's lane/tier (conflicts with --clear)
  --clear             Remove the override; global seats apply
  --until <duration>  With --set: positive 30m, 2h, 1d, or never
                      Default: binding provider quota reset; unknown reset is an error

Defaults: tier balanced, seat herdr-agents. source names global seats or the
project override's writer host; expires is Unix seconds or never.
Unavailable registry reads fall back to global seats and say so. Writes fail.

Examples:
  vega lanista pin codex --tier budget
  vega lanista pin xai --tier budget --set grok-4.7 --until 2h
  vega lanista pin xai --tier budget --clear
