Usage: vega uac prune [--dry-run]

Safely clean merged local branches and clean, paneless merged worktrees.
Run as the registered UAC from its configured target checkout. Reads committed
.vega/land.toml (same validation as land); absent policy defaults to origin/main.
Fetches that target and requires the local target to match it (fast-forward first
if behind). Worktrunk detects integrated content, including squash merges;
unmerged branches remain residue. Worktrunk's default branch is pinned to the
policy target for each subprocess without changing saved configuration. A target
upstream with commits outside the fetched target must be reconciled first.

--dry-run  Preview actions and residue; fetches remote metadata but never removes
           a worktree or branch. Reads Worktrunk integration data without hooks.
           Apply rechecks identity and integration; changed or refused branches
           remain residue. Inspect residue before retrying, never force.
-h, --help Show this reference.

Worktrees must be clean (including untracked files), unlocked, with no active Git
operation and no pane/agent in their directory or descendants. Ignored files are
not part of the clean check and are removed with the worktree: copy gitignored
evidence and secrets out before any removal. Unknown inventory
retains them. Detached trees, ambiguous checkouts and the target stay intact.
Removal delegates to wt remove --foreground --format=json -- <absolute-path-or-branch>,
without force, reap or hook approval. Worktrunk owns safe branch deletion and
retains anything it cannot prove integrated. Unavailable integration data retains
branches; retained or incomplete removals are reported for inspection.
Stashes stay intact. Shares land's repository lock; unfinished landings block it.

Output: deterministic actions and residue tables (kind, detail). Actions are
planned with --dry-run, completed otherwise. Exit 0: cleanup/preview complete,
including safety exclusions or nothing eligible; 1: operational failure; 2: usage.
Repeated runs preserve residue and succeed when nothing remains eligible.

Examples:
  vega uac prune --dry-run
  vega uac prune
