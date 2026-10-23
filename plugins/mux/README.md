# vega mux

Lets coding agents work with each other through herdr panes, on this machine or on other herdr machines. An agent
lists the others, reads what they said, messages them, answers their dialogs and spawns new agents, also in their
own git worktree. It is how the UAC runs its sentinels.

Commands: `vega mux`, `show`, `transcript`, `send`, `dialog`, `answer`, `spawn`, `close`, `reopen`, and for the Slayer `next`
(the agent that has waited longest, on any host). There is no default vega key binding for `next`;
use herdr's goto overlay (`prefix+g`, then `d`) to list done agents across machines and switch to one.

`spawn` records your pane as the agent's parent: its brief carries the done-report footer, and vegad and `vega uac`
track it. `spawn --no-parent` records none and sends the brief with the origin line only, for an agent that reports
by its own command (a `vega gate` reviewer gives its verdict with `vega gate verdict`).

Spawn defaults to headed: the user answers questions in the pane. Add `--headless`
for unattended children whose questions go to their parent. Every spawn records
the mode before startup, including `--no-parent` and a startup stopped at a dialog;
`--resume` preserves it. Older children without a valid mode record stay headless.
Inbox-only targets cannot mail the parent; their brief asks the user in either mode.
Codex's non-blocking `request_user_input_async` follows the mode too: a headless
child is told to mail the parent instead; a headed child keeps the question in its
bar for the user, and the parent gets a copy by mail with the reply command
`vega post send <child> "<answer>"`. That reply arrives when the child's turn ends
(the pane is `blocked` while the question is queued); the user can answer in the
bar at any time.

`vega mux reopen <name|pane>` respawns a closed Claude or Codex agent in a new
workspace using its recorded cwd, parent, mode and session. The cwd can be a
worktree or a plain directory; keep it after closing if you want to reopen.
Spawn saves a durable record and captures herdr's session ID when available;
close refreshes the ID before dropping supervision records. Missing session IDs,
removed directories, running names and unsupported kinds return structured errors.
Reopen is local only and preserves extra harness args, but does not store or replay
explicit `--env` values. `spawn --resume <pane>` still means deliver a saved brief
to an existing pane after the user has answered its startup dialog.

Spawns forward the caller's `SSH_AUTH_SOCK` when set to the agent process. An explicit
`--env SSH_AUTH_SOCK=...` wins, including an empty value. This covers every agent kind and placement, including worktrees. For
`--machine`, use `--env SSH_AUTH_SOCK=<remote-path>` if the socket path differs on the target host.

Codex spawns also pass `-c shell_environment_policy.set.SSH_AUTH_SOCK="<socket>"`
so tool shells keep the socket even with project `inherit = "core"`. The value comes
from the caller, with `--env SSH_AUTH_SOCK=...` taking precedence (including an empty
value). Without either value, no socket config is added. Explicit Codex `-c` / `--config`
arguments after `--` come last and win. The dotted key merges with project config,
preserving the rest of `shell_environment_policy`; an `include_only` allowlist must
still retain `SSH_AUTH_SOCK`. No socket path is hardcoded. Manually started Codex
panes are unchanged and need their own environment policy configuration.

Codex spawns also set the herdr pane identity vega tools read (`vega ci watch`, the gate,
`vega orchestration`): `-c shell_environment_policy.set.HERDR_PANE_ID="<new pane>"` and, on a
local spawn with `HERDR_SOCKET_PATH` set, `...set.HERDR_SOCKET_PATH`. `--env HERDR_PANE_ID=...`
and explicit Codex `-c` arguments win the same way.

Codex spawns default to `FORCE_COLOR=1` (basic ANSI colours), so the composer uses
terminal defaults and stays readable after a live light/dark appearance switch. Codex caches
its startup background probe; see [openai/codex#54159](https://github.com/openai/codex/issues/54159#issuecomment-6294011108).
An explicit `--env FORCE_COLOR=...` wins, including an empty value. This applies to every
spawn location, including worktrees and remote machines.

Design: [vega-mux-mvp.md](../../docs/design/vega-mux-mvp.md), [pane-control.md](../../docs/design/pane-control.md),
[mux-next.md](../../docs/design/mux-next.md).

Agent guide: `vega skills mux`
