---
name: vega-onboard
description: Onboards a host to vega and shows what changed after an upgrade. Lists the required and optional infra (mise, cargo, herdr, an agent; NATS, tailnet, Dolt, gh and more) with the next step per gap, applies the default agent config without replacing the user's own files, and lists the changelog entries since this host's last look. Use when vega was just installed or upgraded on a host, or when a vega feature does not work there.
---

# vega onboard: set up a host, catch up after an upgrade

```
vega onboard                   # mode, changes since last seen, infra, what apply would do
vega onboard apply             # write the defaults, record the changelog as seen
vega onboard changes [--all]   # changelog entries since last seen (or all)
```

- Modes: `first run` (never applied here), `upgrade` (changelog blocks newer than the last seen),
  `current`. The view never writes.
- Required gaps first: each row carries its next command. Ask the user before installing anything.
- dcg is recommended: the view checks its binary and Claude/Codex PreToolUse hooks.
  Follow dcg's install/doctor guidance; onboard does not install the binary or register its hooks.
- Optional parts are feature sets (NATS: `vega post` and vegad; Dolt: `vega bd` sync; gh/tea:
  `vega ci`); without them the core still works.
- `vega post relay install` once per host, also without a broker: vegad then runs in local mode
  (sidebar, attention, upgrade proposals typed into the vega session; cross-host mail off).
- `apply`: hooks and skills always (they only add vega's entries); whole files (CLAUDE.md,
  AGENTS.md, output styles, herdr's config.toml, dcg.config and dcg.packs) and settings keys
  only where the user has none. Missing dcg config/packs are written; differing ones are kept.
  A `kept` unit is the user's own: `vega config diff <unit>` shows the default, `vega config apply
  <unit>` takes it.
- Act on `migration` entries in the changes; tell the user about `feature` entries.
- Upgrade: `vega upgrade` (pull, install, moshi-hook, then this view of the new binary).

NATS and Dolt need explicit endpoint configuration; Tailscale alone does not
configure them. Before upgrading hosts, preserve their existing addresses in
`VEGA_NATS_URL` / `vega/nats.url`, `VEGA_BD_REMOTE` / `vega/bd.remote`, and (for
database creation) `VEGA_BD_SQL` / `vega/bd.sql`. Files are first-line values under
`$XDG_CONFIG_HOME` (normally `~/.config`); environment overrides win.
