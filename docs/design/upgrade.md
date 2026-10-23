# Upgrade: vegad notices a newer vega, `vega upgrade` installs it

Status: accepted
Date: 2026-10-15
Bead: vega-lhc.114 (Slayer decision 2026-10-15)
Parent: [onboarding.md](onboarding.md); builds on [vegad.md](vegad.md) (Upgrade in place)

## Context

An occasional user pulls an upgrade after weeks, if they remember. The README prompt (onboarding.md) only helps
when someone pastes it. The upgrade belongs in vega: vegad notices that the checkout this host was installed
from is behind its upstream and tells one vega session; `vega upgrade` does the pull, the install and the
changes view in one command.

## Install record

`scripts/install-cli.sh` writes `$XDG_STATE_HOME/vega/install.json` as its last step:

```json
{"checkout": "/Users/u/src/durandom/vega", "commit": "<full sha>", "by": "local", "at": 1760000000}
```

- `checkout`: the main checkout (the git common dir's parent), as for the skill link: a worktree's path dies
  with the worktree.
- `commit`: `HEAD` of the tree that was installed (a worktree's, if installed from one).
- `by`: `deploy` when `vega deploy` ran the script (it sets `VEGA_INSTALL_BY=deploy`), else `local`.

No record (installed before this change, or by hand): vegad checks nothing; `vega upgrade` says to run
`mise run install-cli` once.

## Behind detection in vegad

A thread in vegad (`plugins/post/src/upstream.rs`), like the CI and beads threads: git and herdr calls block,
so they never run on the relay loop. It wakes every 60 s and does almost nothing on most wakes:

1. Read `install.json`. Missing or `by: deploy`: nothing (one log line per change of that state).
2. Fetch at most once per 24 h: `git -C <checkout> fetch --quiet` with `GIT_TERMINAL_PROMPT=0`, killed after
   120 s. The time of the last attempt is kept in `$XDG_STATE_HOME/vega/upgrade.json`, so vegad's restarts
   and self-execs do not fetch again. A failed fetch counts as the attempt (logged): no retry storm against
   an unreachable remote; the next try is 24 h later.
3. After a fetch: `upstream` = `git rev-parse @{u}` of the checkout's branch (no upstream: logged, nothing),
   `behind` = `git rev-list --count <installed commit>..<upstream>`, and the changelog blocks of
   `git show <upstream>:CHANGELOG.md` newer than the newest block vegad itself was built with.
4. Propose once per upstream state: when `behind > 0` and `upstream` differs from `proposed` in
   `upgrade.json`. Between fetches the thread re-counts `behind` against the stored upstream with local git
   only, so an install by other means (`mise run install`) ends a pending proposal.

`vega deploy` targets never check: they have a checkout (deploy pulls into it), but their upgrade comes from
the deploying host's `vega deploy apply`. A later `mise run install-cli` by hand on such a host makes it
`local`, and from then on it checks for itself.

## Who hears about it

Only a dedicated vega session, never other agents:

- Candidates: agent panes on this host (`herdr pane list`, the `agent` field set) whose `cwd` is the vega
  checkout or below it (path components: `~/src/vega-skills` is not inside `~/src/vega`). Worktree
  sentinels are siblings of the checkout, so they never qualify.
- Several: the one registered as UAC for the checkout's project (`vega uac claim`), else the lowest pane id.
- Delivery: tool mail from `tool vega-upgrade` into that pane's inbox; vegad's delivery types the usual
  pointer when the agent is free. Then `proposed` is recorded.
- Without a broker (vegad in local mode, [vegad.md](vegad.md) Local mode; vega-lhc.118): the mail text itself
  is typed into the pane as one line (`[tool vega-upgrade] …`, lines joined by ` | `, cut at 1200 characters)
  when the agent is free, behind the draft guard. So the occasional user outside the estate gets the proposal
  too.
- No candidate: the token `$upgrade` (`vega ↓<n>`) on every herdr workspace whose cwd is in the checkout
  (`herdr workspace report-metadata --source vega:upgrade`, refreshed each wake, TTL 180 s). The thread keeps
  looking for a pane each wake; once one appears it gets the mail and the token goes.

The mail:

```
vega upgrade available: 12 commits upstream of the installed a1b2c3d (origin/main is 9f8e7d6).
Ask the user whether to upgrade now. On yes: run vega upgrade (refuses a dirty or diverged checkout),
summarise the changes for the user, act on migration entries, and run vega onboard apply on their OK.
Changelog:
  8 · 2026-10-20 feature: vega upgrade pulls, installs and shows the changes
```

The agent asks first: the Slayer decided (2026-10-16) that vega proposes the upgrade, the user says yes. The
instruction comes before the changelog, so a cut local-mode line keeps it.

## vega upgrade

`vega upgrade` (binary `vega-upgrade` in plugins/onboard, sharing its library):

1. Read `install.json`. `by: deploy`: refuse (`deploy_target`: upgrade the deploying host, then
   `vega deploy apply` there). No record: refuse with `mise run install-cli` as the next step.
2. Refuse a checkout with tracked changes (`dirty`), without an upstream (`no_upstream`), or, after
   `git fetch`, with commits on both sides (`diverged`).
3. `git pull --ff-only` when behind.
4. `mise run install-cli` in the checkout when the installed commit is not `HEAD`; its output goes to
   `$XDG_STATE_HOME/vega/upgrade.log`, the last lines show on failure.
5. moshi-hook, where installed (below).
6. The changes: the newly installed `vega-onboard` (the old process embeds the old changelog) prints its home
   view: `mode: upgrade`, the unseen changelog entries, infra and the config plan.
7. `help:` summarise the changes for the user, act on `migration` entries, `vega onboard apply` on their OK.
   `vega upgrade` never applies config itself.

A current vega still runs step 5: the command is also how moshi-hook gets upgraded.

## moshi-hook

moshi-hook (the Slayer's phone notification and approval bridge, Homebrew) is an optional infra component
(`vega_config::infra`): `moshi-hook` on PATH; the state says whether Homebrew installed it (the binary resolves
into a `Cellar/moshi-hook/` dir) and whether its service is registered (a `*moshi-hook.plist` in
`~/Library/LaunchAgents`, or a `*moshi-hook.service` in `~/.config/systemd/user`). It prepares vega-lhc.110
(decisions via moshi).

In `vega upgrade`, where installed:

- Homebrew install: `brew upgrade moshi-hook`; when its service is registered, `brew services restart
  moshi-hook`. Not from Homebrew: no upgrade, the next step names `moshi-hook update`.
- `moshi-hook doctor` with stdin from `/dev/null` (it asks before fixing anything; no answer means no fix),
  its exit code and output reported. `--yes` is never passed.

A failed moshi step is reported and makes the exit code 1, after the vega steps finished.

## Not done

- No automatic pull or install by vegad: the user's session decides; vegad only tells.
- No mail to every agent, no herdr toast: one session per host is enough, the token covers no session.
- No `ls-remote` instead of a fetch: it gives the upstream sha but not the commits or the changelog.
- No fetch on the relay loop: a slow remote would stall delivery (vegad.md, `slow` steps). The thread's first
  wake fetches when the last attempt is 24 h old, restarts included.
