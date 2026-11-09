# vega uac

Coordinator mode: one session (VEGA, the UAC) runs the user's other agent sessions (sentinels) on a project across
hc terminals and herdr TUI panes, so the user talks to one agent only. The plugin carries the role guide and the home view of
the agents this session runs, with what needs its attention.

`unread_mail` shows the relay's oldest mail age, `pending` when mail has no valid
age, or `unknown` without mail metadata. Unknown does not prove an empty inbox;
inspect `vega post log --pane <pane>` on the owning host for broker read status.

Commands: `vega uac`, `claim`, `release`, `adopt`, `ask`, `status`, `answer`, `land`, `prune`,
`grant`, `revoke`, `grants`, `group`, `park`, `hold`, `unpark`.

`vega uac hold <pane>` pauses attention without a decision bead. Resume with
`vega uac unpark <pane>`; new work from the parent also clears a hold.

Done reports arrive by mail without a duplicate attention wake. Silent gate/CI
waits, parked children and closed hc terminals remain quiet. Spawn briefs request
only done, blocked, or requested CI verdict mail, without acknowledgement or progress.

On the receiving vegad host, `vega uac grant deimos:w15W:p2 proteus:w3V:p1 --expires 2h`
allows exactly one inbox-only peer pair until expiry. `revoke` takes the same pair; `grants`
and the home view list live, expired and revoked records. The sender pane is its host's own
claim; replies are peer requests, never Slayer approval. See `grant --help` and
[quarantine](../../docs/design/vegad.md#quarantine-inbox-only-replies).

`vega uac land BEAD --branch BRANCH` freezes the reviewed source tip, merges by
project policy, validates the integrated head, verifies its push, installs and
closes the bead and owned sentinel. Resume with `land BEAD --resume`; inspect
phases and residue with `land BEAD --status`. `--keep` retains pane/worktree;
branches and stashes are always retained. See `vega uac land --help` and the
[landing contract](../../docs/design/orchestration.md#uac-landing-contract-vega-lhc16611).

A coordination repo's `.vega/group.toml` lists canonical member project IDs.
`vega uac group add <path|project-id>` adds a member; `group discover <dir>` adds
Git repos directly beneath a directory; `group` lists the members. Claim
registers the same UAC for all members, refusing live conflicting UACs. Release
clears its group records; inbox-only hosts route through their local records.
The facility table includes member agents and their project IDs. Land member
work with `land BEAD --repo <member-main-checkout> --branch BRANCH`; pass the
same `--repo` for resume/status. Beads, policy, worktrees and gates stay per repo.

Design: [uac-role.md](../../docs/design/uac-role.md), [uac-rules.md](../../docs/design/uac-rules.md).

Agent guide: `vega skills uac`. `SKILL.md` links to the canonical role guide in
`docs/design/uac-role.md`, whose operational section the plugin embeds as the runtime guide.

`vega uac prune --dry-run` previews ancestry-merged branch and clean paneless
worktree cleanup; `vega uac prune` applies it without force. See `prune --help`.

Project identity uses the flag-day v2 protocol; deployment and exact-grant
requirements are in [the registry cutover handoff](../../docs/design/project-registry-cutover.md).
## Overview JSON (schema 1)

`vega uac --json` selects the project from cwd without requiring a calling pane
or claiming a UAC. It emits `{schema: 1, project: string|null, agents: [...],
grants: [...], unreachable: [string], registry_error: string|null}`. Project
membership includes configured group members. Agent rows have `project`,
`machine`, `pane`, `name`, `tab`, `kind`, `status`, `attention` (string|null),
`parent`, `mode`, `bead`, `unread_mail`, `tier` and `model` (string|null), and
`ci` (target labels). Grant rows are receiving-host mail pair grants with
`sender`, `target`, `status` (`live`, `expired`, `revoked`), `created`, `expires`
(Unix seconds), and `revoked` (Unix seconds|null). Registry errors retain local
rows; unreachable machines are explicit. Reads do not write a UAC status report.

Errors use `{error: {code, message, hint}}` (exit 2 usage, 1 runtime).
Empty lists are arrays; TOON remains the default. Consumers should ignore
future additive fields.
