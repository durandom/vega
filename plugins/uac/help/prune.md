Usage: vega uac prune [--dry-run]

Safely clean merged local branches and clean, paneless merged worktrees.
Run as the registered UAC from its configured target checkout. Reads committed
.vega/land.toml (same validation as land); absent policy defaults to origin/main.
Fetches that target and requires commit ancestry. Never deletes unmerged or
squash-merged branches; reports them as residue for explicit inspection.

--dry-run  Preview actions and residue; fetches remote metadata but never removes
           a worktree or branch. Does not execute Worktrunk or its hooks.
           Planned branch rows can still be refused by Git on apply (exit 1);
           fast-forward the local target/upstream or inspect residue, never force.
-h, --help Show this reference.

Worktrees must be clean (including untracked files), unlocked, with no active Git
operation and no pane/agent in their directory or descendants. Ignored files are
not part of the clean check and are removed with the worktree: copy gitignored
evidence and secrets out before any removal. Unknown inventory
retains them. Detached trees, ambiguous checkouts and the target stay intact.
Removal delegates to wt remove --foreground --no-delete-branch -- <absolute-path>, without force,
reap or hook approval. Branch deletion uses git branch -d, never -D; Git may
conservatively refuse deletion when the local target/upstream is behind.
Stashes stay intact. Shares land's repository lock; unfinished landings block it.

Output: deterministic actions and residue tables (kind, detail). Actions are
planned with --dry-run, completed otherwise. Exit 0: cleanup/preview complete,
including safety exclusions or nothing eligible; 1: operational failure; 2: usage.
Repeated runs preserve residue and succeed when nothing remains eligible.

Examples:
  vega uac prune --dry-run
  vega uac prune
