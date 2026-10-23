# vega

Agent tooling: the `vega` CLI, its plugins, shims and skills.

## Principles

AXI (skill `axi`) applies to every CLI here, with the deviations in
`docs/design/agent-tooling-principles.md`. Where they conflict, that doc wins. Notably:

- No SessionStart hook for discovery: the global `vega` skill plus `vega skills` replace AXI 7.
- Skills are stubs; the guides are served at runtime by the CLI (`vega skills` index, `vega skills <name>` one guide,
  read from `vega-<name> --skill`), not generated into `SKILL.md`.
- Every plugin's `--skill` prints SKILL.md frontmatter (`name: vega-<name>`, third-person what + when description)
  plus a concise body; a test per plugin checks it with `vega_plugin::assert_guide`.
- Shims for known CLIs keep the original stdout and exit codes; they only add commands and `stderr` hints.
- Guides and help texts over 15 lines live in .md files (`plugins/<name>/SKILL.md`, `plugins/<name>/help/<cmd>.md`),
  embedded with `include_str!`; `scripts/inline-md.test.sh` checks it.

## Install

Requires [mise](https://mise.jdx.dev/installing-mise.html). Check with `mise --version`. If it is missing, ask the
user whether you shall install it (see the link above); do not install it unasked. Then run `mise run install`.

## Build

- `mise run check`: fmt, clippy (`-D warnings`), tests.
- `mise run install`: installs binaries, shims (agent-only dir) and the global skill link.

## Work tracking

Beads (`bd`) is the backlog; `bd prime` prints the rules (`.beads/PRIME.md`). The database is local only (no
Dolt remote) and lives in the main checkout; worktrees find it through git.
