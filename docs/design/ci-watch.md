# CI watch: one poller per host, results by mail

Status: built, 2026-10-10; run targets 2026-10-16; rebase and zombie signals 2026-10-20. Beads: vega-lhc.53,
vega-lhc.116, vega-lhc.151, vega-lhc.172. Part of [vegad](vegad.md). Code: `crates/vega-ci-watch` (targets,
buckets, `gh api` client, poll cycle), `plugins/ci` (`vega ci`), `plugins/post/src/ci.rs` (vegad's thread),
`shims/gh`, `crates/vega-shim`.

## Problem

spellkave, 2026-10-09: about eight sentinels on one GitHub user hit the 5000/h REST core limit twice in one
night. Each ran its own `gh run watch` or `gh pr checks --watch` loop plus bulk job-log pulls. CI jobs on that
user got 403, and every session was blind for up to an hour.

## Rate math

| Caller | Interval | Budget | Cost |
| --- | --- | --- | --- |
| `gh run watch` | 3 s (`--interval` default) | REST core | about 1200 requests/h per watcher, more with jobs |
| `gh pr checks --watch` | 10 s | GraphQL (`statusCheckRollup`) | 360 queries/h per watcher |
| `gh run view --log-failed` | once | REST core | one request per job plus the log download |
| vegad, per watched target | 30 s pending, 120 s settled | REST core | 0 when nothing changed; 1 per changed endpoint |

- REST core: 5000/h per user token, shared by every process and every CI job that uses that token.
- GraphQL: 5000 points/h, separate from core. `gh pr checks` costs points, not core requests.
- Conditional requests: a request with `If-None-Match: <etag>` that returns `304 Not Modified` does not count
  against the core limit. Verified 2026-10-10 with `gh api -i -H 'If-None-Match: …'`: two 304s left
  `X-RateLimit-Remaining` unchanged, the next 200 cost 1. `gh api` exits 1 on a 304 but prints the status line
  and headers with `-i`, so the poller reads the status line, not the exit code.
- `GET /rate_limit` is not reliable: on 2026-10-10 it reported `used: 0` while real requests on the same token
  reported `used: 155`. The limit comes from the `X-RateLimit-*` headers of real responses; a conditional
  `GET /user` (304, free) refreshes them.

Eight sentinels with `gh run watch` cost about 9600 requests/h: the limit is gone after half an hour. The same
eight PRs through vegad cost one 200 per change: a CI run of ten minutes changes the check runs a handful of
times as jobs start and finish, so a watched PR costs tens of requests per CI run, and nothing while idle.
Worst case, a PR whose endpoints change on every poll costs 5 × 120 = 600/h while its workflows are pending.

Per poll of a GitHub PR target, all conditional: 3 requests while job checks or statuses are pending,
5 otherwise (the head's workflow runs and the branch's run list for the
[zombie check](#signals-rebase-and-zombie-runs)); a branch target costs one fewer. There is at most one extra
workflow-list request per head per poll, shared by all watchers, without per-run fan-out or paging.
Zombie detection also asks at most one jobs request per open run of the branch every 15 min.
Settled, a PR polls every 120 s: 5 × 30 = 150/h
if every endpoint changed every time, 0 when nothing did. A run target costs 2 per poll, as before.

## Shape

```
sentinel ──vega ci watch 42──► ~/.local/state/vega/ci/watch/<target>__<pane>.json
                                         │ (read every 5 s)
vegad ── ci thread ──gh api -i (ETag)──► GitHub
   │          └── writes ci/state/<target>.json (cache) and ci/rate.json
   └── mail to the owner: "ci: PR #42 (o/r) head 1a2b3c4: red, failed: test, lint"
sentinel ──vega ci──► reads the cache, no API call
```

- The watch's owner is `--for <pane>` (any pane on this host), else the caller's pane: the herdr pane whose process tree
  holds the caller (`vega_herdr::me`), with `HERDR_PANE_ID` only a hint, as it is stale in long-lived processes
  and missing when a tool shell drops it (Codex `inherit = "core"`). Outside every pane it fails with `not_in_herdr` and registers nothing.
- `vega ci watch` registers and returns at once; it makes no API call. The event arrives as mail (`vega post`),
  with the usual pointer when the owner is free.
- vegad (`vega post relay`) runs the poller in its own thread, next to the herdr event stream, so a slow
  `gh` call never stalls mail delivery.
- One poller per host polls each target once per cycle, however many panes watch it.

## Targets and requests

| Target | Requests per cycle (all conditional) | Ends when |
| --- | --- | --- |
| PR `owner/repo#n` | `pulls/n` (head sha and branch, state, merge state), `commits/<sha>/check-runs`, `commits/<sha>/status`; job checks/statuses not pending: `actions/runs?head_sha=<sha>&per_page=100`, `actions/runs?branch=<b>` | merged or closed (mail says which) |
| branch `owner/repo@b` | `commits/<b>/check-runs`, `commits/<b>/status`; job checks/statuses not pending: `actions/runs?head_sha=<sha>&per_page=100`, `actions/runs?branch=<b>` | the branch is gone (404/422) after it was seen; before the push the watch waits |
| run `owner/repo/run/<id>` | `actions/runs/<id>`, `actions/runs/<id>/jobs?per_page=100` | the run completed (see [Dispatched runs](#dispatched-runs)) |
| workflow `owner/repo/workflow/<file>[@<ref>]` | until resolved: `actions/workflows/<file>/runs?event=workflow_dispatch&branch=<ref>` (`head_sha=` for a 40-hex ref); then as a run | as a run; or no run found 15 min after the watch |

Every target also ends when no watcher is left (pane gone, `vega ci unwatch`) or after three days without a
change. ETags are kept per URL; a new head sha starts new URLs.

Buckets as in gh-axi (`repos/gh-axi/src/commands/pr.ts`): `pass` (success, neutral), `fail` (failure,
timed_out, action_required, startup_failure, error), `skip` (skipped), `cancel` (cancelled), `pending`
(anything else, or not completed). Commit statuses map `success`→pass, `failure`/`error`→fail,
`pending`→pending.

GitHub PR and branch watches include the head's workflow runs alongside check runs and commit statuses.
A queued workflow can have no job checks yet: external passes alone do not prove green. When job checks
and statuses are no longer pending, the poller lists `actions/runs?head_sha=<sha>&per_page=100` conditionally.
`queued`, `in_progress`, `waiting`, `requested`, `pending`, and unknown unfinished states keep the head
pending; completed runs use the same conclusion buckets as jobs. A failed workflow is red even without a
failed job check. A truncated list stays pending rather than spending more calls on pagination.
Missing or malformed responses cannot produce a green mail; an old cache without workflow data also waits
for this probe. Runs of other heads do not affect this head's verdict.
Forgejo remains based on commit statuses: its Actions API has not been measured (see Open questions).

Verdict per head: `red` as soon as one check or workflow fails (failed names listed, pending count added), else `pending`
while any check or workflow is pending, else `green` when at least one passed, else `none` (no checks, or only skipped and
cancelled). Cancelled runs are not failures (gh-axi): usually a newer push cancelled them.

## Events

Each watcher gets one mail per new (head, verdict) with verdict `green` or `red`, plus one when the target
ends. A watcher that registers after CI finished gets the current verdict on the next cycle. `pending` and
`none` send nothing; `vega ci` shows them.

```
ci: PR #42 (o/r) head 1a2b3c4: green (12 pass, 2 skip). vega ci show o/r#42
ci: PR #42 (o/r) head 1a2b3c4: red, failed: test (ubuntu), lint; 3 pending. vega ci show o/r#42
ci: PR #42 (o/r) merged; watch ended
```

The mail has no sender pane (like mail from the vega board): there is nobody to reply to.

## Signals: rebase and zombie runs

spellkave, 2026-10-19: rebase churn burned runner time, and zombie runs blocked PRs. Two GitHub-only mails,
each once per pane and signal, next to the verdict mails (vega-lhc.151):

```
ci: PR #42 (o/r) head 1a2b3c4: conflicts with its base (CONFLICTING); rebase onto the base and resolve them before you push again, CI on this head cannot merge. vega ci show o/r#42
ci: PR #42 (o/r) head 1a2b3c4: behind its base, which requires up-to-date branches; rebase onto the base before the merge. vega ci show o/r#42
ci: PR #42 (o/r): run 39310882417 is still queued or in progress though all its jobs completed (15 min and more); it holds its concurrency group. vega does not cancel it; to force-cancel: gh api -X POST repos/o/r/actions/runs/39310882417/force-cancel
```

**Rebase.** From `pulls/n`, which the poll asks anyway: no extra request. Recorded on b4arena/spellkave
(2026-10-19, fixtures `pull-dirty.json`, `pull-unstable.json`): a conflicting PR says `mergeable: false`,
`mergeable_state: dirty`, `rebaseable: false`. Signalled for `dirty` (also when `mergeable` is false, since a
draft says `draft`) and `behind`; once per (head, state), so the `unknown` GitHub reports while it recomputes
after a push to the base does not re-arm it. `behind` appears only when branch protection requires
up-to-date branches: spellkave does not, and its #4353, 26 commits behind main, said `unstable`. Being
behind is not otherwise signalled: it would need a `compare` request per poll, and mailing every behind PR
would cause the rebase churn this is meant to stop. Not verified: whether a conditional request (304) starts
GitHub's lazy mergeability computation; a push to the head or base starts it anyway. A pane mailed the
rebase of the current head is not `waiting` (GitHub runs no `pull_request` CI on a conflicting PR).

**Zombie run.** A run whose `status` stays `queued` or `in_progress` while all its jobs are `completed`
(none at all counts: spellkave has had `queued` runs without jobs since 2026-08-06, fixture
`run-zombie.json`). Recorded: a live spellkave run says `queued` while its jobs run (fixtures `runs-branch.json`,
`check-runs-actions.json`), so the run status
alone proves nothing; only the jobs do. `waiting`, `pending` and `requested` (approval, concurrency wait)
are not zombies.

- PR and branch: once the checks are not pending (a zombie's jobs are done), the poll lists the branch's
  newest 20 runs (`actions/runs?branch=<b>&per_page=20`, conditional; one request for every head of the
  branch, since a zombie of an older head holds the new head's concurrency group). An open run that stays
  open 15 min (`ZOMBIE_AFTER`) gets one `actions/runs/<id>/jobs?per_page=100` request; all done (and
  `total_count` equal to the jobs listed) makes it a zombie, else it is asked again 15 min later.
- Run target: from the run and jobs it polls anyway; no extra request. Done for 15 min by vegad's polls is a
  zombie, which ends the pane's `waiting` once mailed (the run will never complete).

vega only detects and mails the command; it never cancels. The zombie is dropped once the run leaves the
open list (completed, for example force-cancelled), so a later zombie mails again.

## Dispatched runs

spellkave, 2026-10-11 and 2026-10-15: sentinels dispatched a workflow (`gh workflow run`), registered `vega
ci watch` on the branch and waited for a mail that never came (once ~9 h). A dispatched run has no PR, and
its check runs land on a head sha the branch watch had already mailed green for: the new pending-then-green
is the same (head, verdict), so nothing is mailed. Only a red would have arrived.

A run target watches the run itself, GitHub Actions only (a Forgejo host is refused at `vega ci watch`;
Forgejo Actions runs are not built, see Open questions):

- `--run <id>`: the run id. `gh workflow run` prints the run URL (`…/actions/runs/<id>`) when GitHub returns
  it; `vega ci watch <run URL>` works too.
- `--workflow <file|id> [--ref <branch|tag|sha>]`: vegad resolves it, so `vega ci watch` still makes no API
  call. Each cycle until resolved lists that workflow's `workflow_dispatch` runs (conditional, a 304 is free)
  and takes the newest created at most 10 min (`DISPATCH_SLACK`) before the earliest watch of the target:
  the watch usually follows the dispatch by seconds, sometimes minutes. The resolved id is cached in the
  state (`run_id`). No run within 15 min (`RESOLVE_WITHIN`) ends the watch with a mail saying so. Two
  dispatches of the same workflow and ref within the slack are ambiguous: the newer wins; use `--run`.

Checks are the run's jobs (check-run buckets) plus the run itself, so a run that fails without a failed job
is red too. Mails follow the usual rule (one per new (head, verdict)): a failed job mails red at once while
the run goes on. A completed run ends the target once its verdict is mailed, without an extra "ended" mail;
a completed run that neither passed nor failed (`cancelled`, `skipped`) mails its conclusion and ends:

```
ci: deploy.yml run 38721967731 (o/r@main) head 9f2c4e1: green (3 pass). vega ci show o/r/workflow/deploy.yml@main
ci: run 38721967731 (o/r) head 9f2c4e1: red, failed: build; 2 pending. vega ci show o/r/run/38721967731
ci: run 38721967731 (o/r) completed: cancelled; watch ended
```

A run target is owed (`waiting`) until the run completes, however long it queues: the 15-minute
no-checks grace of PR and branch watches does not apply.

## Ownership

The pane that runs `vega ci watch` owns the watch; `--for <pane>` registers another pane on this host (a UAC
for its sentinel). Without a target, the watch is the current branch (its upstream name) of the cwd's repo:
run in a sentinel's worktree, this is the "PR head branch → worktree → pane" mapping without a lookup. A
branch watch ends when the branch is deleted after the merge; pass the PR number to end it at the merge.

## Supervision

A sentinel that ends its turn to wait for a verdict is idle without `vega post done`: vegad's supervision
([vegad.md](vegad.md)) would call it `silent` and wake its parent for nothing (vega-lhc.64). While a watch of
its pane still owes it a mail it is `waiting` instead: out of the wake set, shown in `vega uac` with the
target. Owed (`store::owed`): the target is not polled yet, its checks are pending, it has no checks yet (for
15 min after the last change), or its verdict for the current head was not mailed to that pane. A mailed
verdict owes nothing, though the watch stays until the merge: the mail wakes the sentinel, and if it then idles
without a report, it is `silent` again and its parent is woken. Watches are host-local, so only local
children can be `waiting`.

Not built: watching every open PR of a repo automatically and finding the owner through the lanes records.
It costs requests for PRs nobody waits on, and a branch does not name its pane reliably.

## Backoff

From the last `X-RateLimit-Remaining` and `X-RateLimit-Reset` of any response:

- remaining < 1000: intervals × 4.
- remaining < 200: no polls until the reset; `vega ci` says so.
- 403 or 429 with `Retry-After`: no polls for that long (at least 60 s).

## Cross-host

One poller per host first. Two hosts that watch the same PR poll it twice, but both pay only for changes, so
the cost is small. Sharing one poller across hosts through the registry (a `<host>.ci.<target>` key, the
other hosts subscribe) is a follow-up, not built.

## CLI

```
vega ci                         # watches and their state from vegad's cache, gh limit line (TOON)
vega ci watch [<pr|url|branch>] [-R owner/repo] [--for <pane>]
vega ci watch --run <id> | --workflow <file> [--ref <ref>]   # a dispatched GitHub Actions run
vega ci unwatch [<target>]
vega ci show <target> [--all]   # failed, pending, cancelled checks of the head (--all: also passed, skipped);
                                # also the merge state and zombie runs
```

`vega ci` reads only local files: it makes no API call. Without vegad running it says so: the watch is
registered but nothing polls it.

## gh shim

`shims/gh` (Rust, like `shims/herdr`; shared mechanics in `crates/vega-shim`):

- Passthrough by `exec`: stdout, exit codes and TTY are the original's.
- Deny `gh run watch` and `gh pr checks --watch` with exit 1 and a stderr pointer to `vega ci watch`. This is
  a documented deviation from P2 in [agent-tooling-principles.md](agent-tooling-principles.md): a loop that
  drains the shared limit hurts every agent and CI job of the user.
- Hint once per session (stderr) when a session calls `gh pr checks`, `gh run view` or `gh run list` for the
  third time within ten minutes.
- Hint once per session (stderr, before the call) on `gh workflow run`: watch the run with `vega ci watch
  --run <id>` or `--workflow`. The shim `exec`s gh, so it cannot read the printed run URL and name the id.
- Count calls per pane (`$XDG_STATE_HOME/vega/calls/gh/<pane>`, one line per call); `vega uac status` shows
  the last hour per pane and the gh core limit. A caller without `HERDR_PANE_ID` (e.g. a Codex session
  started outside herdr) counts under its session, `codex:<thread id>` or `claude:<session id>`; `vega uac
  status` lists those apart (`gh_calls_without_pane`, harness and id prefix), never as a pane.
- The spawn footer says: never poll gh in a loop; use `vega ci watch`.

## tea shim

`shims/tea`, the same shape as the gh shim, for agents that poll Forgejo with `tea`:

- Passthrough by `exec`; denies nothing (no polling loop is known for tea yet).
- Hint once per session (stderr) when a session calls `tea pulls` (aliases `pull`, `pr`) or `tea api` for the
  third time within ten minutes.
- Count calls per pane (`$XDG_STATE_HOME/vega/calls/tea/<pane>`, same fallback without `HERDR_PANE_ID`);
  `vega uac status` shows the last hour per pane (`tea_calls`, omitted when zero), with no limit line: Forgejo
  sends no rate headers.

## Forgejo and Gitea

First target: `git.b4mad.industries` (Forgejo 11, Gitea API 1.22), CI by Tekton Pipelines-as-Code (PaC).
Measured 2026-10-12. The rest of this doc describes GitHub; this section lists what differs.

| | GitHub | Forgejo/Gitea |
|---|---|---|
| Transport | `gh api -i` | `tea api -i https://<host>/api/v1/<path>` |
| Token | `gh auth` | `tea login add` (per host; tea picks the login by URL host) |
| Conditional requests | ETag, 304 is free | none: no ETag, no Last-Modified |
| Rate headers | `X-RateLimit-*` | none, authenticated or not |
| Checks | check-runs plus statuses | commit statuses only |
| Status key | `state` | `status` (plus `context`, `description`) |

**Forge detection.** `github.com` is GitHub, never probed. Any other host: `GET /api/v1/version`; a 2xx with a
string `version` means Forgejo, 404 means GitHub (Enterprise has no such route); both are cached per host in
`forges.json` in the ci state dir. Any other 2xx, 3xx or 4xx is taken as GitHub but not cached: a captive portal
(302) or a Forgejo behind a login (401, 403) answers the same, and the next probe may tell. 5xx or a failed probe
is an error. `Target.forge`
is stored in the watch file only when not GitHub, so old watch files read as GitHub.

**Requests per cycle and target** (`poll_forgejo`): PR watch `pulls/n` (state, merged, head sha), branch watch
`branches/<b>` (404 means gone, or not pushed yet if never seen), then `commits/<head>/status?limit=50`.
Two full requests, no 304. The saving is only that one vegad poller per host serves all watchers. A merged PR
reports `state: closed` plus `merged: true`. Intervals are unchanged (30 s pending, 120 s settled).

**tea specifics** (`crates/vega-ci-watch/src/tea.rs`). `tea api -i` prints status line and headers on
**stderr**, the body on stdout, and exits 0 for any HTTP status; exit 1 is a tea-level error (for example
"no available login"). The status line decides, never the exit code. tea is optional, not a mise tool of this
repo (mise builds it from source with Go): `brew install tea`, or `mise use -g go:gitea.dev/tea@0.16.0` with Go
installed. Without it the tea shim prints that hint and exits 127, and a Forgejo watch reports the same as its
error. vegad finds `tea` on its PATH, else in
`~/.local/share/mise/shims`; `VEGA_CI_TEA` overrides it for tests.

**Backoff.** No rate headers, so only an HTTP 429 pauses the poller. A 403 on Forgejo is a token without
access, reported as an error, not a pause (on GitHub 403 can be a spent limit). 429 is not verified live.

**Mapping.** `success` and Gitea's `warning` are pass; `failure` and `error` fail; anything else (including
`pending`) is pending. A commit without statuses answers `statuses: null`, `sha: ""`, verdict `none`.
Limit: 50 statuses per request, no paging; PaC reports a handful per PR.

**PaC caveat.** A run that waits for approval shows as pending with the description "Pending approval, waiting
for an /ok-to-test". Nothing starts until a maintainer comments `/ok-to-test`, so the verdict stays pending,
possibly for a long time. `vega ci show` prints the description as a `note:` (`Check.note`).

**Not parity.** No `gh run view` equivalent for failed logs: the hint points at the check link in the PR.
`vega ci` hides the `gh` rate-limit line when only Forgejo watches exist.

## Open questions

- Forgejo Actions runs (a `workflow_dispatch` on Forgejo): not built. Its run API differs from GitHub's and was
  not measured; the PaC instance reports through commit statuses, which a branch watch already sees.

- GitHub Enterprise hosts: the poller passes `--hostname` from the remote URL; not tested.
- Should a `none` verdict mail after a while (a PR without CI never sends anything)?
- Forgejo 429 behavior and statuses paging (>50) are untested.
- Forgejo hosts behind a `tea` login with a different name than the host: tea picks by URL, untested.
