---
name: vega-config
description: Shows, diffs and applies the coding agents', herdr's and dcg's config that vega owns (instructions, output styles, settings, hooks, global skills, herdr config, dcg config and packs), rendered from repo defaults or local overrides in ~/.config/vega/config. Use when these instructions, hooks or configs drift, to change them, or to check what a deploy would write.
---

# vega config: agent, herdr and dcg config from one source

```
vega config                  # per unit: target, ownership, state; local sources
vega config diff [UNIT...]   # what apply would change, as unified diffs
vega config apply [UNIT...]  # write; refuses units chezmoi still manages
vega config print UNIT       # the rendered file of a single-file unit
vega config init SOURCE      # copy a repo default to ~/.config/vega/config/ to edit it
vega config infra            # the infra vega needs: required/optional, state, next step
```

- Units: herdr, claude.md, codex.md, pi.md, claude.styles, claude.settings, claude.hooks,
  codex.hooks, pi.prefix, skills, agentsview, dcg.config, dcg.packs. Groups: instructions, hooks.
- `dcg.config` and `dcg.packs` replace config.toml and their YAML pack targets;
  oc.yaml's repo default was imported byte-for-byte from the existing pack.
  `allowlist.toml` stays user-owned. Preview: `vega config diff dcg.config dcg.packs`.
  Direct deletion flags get a `vega uac prune` / `wt remove` reason; dcg also uses it
  for extracted inline-shell bodies. Other core denials stay intact.
- Sources: a file in `~/.config/vega/config/` wins over the repo default in `config/` (same
  layout, e.g. `instructions/vega.md`, `claude/settings.json`, `herdr/config.toml`). Edit the
  source, never the target: apply replaces whole files (old copy in `<file>.vega-bak`).
- Shared files (`settings.json`, `hooks.json`) are key-merged: only vega's keys and hook entries
  change. `claude.settings` keys are seeded: a local switch (e.g. outputStyle) stays until the
  source value changes.
- `managed (chezmoi)`: the dotfiles still own the target; apply leaves it until they drop it.
- `vega deploy apply` runs `vega config apply` on every host; install-cli.sh applies only `hooks`.
- `infra` checks locally (no network): mise, cargo, git, vega, herdr and an agent are required;
  NATS, tailnet, Dolt, bd, gh and the rest are optional. `vega onboard` walks through it.

Service endpoints are explicit local config, outside the rendered config units.
NATS needs `VEGA_NATS_URL` or `vega/nats.url`; Dolt needs `VEGA_BD_REMOTE` or
`vega/bd.remote`, and database creation also needs `VEGA_BD_SQL` or `vega/bd.sql`.
Files are first-line values under `$XDG_CONFIG_HOME` (normally `~/.config`);
environment overrides win. A tailnet alone does not configure either service.
Janitor embeddings similarly need `VEGA_JANITOR_EMBED_ENDPOINT` or
`vega/janitor-embed.url`. Before upgrading hosts, preserve their existing
endpoints in these files or their environment overrides.
