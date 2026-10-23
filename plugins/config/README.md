# vega config

Owns the config of the coding agents, herdr and dcg: CLAUDE.md, AGENTS.md, output styles, Claude settings keys, agent
hooks, global skills and herdr's config.toml. Each file is rendered from the repo default in `config/` or a local
override in `~/.config/vega/config/`, so one source drives every host. Shared files are key-merged: only vega's
keys change.

Commands: `vega config`, `diff`, `apply`, `print`, `init`, `infra`.

`dcg.config` and `dcg.packs` also manage Destructive Command Guard's config.toml and
custom YAML packs, with per-file local overrides. `allowlist.toml` remains user-owned.
Preview with `vega config diff dcg.config dcg.packs`; the default blocks direct branch deletion
with guidance to use `vega uac prune` or `wt remove`. `-D` remains blocked.

`infra` requires explicit endpoint configuration for optional NATS and Dolt;
Tailscale alone does not configure them. Preserve existing host addresses before
upgrading; environment variables override the local first-line files documented
in [Service endpoints](../../docs/design/config.md#service-endpoints).

Design: [config.md](../../docs/design/config.md).

Agent guide: `vega skills config`
