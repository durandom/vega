---
name: vega-lanista
description: Installs and upgrades the lanista LLM model catalog, ships its seats allowlist, and resolves model tiers. Use when choosing an agent model, editing project/host/default pins, or diagnosing a missing or stale catalog.
---

# vega lanista

- `vega lanista`: install, catalog sync, seats and global pins.
- `vega lanista install [--ref <ref>]`: install/upgrade from git and sync repo seats.
  Editable or foreign installs require `--replace`.
- `vega lanista doctor`: health checks.
- `vega lanista pin <lane> --tier flagship|balanced|budget`: resolve project >
  host > registry default > repo seats. Source identifies the winning layer.
- Add `--set <model> [--until 2h|never]` or `--clear` for project overrides.
  Project expiry defaults to the binding quota reset; unknown quota requires --until.
- Add `--host <host>` for host configuration, or `--default` for the control
  host's registry default. These layers default to expiry never. Every set must
  match the repo allowlist. Private foreign writes require a broker grant.
- Inbox-only hosts edit only themselves through their theme control host's
  validated relay. They read only their exact host key, use its cache offline,
  and never read/edit registry defaults. Explicit project read grants use
  `VEGA_LANISTA_PIN_HOSTS`; no bucket enumeration is attempted.
- `vega lanista hosts [--host <host>] --json`: effective models, source, expiry,
  resolved lane and allowlists. Private views include the current project's pins.

Pi project/host overrides may select allowlisted Google models; otherwise Pi
uses the full zai chain: project > host > registry default > repo seats.
`pin pi --default --set` is refused; configure `pin zai --default --set <glm-model>`.
Explicit spawn model flags win.

Seats live in `plugins/lanista/seats.json`; edit the repo, not host copies.
Vegad fetches the catalog on ganymede and distributes it. Do not fetch on other
hosts. Inspect `catalog_sync` for stale data. For catalog queries, use lanista's
own `pareto`, `profiles`, or `pick` commands.
