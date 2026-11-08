# vega mux

Lets coding agents work with each other through herdr TUI panes or hc terminals, on this machine or on saved remote machines. An agent
lists the others, reads what they said, messages them, answers their dialogs and spawns new agents, also in their
own git worktree. It is how the UAC runs its sentinels.

`spawn` selects the caller's ancestry-verified terminal world (`--backend herdr|hc`
can override it). hc launches use `--outlive-parent`, the shared harness defaults and
repo trust, a separate API environment map, and the caller's `herdr.space` label.
Remote launches run the target's `vega-mux prepare` helper to inherit its existing
repository trust and hook approvals; matching vega versions are needed on both ends. `--worktree` uses
worktrunk and `herdr-app do space.new`, then selects the space shown before, so the new space
opens in the background (if `herdr-app ls` or the reselect fails, spawn warns on stderr and the new space
stays shown); explicit `--at split|workspace` is TUI-only.
Target commands accept `hc:t7` or `hc:machine/t7` independently of the caller's world.
`--machine` lookup uses `hc machine ls`, falling back to `herdr machine list` when hc
is absent or its fleet is empty. hc remote spawn refuses corporate-to-private control.

Mail keeps machine-qualified addresses intact. The hc machine name must equal its
vega registry host: `vega mux send machine/t7 "<text>"` needs no `--host`.
Post verifies that host exists; unknown hosts or unavailable registry checks fail
without enqueueing. `vega config infra --fleet` diagnoses mismatched names.
`send --now` reaches
that hc address directly and its reply footer names the sender's registry host.

`spawn` and `send` record events through the bounded central
[history outbox](../../docs/design/history.md) on every host, including inbox-only ones.

`show` includes `unread_mail`: the relay's oldest mail age, `pending` without a
valid age, or `unknown` without metadata. Unknown does not prove an empty inbox;
inspect `vega post log --pane <pane>` on the owning host for broker read status.

Commands: `vega mux`, `show`, `transcript`, `send`, `dialog`, `answer`, `spawn`, `close`, `reopen`, and for the Slayer `next`
(the agent that has waited longest, on any host). There is no default vega key binding for `next`;
hc targets focus through `herdr-app do agent.focus`. In the TUI, use herdr's goto
overlay (`prefix+g`, then `d`) to list done agents across machines and switch to one.

`spawn` requires `--tier flagship|balanced|budget` with no default: flagship for
design/root-cause, balanced for normal implementation, budget for mechanical work.
Lanista's project pins resolve the model per kind (Pi uses zai);
`vega lanista pin <lane> --tier <tier>` shows the selected pin. An explicit
`-m`/`--model` after `--` wins and needs no tier; `--no-parent` is also exempt.
Spawn records retain tier and model for `show` and `reopen`; the UAC agent table
shows them too. Local `--cwd` selects the pin project; remote spawns use the caller's project.

`spawn` records your pane as the agent's parent: its brief carries the done-report footer, and vegad and `vega uac`
track it. `spawn --no-parent` records none and sends the brief with the origin line only, for an agent that reports
by its own command (a `vega gate` reviewer gives its verdict with `vega gate verdict`).

Spawn defaults to headed: the user answers questions in the pane. Add `--headless`
for unattended children whose questions go to their parent. Every spawn records
the mode, including `--no-parent` and a startup stopped at a dialog (hc sets the
mode label at launch and saves the durable record when run returns);
`--resume` preserves it. Older children without a valid mode record stay headless.
Inbox-only targets send quarantine notes unless the receiver has a live operator pair grant
for peer inbox mail. Such mail never records done or becomes a parent task channel;
their brief still asks the user in either mode.
Codex's non-blocking `request_user_input_async` follows the mode too: a headless
child is told to mail the parent instead; a headed child keeps the question in its
bar for the user, and the parent gets a copy by mail with the reply command
`vega post send <child> "<answer>"`. That reply arrives when the child's turn ends
(the pane is `blocked` while the question is queued); the user can answer in the
bar at any time.

`vega mux reopen <name|pane>` respawns a closed Claude or Codex agent in a new
TUI workspace or hc tab using its recorded cwd, parent, mode and session. The cwd can be a
worktree or a plain directory; keep it after closing if you want to reopen.
Spawn saves a durable backend record and captures herdr's session ID when available;
hc captures the child's harness session env on an ancestry-verified vega call.
Close preserves that ID before dropping supervision records. Missing session IDs,
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

TUI Codex spawns also set the herdr pane identity vega tools read (`vega ci watch`, the gate,
`vega orchestration`): `-c shell_environment_policy.set.HERDR_PANE_ID="<new pane>"` and, on a
local spawn with `HERDR_SOCKET_PATH` set, `...set.HERDR_SOCKET_PATH`. `--env HERDR_PANE_ID=...`
and explicit Codex `-c` arguments win the same way. hc Codex spawns inherit the child
terminal environment so tool shells retain the new `HC_*` identity, rather than
copying the parent's terminal ID; explicit trailing config overrides still win.

Codex spawns default to `FORCE_COLOR=1` (basic ANSI colours), so the composer uses
terminal defaults and stays readable after a live light/dark appearance switch. Codex caches
its startup background probe; see [openai/codex#54159](https://github.com/openai/codex/issues/54159#issuecomment-6294011108).
An explicit `--env FORCE_COLOR=...` wins, including an empty value. This applies to every
spawn location, including worktrees and remote machines.

Design: [vega-mux-mvp.md](../../docs/design/vega-mux-mvp.md), [pane-control.md](../../docs/design/pane-control.md),
[mux-next.md](../../docs/design/mux-next.md), [hc migration](../../docs/design/hc-migration.md).

Agent guide: `vega skills mux`
