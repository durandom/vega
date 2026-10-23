# vega skills

Serves the agent guides of all vega plugins at runtime, so the skills stay stubs and each guide lives with its
plugin's code (`vega-<name> --skill`). It also manages the personal skills library: show, install, remove and
vendor skills, and check installs for drift.

Commands: `vega skills`, `vega skills <name>`, `vega skills lib`.

Design: [skills-management.md](../../docs/design/skills-management.md),
[agent-tooling-principles.md](../../docs/design/agent-tooling-principles.md).

Agent guide: `vega skills skills`
