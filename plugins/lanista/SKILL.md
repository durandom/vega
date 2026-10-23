---
name: vega-lanista
description: Installs and upgrades the lanista LLM model catalog from git, ships its seats allowlist from the vega repo, and returns the model id to start per provider lane and tier. Use when choosing a model for an agent, when lanista is missing, outdated or unhealthy on a machine, or when editing which models a host can start.
---

# vega lanista: the model catalog

lanista (github.com/b4arena/lanista) is the Slayer's catalog of LLMs with prices and benchmarks. vega owns its lifecycle; for queries (`lanista pareto`, `profiles`, `pick`) run `lanista` itself.

- `vega lanista`: installed commit against git main, catalog age and source host, its sync state, seats file state, and global seat pins per lane and tier. Use `pin` for project resolution.
- `vega lanista install [--ref <ref>]`: `uv tool install` from git at the commit the ref names (default main), then writes the seats file. Idempotent; `scripts/install-cli.sh` runs it, so `vega deploy` keeps machines current. An editable install (a dev checkout) is left alone: it says what it would do; `--replace` replaces it.
- `vega lanista pin <provider> [--tier flagship|balanced|budget]`: resolve inside the target project: project override, then global seats; `source` names the writer host or global seats. Use the returned model for gate or mux harness arguments.
- Add `--set <model> [--until 2h|never]` to share a runtime project override across hosts. Default expiry is the binding quota reset; unknown/stale quota requires explicit `--until`. `--clear` returns to global seats. Registry read failure says it fell back; writes fail. Inbox-only hosts need exact read grants plus `VEGA_LANISTA_PIN_HOSTS=host1,host2`, otherwise they use global seats.
- `vega lanista doctor`: lanista's health checks.

Seats: `plugins/lanista/seats.json` in the vega repo is the source. Install writes it to `~/.config/lanista/seats.json` and backs a differing file up once to `seats.json.vega-bak`. Edit the repo copy, never a host's. Each pin's `tier` decides `pin`; lanista's profiles cannot, as models without a benchmark score drop out of them.

Catalog: vegad on the builder host (ganymede; `VEGA_LANISTA_BUILDER` or `~/.config/vega/lanista-builder` override) runs `lanista fetch` daily with the lanista secrets and puts the index into the broker; every other host's vegad copies it within 15 min. `catalog` in the home view shows age and source host, `catalog_sync` what vegad last did. Do not run `lanista fetch` on other hosts; a stale catalog means the builder's vegad needs a look. By hand on the builder: `vega secrets run --for lanista -- lanista fetch`.

Migration/recovery: `vega post identity --help`.
