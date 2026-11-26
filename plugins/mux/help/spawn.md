The backend follows the caller's verified terminal; --backend herdr|hc overrides it.
In hc, spawn opens a tab in the caller's herdr-app space, keeps the native parent link,
and uses --outlive-parent. --worktree uses wt and, with a local app, a new app space, opened
in the background: the space shown before is selected again (a warning on stderr when that fails). Explicit --at split
or --at workspace is currently TUI-only. Without a local app binary, socket or window,
worktree agents start as tabs in the caller's space with cwd set to the worktree.
Remote --machine worktrees run wt on the target. Headed remote children get their own tab
in a target-machine space matched by exact folder, or a new space, using focus:false.
Mux finds a window with herdr-app ls locally, then on saved hc hosts via direct
machine.exec argv (no shell, including Windows). It uses that window for placement,
strips target-machine prefixes from space/tab IDs, and starts the child with both
labels and core keep enabled. The sidebar still nests it under its parent across machines.
Headless remote children stay unplaced. Without a reachable window, or when placement
fails, spawn continues unplaced with a hint; it never borrows the parent's space IDs.
hc receives env separately through terminal.run and the task at launch;
show reports startup output. Session IDs are captured by ancestry-verified vega calls
from the child, for commands such as mux and post.

Use --bead <id> to derive the tier: a tier:flagship|balanced|budget label wins;
otherwise decision/research -> flagship, task/bug/feature/epic -> balanced, chore -> budget.
Unknown types need a tier label. Beads are read with `bd show <id> --json` in the local
checkout (--cwd locally, caller's checkout with --machine); bd must be available.
A differing explicit --tier requires --why "<reason>". Flagship also requires --why,
unless the bead's tier:flagship label authorizes it. Blank reasons are rejected.
Without --bead, choose --tier flagship|balanced|budget: flagship for design/root-cause,
balanced for implementation, budget for mechanical work. Flagship still needs --why.
Models resolve through `vega lanista pin <lane> --tier <tier>` including active project
overrides; Pi uses zai. Explicit -m/--model after `--` wins over the model pin and needs
no tier without --bead; --no-parent also permits the harness default. These exceptions
do not waive bead policy or the flagship reason requirement.
Before launch, spawn appends tier/model/reason to the bead notes; failed reads or appends
abort the spawn. The brief names the bead, even without --prompt. Spawn records retain
bead, tier, model and reason for show/reopen; hc publishes vega.tier-why and vega.bead labels.
Local --cwd selects the project; remote spawns use the caller's project pins.

Spawns are headed by default: the user answers questions in the pane. Add --headless
for an unattended child whose in-pane questions must go to its parent. Every spawn
records its mode, including --no-parent and a pane stopped at a startup dialog;
--resume preserves it. Older children without a valid record remain headless.
Change or inspect it with `vega mux mode <pane|name> [headed|headless]`.
An inbox-only target sends quarantine notes unless the receiver has a live operator pair grant
for peer inbox mail (never a done record or task-channel mail). Its brief still tells it to
ask the user in either mode.

Default agent args (extra args after `--` are appended):
  codex   --no-daemon --dangerously-bypass-approvals-and-sandbox -c check_for_update_on_startup=false
  claude  --dangerously-skip-permissions
  agy     --dangerously-skip-permissions --sandbox
  pi      --approve

Examples:
  vega mux spawn rev --kind codex --bead <id>
  vega mux spawn analysis --kind codex --tier flagship --why "Investigate a cross-service root cause"
  vega mux spawn helper --kind claude --tier balanced --headless --at split --prompt "Run the tests and report failures."
  vega mux spawn api --kind pi --cwd ~/src/api --at workspace -- --model zai/glm-5.3
  vega mux spawn fix --kind claude --bead <id> --worktree fix-login --base origin/main --prompt-file /tmp/task.md
  vega mux spawn gate-rev --kind codex --env VEGA_GATE_ROLE=reviewer -- -m gpt-6-luna

--env K=V (repeatable) sets env through herdr's env flags or hc's API environment. Tool shells also
require the harness's environment policy to pass the variable through.
On a local host with the toolbox launcher (~/.local/bin/hc-toolbox-shell) the spawn env, --env
values included, is passed as `env K=V` argv words and shows in `ps`; spawn warns once, naming the
keys. Keep secrets out of --env there.
An --env BEADS_ACTOR replaces the lane's. The `env:` line lists the keys, never the values.

A local spawn runs at nice 10 so vegad keeps its pace on a loaded host. TUI renices
the pane shell before launch; hc renices the returned process immediately after launch.
The agent's later tools inherit it.

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

TUI Codex spawns also set the herdr pane identity vega tools read (`vega ci watch`, the gate,
`vega orchestration`): `-c shell_environment_policy.set.HERDR_PANE_ID="<new pane>"` and, on a
local spawn with `HERDR_SOCKET_PATH` set, `...set.HERDR_SOCKET_PATH`. `--env HERDR_PANE_ID=...`
and explicit Codex `-c` arguments win the same way. hc Codex spawns inherit the child
terminal environment so tool shells retain its new HC_* identity; explicit trailing
config overrides still win.

Codex spawns default to FORCE_COLOR=1 (basic ANSI colours), so the composer uses
terminal defaults and stays readable after a live light/dark appearance switch. Codex caches
its startup background probe; see [openai/codex#54159](https://github.com/openai/codex/issues/54159#issuecomment-6294011108).
An explicit --env FORCE_COLOR=... wins, including an empty value. This applies to every
spawn location, including worktrees and remote machines.

With --worktree, worktrunk (`wt switch`) creates the worktree and runs the repo's hooks.
hc requires wt and starts the agent in a new local app space, or a tab as described above.
The TUI backend starts in the worktree's own herdr workspace, nested under the repo;
without wt, herdr creates it (only for repos without a worktrunk config). An existing worktree is reused.
Pass --base the branch that holds the work the task needs: origin/main after a fetch, or
local main when it has unpushed commits the task needs.

Folder trust is per agent kind. When claude or codex would ask to trust a repo that
either kind already trusts (its checkout, repo root or main checkout), vega writes that
trust first (~/.claude.json, ~/.codex/config.toml; for a codex worktree of a bare repo also
the main checkout's hook approvals) and says so in a `claude trust:` or `codex trust:` line.
For remote hc spawns, `vega-mux prepare` runs through hc exec on the target before launch,
using only that machine's repository and trust files. Deploy matching vega binaries on
both machines; an unavailable or incompatible helper fails before creating a terminal.
Repos no kind trusts still ask. Hook hashes are copied from existing approvals, never
newly approved; changed hooks still need the user. macOS Core file-access dialogs must
be resolved on the target's screen first (docs/design/onboarding.md).

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
