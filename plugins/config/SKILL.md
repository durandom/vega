---
name: vega-config
description: Shows, diffs and applies the coding agents', herdr-app's and dcg's config that vega owns (instructions, output styles, settings, hooks, global skills, herdr-app config, dcg config and packs), rendered from repo defaults or local overrides in ~/.config/vega/config. Use when these instructions, hooks or configs drift, to change them, or to check what a deploy would write.
---

# vega config: agent, herdr-app and dcg config from one source

With herdr-app running, `vega config apply herdr-app` checks all Vega mods and selects its layout
on first apply. Choices stay yours; status reports clone drift.

```
vega config                  # per unit: target, ownership, state; local sources
vega config diff [UNIT...]   # what apply would change, as unified diffs
vega config apply [UNIT...]  # write; refuses units chezmoi still manages
vega config print UNIT       # the rendered file of a single-file unit
vega config init SOURCE      # copy a repo default to ~/.config/vega/config/ to edit it
vega config infra            # the infra vega needs: required/optional, state, next step
```

- `vega config` lists units; groups: instructions, hooks, herdr-app.
- `dcg.config` and `dcg.packs` replace config.toml and their YAML pack targets;
  `allowlist.toml` stays user-owned. Preview: `vega config diff dcg.config dcg.packs`.
- Sources: a file in `~/.config/vega/config/` wins over the repo default in `config/` (same
  layout, e.g. `instructions/vega.md`, `claude/settings.json`, `herdr-app/keys.json`). Edit the
  source, never the target: apply replaces whole files (old copy in `<file>.vega-bak`).
- Shared files (`settings.json`, `hooks.json`) are key-merged: only vega's keys and hook entries
  change. `claude.settings` keys are seeded: a local switch (e.g. outputStyle) stays until the
  source value changes.
- `claude.hooks` installs/enables `vega-hooks@vega-local` through Claude's CLI in user scope.
  Verified installation precedes legacy hook removal. Restart Claude sessions.
- `managed (chezmoi)`: the dotfiles still own the target; apply leaves it until they drop it.
- `vega deploy apply` runs `vega config apply` on every host; install-cli.sh applies only `hooks`.
- Apply retains 256 redacted results in `$XDG_STATE_HOME/vega/config-evidence.json`:
  phases, targets, operations and presence, never values.
- `infra` warns above 50k deps entries; `mise run target-sweep` between builds.
  `infra --fleet` additionally checks hc names against registry hosts (network);
  mismatches exit 1 with `hc machine rename <old> <host>`. It never renames machines.
- Apply the `herdr-app` group together: checked status, agents and inbox mods precede
  its layout. New decisions open a non-modal inbox; the status row or scale reopens it.

Service endpoints are explicit local config, outside the rendered config units.
NATS needs `VEGA_NATS_URL` or `vega/nats.url`; Dolt needs `VEGA_BD_REMOTE` or
`vega/bd.remote`, and database creation also needs `VEGA_BD_SQL` or `vega/bd.sql`.
Files are first-line values under `$XDG_CONFIG_HOME` (normally `~/.config`);
environment overrides win. A tailnet alone does not configure either service.
Janitor embeddings similarly need `VEGA_JANITOR_EMBED_ENDPOINT` or
`vega/janitor-embed.url`. Preserve host endpoints when upgrading.

`vegad-start` flags an old login job; `vega post relay install` removes it.
