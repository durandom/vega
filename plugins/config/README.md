# vega config

Owns the config of the coding agents, herdr-app and dcg: CLAUDE.md, AGENTS.md, output styles, Claude settings keys, agent
hooks, global skills and herdr-app config. Each file is rendered from the repo default in `config/` or a local
override in `~/.config/vega/config/`, so one source drives every host. Shared files are key-merged: only vega's
keys change.

`claude.hooks` installs the minimal `vega-hooks@vega-local` plugin through Claude Code's CLI in user
scope. It removes legacy vega settings hooks after verifying the plugin; restart running Claude sessions.

The `herdr-app` group installs checked Vega status, sidebar and inbox mods plus the
maintained layout. `vega-sidebar` (a clone of the shipped agents mod) is the only sidebar list: UAC
projects across machines in dragged order, then spaces without a UAC, with clickable badges.
The non-modal inbox shows decisions and waiting panes; only new decisions auto-open it.
Doctor includes local hc, herdr-app and Python 3 availability.
The inbox also reads badges from connected hc machines and executes answers and
dismissals in their recorded remote checkout. Python 3 is required on both ends;
read failures show a short status-line error only after three consecutive failures
per source, without hiding local or other hosts' cards. Failed sources keep their
last good cards until the original 60-second expiry; a failed `hc machine ls`
falls back to the last cached machine inventory.
`infra` flags a leftover `vegad-start` login job (`vega post relay install` removes it);
runtime daemon state remains in `vega post relay status`.

Commands: `vega config`, `diff`, `apply`, `print`, `init`, `infra` (`check`).

`mise run install-dcg` installs checksum-pinned dcg 0.16.0, backing up the old binary
and restoring it if strict doctor fails. `vega config check` reports dcg version and
Claude/Codex/pi guard registration for agents on PATH. `claude.settings` and `codex.hooks`
merge dcg entries from `dcg/hook.json`; `pi.dcg` in the `hooks` group manages
`~/.pi/agent/extensions/dcg-guard.ts`. Preview the diff and restart agents after apply.

`infra` also warns above 50,000 entries in this checkout's `target/debug/deps`;
its directory scan stops at 50,001. `mise run target-sweep` performs bounded
maintenance between builds; see [build-cache.md](../../docs/design/build-cache.md).

The package also ships `vega-project`: `vega project config` reads/edits personal
project display names and labels, and `vega project view` manages named sidebar
filters in `$XDG_CONFIG_HOME/vega/projects.toml`. These runtime preferences are
separate from deployable config sources and are never applied from the repository.
See [the sidebar design](../../docs/design/herdr-app-ui.md).

`vega config infra --fleet` adds an explicit network check: every hc machine name
must exist as a vega registry host. Mismatches exit 1 and suggest
`hc machine rename <old> <host>`; no machine is changed. Empty fleets are reported
without contacting the registry. Plain `infra` and onboarding remain local-only.

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
