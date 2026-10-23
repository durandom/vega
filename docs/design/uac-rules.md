---
name: vega-uac-rules
description: Generic coordinator rules for VEGA in uac mode, on briefing sentinels, branches, landing and cleanup. Use when the uac guide says so, before briefing a sentinel or landing its work. A project's own coordinator rules add to these and win where they differ.
---

# UAC rules

## Landing

- A project's recorded merge authority (rules file, epic notes) wins; else land by size and
  risk (uac guide).
- Land only on a green gate of the current head. Never override a red gate without reading each
  red job and knowing its cause is external.

## Branches

- Create worktrees only with `vega mux spawn --worktree` or `wt switch --create`, never
  `git worktree add`: the project's wt hooks must run, or commit hooks fail.
  A brief allowing a sentinel more worktrees says so too.
- Never rebase pushed history; merge the base in instead.
- Merge the base only for a conflict or a landed dependency, and once before the final gate.
- Stacked work: a child may start from its implemented parent's branch. After
  the parent lands, merge the base into the child, rerun the gates it touched, then land it.

## Sentinels

- A brief is self-contained (later context gets lost): target, scope, constraints, ownership,
  observable done-when.
- Coordinate; do not debug inside a lane. Route a failure: an external blocker (record it,
  park the work), a missing dependency (fix the order), a decision (ask the Slayer;
  `vega uac park <pane> --bead <id>`).
- Repeated special cases of one root problem show a design gap: stop the exceptions, park the
  work (neutral commit, handoff note), record a design item, plan it with the Slayer.
- A pane is not the board: the Slayer may never see it.
- Spawn/adopt: headed (dialogs stay) unless `adopt --headless` or `spawn --headless`.
  Flip: `vega mux mode`.
- Never let a sentinel raise timeouts, weaken or delete assertions, or change CI resources to get a
  gate green. No synthetic load on shared hosts. Never print secrets or env files.

## Backlog

- Your sentinels' beads carry `uac-lane`; without beads, the handover file lists their follow-ups.
- A growing lane backlog: a triage sentinel proposes close, merge or keep per group, with reasons,
  changing nothing. One `slayer-decision` per group; apply the answers. Label, never reparent.

## Cleanup

- When a lane finishes, stop its servers and close its panes. Before removing a
  worktree, copy out the gitignored evidence an open item cites (logs, reports; not build
  output).
- Before ending, write a handover: open work with branch, worktree and PR, running
  sentinels, decisions for the Slayer with your recommendation, unpushed work, what is
  safe to remove.
