Spawns are headed by default: the user answers questions in the pane. Add --headless
for an unattended child whose in-pane questions must go to its parent. Every spawn
records its mode, including --no-parent and a pane stopped at a startup dialog;
--resume preserves it. Older children without a valid record remain headless.
Change or inspect it with `vega mux mode <pane|name> [headed|headless]`.
An inbox-only target cannot mail its parent: its brief tells it to ask the user
in either mode.

Default agent args (extra args after `--` are appended):
  codex   --no-daemon --dangerously-bypass-approvals-and-sandbox -c check_for_update_on_startup=false
  claude  --dangerously-skip-permissions
  agy     --dangerously-skip-permissions --sandbox
  pi      --approve

Examples:
  vega mux spawn rev --kind codex
  vega mux spawn helper --kind claude --headless --at split --prompt "Run the tests and report failures."
  vega mux spawn api --kind pi --cwd ~/src/api --at workspace -- --model zai/glm-5.3
  vega mux spawn fix --kind claude --worktree fix-login --base origin/main --prompt-file /tmp/task.md
  vega mux spawn gate-rev --kind codex --env VEGA_GATE_ROLE=reviewer -- -m gpt-6-luna

--env K=V (repeatable) sets env on the agent's pane through herdr. Tool shells also
require the harness's environment policy to pass the variable through.
An --env BEADS_ACTOR replaces the lane's. The `env:` line lists the keys, never the values.

A local spawn runs at nice 10 (the pane shell is reniced before the agent starts), so vegad
keeps its pace on a loaded host; the agent's tools inherit it.

Spawns forward the caller's SSH_AUTH_SOCK when set to the agent process. An explicit
--env SSH_AUTH_SOCK=... wins, including an empty value. This covers every agent kind and placement, including worktrees. For
--machine, use --env SSH_AUTH_SOCK=<remote-path> if the socket path differs there.

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

Codex spawns default to FORCE_COLOR=1 (basic ANSI colours), so the composer uses
terminal defaults and stays readable after a live light/dark appearance switch. Codex caches
its startup background probe; see [openai/codex#54159](https://github.com/openai/codex/issues/54159#issuecomment-6294011108).
An explicit --env FORCE_COLOR=... wins, including an empty value. This applies to every
spawn location, including worktrees and remote machines.

With --worktree, worktrunk (`wt switch`) creates the worktree and runs the repo's hooks;
without wt, herdr creates it (only for repos without a worktrunk config). The agent starts in
the worktree's own herdr workspace, nested under the repo. An existing worktree is reused.
Pass --base the branch that holds the work the task needs: origin/main after a fetch, or
local main when it has unpushed commits the task needs.

Folder trust is per agent kind. When claude or codex would ask to trust a repo that
either kind already trusts (its checkout, repo root or main checkout), vega writes that
trust first (~/.claude.json, ~/.codex/config.toml; for a codex worktree of a bare repo also
the main checkout's hook approvals) and says so in a `claude trust:` or `codex trust:` line.
Repos no kind trusts still ask.

Capacity: `capacity:` is the binding window of the kind's provider from token-burn's
stored sample, its account window with the least left (claude, codex, agy: antigravity, pi: its
--provider, or the provider/ of --model: xai|zai). Under 10% left, or a confident forecast running it out
within 2h, adds a stderr warning; vega never changes the kind. A reset the Slayer planned or an
override (vega capacity) covers it: the line says so and there is no warning.
No token-burn: capacity: unknown. `vega capacity` lists every window.

The brief is typed once the agent has started (codex: its status line leaves `Starting`) and
confirmed: `prompt: taken` means the agent works and its input box no longer holds the brief.
--no-parent records no parent: the brief gets no done-report footer, and vegad and `vega uac`
do not track the agent (for one that reports by its own command, e.g. a gate reviewer).

A spawn that stops at a dialog (trust, hooks review, update, login) keeps its pane and worktree and
says `brief: not delivered`; vega saves the brief. Once the user answered the dialog,
`vega mux spawn --resume <pane>` delivers it with the task footer.
