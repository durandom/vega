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
  vega lanista pin zai --tier budget --set glm-5.2 --until 2h
  vega lanista pin zai --tier budget --clear
Host layer: `pin <lane> --tier <tier> --host <host> --set <allowed-model>`
(or `--clear`). Host expiry defaults to never; `--until 2h` expires to global.
Without --host, private hosts edit the project; inbox-only hosts edit themselves.
Resolution: project > local host > registry default > global.
`--default --set <model>` / `--default --clear` edits `<control>.lanista.default`
on a private host (default expiry never). It conflicts with --host.
Inbox-only hosts neither read nor edit defaults. Host source is `host (<host>)`.
`vega lanista hosts [--host <host>] --json` includes effective pins and allowlists.
Inbox-only hosts relay through their theme control host and cache exact-key reads.
Pi project/host overrides win; otherwise Pi resolves the full zai chain.
Use `pin zai --default --set <glm-model>` for Pi's shared default, not lane pi.
