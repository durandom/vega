Usage: vega uac land BEAD --branch BRANCH [--head SHA] [--target BRANCH] [--keep]
       vega uac land BEAD --resume [--retry-install | --install-done]
       vega uac land BEAD --status

Explicit UAC landing from its target checkout (including bare layouts). Freezes the
source tip; optional --head must be its full SHA. --target overrides project
policy for an integration branch. --keep retains the sentinel pane/worktree.

Requires committed .vega/land.toml on the target: strategy (no-ff or ff-only),
check argv; optional install argv, remote (origin), target (main). Incoming policy
changes cannot affect this landing. Requires gate coverage when the target has
.vega/gate.toml (otherwise reported as not configured on target) and a sentinel
done report or bead notes. Does not infer readiness from a successful child command.
A bead already closed by the completed sentinel is accepted after checking its
latest closure audit actor; landing appends its receipt to notes and preserves
the original close reason. Missing/unrelated closure evidence stops landing.
A bead already assigned to the UAC needs no reclaim; sentinel claims transfer
with conditional owner/status guards.

Phases: merge, limited CHANGELOG conflict resolution, project check, gate, verified
push (three attempts for GitHub SSH timeouts), install, bead close, pane/worktree
cleanup. Cleanup defaults on only for an identified completed child of this UAC;
branches and stashes are retained. Preview later merged cleanup with
`vega uac prune --dry-run`. Never force-removes or approves hooks. Landing
runs the project's optional install argv and adds no deploy step of its own.

When configured, the source gate must pass before a receipt is created. If coverage is missing,
run `vega gate start` in the source worktree, wait for its verdict, then repeat
`vega uac land BEAD --branch BRANCH` (with the original flags). After integration, a
pending gate starts review and returns exit 1; use --resume after its verdict.
Resume re-observes reality and never repeats a completed merge. Interrupted install
is uncertain: inspect effects, then --resume --retry-install explicitly retries
it, or --resume --install-done attests it completed. These flags apply only to an
interrupted installation on the same verified pushed head.

Runtime receipts: $XDG_STATE_HOME/vega/uac/land/<repo-hash>/<bead>.json
(default state root: ~/.local/state). repo-hash is SHA-256 of the canonical Git
common-directory path; each repository has its own subdirectory.
One unfinished receipt blocks new landings in that repository. Abandonment needs
UAC inspection and reversible receipt archival; see the design recovery contract.
--status shows receipt and residue without running phases. Exit 0: complete or
status; 1: failed/waiting/uncertain; 2: usage. Child exit is reported separately.

Examples:
  vega uac land v-123 --branch fix-login
  vega uac land v-124 --branch parser --target integration --keep
  vega uac land v-123 --resume

-h, --help: this reference. Design: docs/design/orchestration.md.
