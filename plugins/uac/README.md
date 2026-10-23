# vega uac

Coordinator mode: one session (VEGA, the UAC) runs the user's other agent sessions (sentinels) on a project across
their herdr machines, so the user talks to one agent only. The plugin carries the role guide and the home view of
the agents this session runs, with what needs its attention.

Commands: `vega uac`, `claim`, `release`, `adopt`, `ask`, `status`, `answer`, `land`, `prune`.

`vega uac land BEAD --branch BRANCH` freezes the reviewed source tip, merges by
project policy, validates the integrated head, verifies its push, installs and
closes the bead and owned sentinel. Resume with `land BEAD --resume`; inspect
phases and residue with `land BEAD --status`. `--keep` retains pane/worktree;
branches and stashes are always retained. See `vega uac land --help` and the
[landing contract](../../docs/design/orchestration.md#uac-landing-contract-vega-lhc16611).

Design: [uac-role.md](../../docs/design/uac-role.md), [uac-rules.md](../../docs/design/uac-rules.md).

Agent guide: `vega skills uac`. `SKILL.md` links to the canonical role guide in
`docs/design/uac-role.md`, which the plugin embeds directly.

`vega uac prune --dry-run` previews ancestry-merged branch and clean paneless
worktree cleanup; `vega uac prune` applies it without force. See `prune --help`.

Project identity uses the flag-day v2 protocol; deployment and exact-grant
requirements are in [the registry cutover handoff](../../docs/design/project-registry-cutover.md).
