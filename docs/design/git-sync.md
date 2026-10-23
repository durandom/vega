# Git sync: vegad keeps checkouts fresh

Status: built, 2026-10-19. Bead: vega-lhc.120 (Spellkave UAC proposal, msg 3668; Slayer input
2026-10-19). Part of [vegad](vegad.md), next to [beads sync](beads-sync.md). Code:
`crates/vega-bd-sync/src/git.rs` (repos, state, step, hint), `plugins/post/src/gitsync.rs` (vegad's
thread), `plugins/bd` (`vega bd status`), `plugins/uac` (`vega uac status`), `shims/bd` (hints).

## Problem

Only beads synced across machines; git checkouts went stale. spellkave's main checkout stood 144
commits behind origin on proteus and 68 on ganymede (fast-forwarded by hand on 2026-10-19). The only
git fetch was vegad's daily one of the vega checkout ([upgrade.md](upgrade.md)). Sentinels started
from a stale main and nobody told them how far their base had fallen behind.

## Which repos

The bd registry (`$XDG_STATE_HOME/vega/bd/projects/`, [beads-sync.md](beads-sync.md)) plus every
UAC claim on this host (`vega uac claim`). UAC claim files are keyed by v2
project IDs, and retain the owning host's canonical `common_dir` metadata.
`project_uac_locations` and bd registrations supply IDs plus canonical local
common directories to `git::claimed`/`git::repos`. Workers verify the current
ID before filesystem access and deduplicate by ID/common directory. Independent
clones retain independent state. Remote IDs are never opened as paths; remote
locations are omitted. Not the janitor map: ~870
checkouts on proteus, most of them untouched for months, would be a network storm every 5 min.

## Rules

vegad's thread looks every 30 s; per repo:

1. **Fetch** every 5 min: `git --git-dir=<common> fetch --quiet --no-recurse-submodules origin`.
   Updates remote-tracking refs only, no working tree, no local branch. No prompt
   (`GIT_TERMINAL_PROMPT=0`), killed after 120 s. A failure is an `error`, retried on the next
   interval.
2. **Default branch**: `origin/HEAD`'s target, else `main` if `origin/main` exists.
3. **The main checkout** is the worktree that has the default branch checked out (`git worktree list
   --porcelain`). Git allows one, so no guessing in bare layouts. None: `skipped` (`no checkout has
   main checked out`): the permanent checkout sits on another branch or a rebase.
   In particular, `vega deploy apply --from-branch <feature>` leaves the host checkout on that feature
   branch, so gitsync skips main while no checkout has main checked out. Deploy again without
   `--from-branch` to return to main. `vega deploy` status still compares with pushed main.
4. **Fast-forward** at most every 15 min, only when the branch is behind its upstream:
   - local commits not on the upstream: `skipped` (`diverged`), never merged;
   - tracked files changed (`git status --porcelain --untracked-files=no`): `skipped`;
   - else `git merge --ff-only --quiet origin/<branch>` in that checkout. Untracked files do not
     block it: git itself refuses a merge that would overwrite one (then `skipped` with git's
     message). Git hooks run as for a manual `git pull`.

   A skip stays until the next attempt; the branch level with its upstream clears it.
5. **Worktrees**: for every other non-bare worktree, the upstream commits its HEAD lacks
   (`rev-list --count <head>..origin/<branch>`). vegad never rebases, merges or touches them.

`GIT_OPTIONAL_LOCKS=0` keeps vegad's `git status` from taking the index lock an agent may need.

## State and status

`$XDG_STATE_HOME/vega/bd/git/v2/<hex(ProjectId)>/<hex(common_dir)>.json`
next to the beads state, one writer (vegad). Each envelope carries schema 2,
project ID and canonical local common directory. Linked worktrees share state;
independent clones retain separate records. The status model is beads sync's:
one row per local clone, grouped by ID, a one-word state, and problems below the table.

`vega bd status` adds:

```
git[2]{project,location,branch,behind,ahead,fetched,state}:
  github.com/durandom/vega,/Users/u/src/vega/.git,main,0,0,2m ago,ok
  github.com/b4arena/spellkave,/Users/u/src/spellkave/.bare,main,12,0,2m ago,skipped
git_problems[1]{project,problem}:
  github.com/b4arena/spellkave,tracked files changed
worktrees_behind[1]{worktree,branch,behind}:
  ~/src/durandom/vega.fix,fix,7
```

States: `pending` (never fetched), `ok`, `behind` (a fast-forward is due), `skipped`, `error`, and
`local` (no `origin` remote: nothing to fetch, not an error).
`vega uac status` prints the same table as `git_sync`. vegad logs a fast-forward and every change
of state.

## Telling agents (bd shim)

Agents run bd all the time, so the bd shim is where they hear about sync state (Slayer, 2026-10-19).
Before it runs bd, the shim prints one stderr line each (P2: stdout and exit code stay bd's):

- the project's beads sync is `stopped` (the reason; the UAC resolves it);
- this is the main checkout, it is behind, and vegad's fast-forward was skipped (the reason);
- this is another worktree and it is behind the upstream: `vega: this worktree is N commit(s)
  behind origin/main ...; vegad never rebases worktrees, the UAC decides`.

Identity uses the shared bounded local resolver and reads per-location v2 state.
Each Git probe has a two-second deadline: failure or timeout suppresses hints, then the
user's bd command runs without a shim-imposed timeout (subject to the
[bd shim's guards](beads-sync.md#claim-identity-bd-shim)). Known gap, as for the actor:
`bd -C <dir>` hints for the current dir.

No herdr token or mail per sentinel: the behind count changes with every landing, and a sentinel
needs it only when it acts, which is when it runs bd.

## Not done here

- A per-project opt-out: every known repo is fetched and its clean main fast-forwarded. If a project
  needs otherwise, a flag in its registry record is the place.
- Remotes other than `origin`; repos known only to janitor.
- Windows (Io): vegad is Unix-only today.


Project records use normalized fetch-remote IDs; filesystem execution retains
owner-local locations. The startup journal applies plans before workers start,
preserving per-clone stopped state, timestamps and notification causes.
Interrupted startup replays the saved plan; rollback restores original bytes
while writers stay stopped. Historical bd backups remain untouched. The UAC
coordinates preview, audit, deploy, verify and rollback: see
[the operator runbook](project-registry-cutover.md#operator-runbook-146).
