# vega docs

## Start here

- [README](../README.md): what vega is, the plugin and shim inventory, install, onboarding with an agent, upgrades.
- [Concepts](concepts.md): hosts, panes, agent kinds, UAC and sentinels, projects and their identity, mail,
  vegad, beads, the gate, deploy, config, secrets and the trust model, with one diagram.
- In a shell: `vega` (plugins), `vega skills` (agent guides), `vega <plugin> --help` (reference).

## How-tos

- Install, onboard a host, upgrade: [README](../README.md#install), `vega onboard`, `vega upgrade --help`.
- Per plugin: `plugins/<name>/README.md` (linked from the [README inventory](../README.md#what-is-in-it)) and
  `vega skills <name>`.
- Shims (`bd`, `git`, `gh`, `herdr`, `tea`): [shims/README.md](../shims/README.md).
- A local agentsview sidecar: [config/agentsview/LOCAL-SETUP.md](../config/agentsview/LOCAL-SETUP.md).
- What changed per version: [CHANGELOG.md](../CHANGELOG.md).
- Writing agent skills: [skill_best_practices.md](skill_best_practices.md).

## Design docs (background)

Decision records and research, written while building vega. They explain why; the CLI guides and `--help` are
the current reference. Some describe proposals, research or one-time cutovers; check a doc's header for its status and date.

| Doc | About |
| --- | --- |
| [agent-tooling-principles.md](design/agent-tooling-principles.md) | Rules for every vega CLI (AXI and vega's deviations) |
| [repo-layout.md](design/repo-layout.md) | Where plugins, crates, shims, config and skills live |
| [vega-mux-mvp.md](design/vega-mux-mvp.md) | The first plan for agent-to-agent messaging over herdr |
| [vega-mux-battletest.md](design/vega-mux-battletest.md) | Field test of vega mux with Codex and pi |
| [vega-mux-broker-spike.md](design/vega-mux-broker-spike.md) | Spike: NATS JetStream as the post office |
| [vegad.md](design/vegad.md) | The per-host daemon: delivery, wake-ups, registry, local mode, security |
| [pane-control.md](design/pane-control.md) | Steering TUI panes through herdr: dialogs, answers, folder trust |
| [mux-next.md](design/mux-next.md) | `vega mux next`: focus the agent waiting longest, on any host |
| [uac-role.md](design/uac-role.md) | The UAC coordinator guide (`vega skills uac`) |
| [uac-rules.md](design/uac-rules.md) | Generic coordinator rules: briefs, branches, landing, cleanup |
| [orchestration.md](design/orchestration.md) | Waiting evidence and supervised coordination |
| [project-identity.md](design/project-identity.md) | A project is its normalized git fetch remote |
| [project-registry-cutover.md](design/project-registry-cutover.md) | Registry, UAC, post and pins move to project IDs |
| [project-local-sync-cutover.md](design/project-local-sync-cutover.md) | Per-location bd and git sync state |
| [project-local-consumers-cutover.md](design/project-local-consumers-cutover.md) | The remaining local consumers of project IDs |
| [beads-sync.md](design/beads-sync.md) | `vega bd init`; vegad syncs each project's beads over a Dolt remote |
| [git-sync.md](design/git-sync.md) | vegad fetches and fast-forwards project checkouts |
| [ci-watch.md](design/ci-watch.md) | One CI poller per host; verdicts by mail |
| [gate.md](design/gate.md) | LLM review before an agent's push |
| [live-review.md](design/live-review.md) | Agent and owner review a running web app together |
| [live-review-research.md](design/live-review-research.md) | Research behind live review (agent-browser) |
| [board-ui.md](design/board-ui.md) | Layout of `vega board` |
| [herdr-attention-ui.md](design/herdr-attention-ui.md) | Study: an attention popup in herdr (proposal) |
| [config.md](design/config.md) | vega owns agent, herdr and dcg config; service endpoints |
| [onboarding.md](design/onboarding.md) | The infra model, `vega onboard`, the changelog |
| [upgrade.md](design/upgrade.md) | vegad notices a newer vega; `vega upgrade` installs it |
| [secrets.md](design/secrets.md) | Per-host age secrets, injected per tool |
| [capacity.md](design/capacity.md) | Subscription headroom from token-burn |
| [lanista-catalog.md](design/lanista-catalog.md) | The LLM model catalog, built once and distributed by vegad |
| [pagerduty.md](design/pagerduty.md) | One vegad polls PagerDuty; the owning UAC investigates |
| [janitor.md](design/janitor.md) | A map of every git checkout on a machine |
| [theme.md](design/theme.md) | One dark/light mode for every TUI |
| [theme-symbols.md](design/theme-symbols.md) | Symbols of the UAC pane label and the board |
| [skills-management.md](design/skills-management.md) | Research and options for managing skills |
| [agentcraft-research.md](design/agentcraft-research.md) | Research: AgentCraft, not used as a building block |
| [going-public.md](design/going-public.md) | The audit before publishing this repository |
