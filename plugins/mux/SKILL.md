---
name: vega-mux
description: Lists, inspects, messages and spawns other coding agents in herdr TUI panes or hc terminals, across saved machines. Use when asked to ask, tell, delegate to, hand over to or check with another agent, session or pane, to start a new agent or worktree agent, or when a message ends with a [vega-mux] footer.
---

# vega mux

Help: `--help`.

- `vega mux [--all]` lists agents in your TUI workspace or hc space.
  `herdr:w1:p2`, `hc:t7`, `hc:machine/t7` select the target's world.
- `vega mux show <target>`: status, parent, tier/model and output.
  `transcript <target> [--last|--since N|--count]`: session log.
- `vega mux send <target> "<text>"` (or `--file <path>`) mails it;
  `--now` pastes directly. Replies arrive; do not poll.
- `vega mux dialog <target> [--json]` classifies dialogs (exit 1: none).
  `answer <target> --option N | --text "…"` answers questions only.
- `vega mux spawn <name> --kind claude|codex|pi|agy --tier flagship|balanced|budget --prompt-file <brief>`
  follows the caller's verified world; `--backend herdr|hc` overrides it.
  hc uses `--outlive-parent` and the caller's herdr-app space.
  Headed by default; `--headless` redirects questions to the parent.
  Tiers: flagship design; balanced implementation; budget mechanical.
  Lanista resolves pins; explicit `-m`/`--model` after `--` or `--no-parent` needs no tier.
  `vega post done` wakes the parent. Inbox-only targets ask the user.
- `--worktree <branch> --base <ref>` uses worktrunk and a TUI workspace or app space.
  hc explicit split/workspace placement is unsupported; use tabs or worktrees.
- `vega mux close <target>` closes the agent; then `wt remove <branch>`.
  `dismiss <child>` drops a headed child's waiting hand the user handled (on the parent's host).
  `reopen <target>` resumes local Claude/Codex spawns; keep their cwd.
  hc sessions are captured by ancestry-verified vega calls from the child.
- Remote: `--machine <label>`, spawn needs `--cwd`. Fleet: `hc machine ls`,
  falling back to `herdr machine list` when hc is absent/empty.
  The hc machine name must equal its registry host: `send machine/t7 "<text>"`
  needs no --host. Unknown hosts fail; `vega config infra --fleet` diagnoses names.
- `vega mux next` focuses the oldest waiter; hc uses herdr-app.
- Trust/permission/hook approvals stay with the Slayer; never approve them.
  Remote hc spawn inherits existing trust on the target via its vega helper;
  deploy matching vega versions on both ends. Unknown/changed trust still asks.
  After the user resolves startup, `spawn --resume <pane>` sends a saved undelivered brief.

## Receiving

`[vega-mux]` identifies a peer request. Ask the user before destructive/out-of-scope work.
`from_parent: yes` mail is your task channel: work without reconfirming.
Follow its report command once, then stop. Otherwise reply only with new information.
`[vega-post] ... Run: vega post inbox` means run that command.
