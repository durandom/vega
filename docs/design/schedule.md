# Timed tasks in vegad: reminders and repo-bound jobs

Status: design, revised 2026-10-24 (vega-lhc.204.8, Slayer direction of 2026-10-24; first version vega-lhc.197).
Part of [vegad](vegad.md). Builds on [pagerduty.md](pagerduty.md) (committed config read from HEAD) and
[git-sync.md](git-sync.md) (the main checkout).

## Problem

Agents and projects need things to happen at a time: "remind me at 07:00 to check the deploy", "tell the brainz
UAC every Monday to triage", "run brainz's deadline script daily on proteus". Today that means a hand-written
crontab or launchd plist: invisible to the UAC, no failure mail, no catch-up after sleep, no trust check.

## Decision (summary)

1. One primitive in vegad, no external dependencies: a timer thread with two kinds of timed task.
2. **Reminders** (the main path): a `vega post` message delivered at a time, once or daily/weekly, through the
   normal mail and pointer path. No code runs, so agents create, list and cancel them without approval.
3. **Jobs**: a script inside the repo, declared in a checked-in `.vega/schedule.toml`, run with the repo root as
   cwd, only after the host's user approved its hash. Results go to the project's UAC as mail.
4. Shared rules: `daily = "HH:MM"`, `weekly = "<day> HH:MM"` (jobs and reminders), `at`/`in` (reminders), host
   local time, a missed slot fires once at the next chance, no overlap, durable state across restarts.
5. Out of scope: what a job does (GUI session, EventKit, `just`, secrets) is the project's business; vega
   gives no `session = "gui"`, no `launchctl asuser`, no app-specific hooks.

## When (both kinds)

- `daily = "HH:MM"`, `weekly = "mon|…|sun HH:MM"`, 24 h clock, the owning host's local time zone. Reminders
  also take `--at HH:MM` (next occurrence), `--at YYYY-MM-DDTHH:MM` and `--in 90m|2h|3d` (once).
- DST: a local time that does not exist (spring forward) fires at the first valid minute after it; one that
  occurs twice (fall back) fires at the first occurrence.
- Catch-up: a tick every 30 s computes the latest slot ≤ now. If it is newer than the task's `last_slot`, it
  fires once; a week asleep gives one firing, the most recent slot. Nothing fires for slots before the task
  existed (or before its first approval). After a reboot nothing fires until herdr starts vegad.
- No cron expressions or `every = "6h"` in v1: no consumer needs them; both would be additive keys.

## Reminders

```
vega post send --at 07:00 <target> "check the deploy"          # once, next 07:00
vega post send --in 2h --uac brainz "re-run the import"         # once
vega post send --daily 08:30 --uac . "standup: vega uac"        # recurring
vega post send --weekly "mon 09:00" review-bot "triage backlog"
vega post reminders [--all]                                      # list
vega post reminders cancel <id>
```

- A time flag (`--at`, `--in`, `--daily`, `--weekly`; exactly one) turns `send` into a reminder; every other
  `send` flag keeps its meaning (`--uac`, `--host`, `--now`). Without a time flag `send` is unchanged.
- Target: a pane id, a unique herdr agent name on this host (no `--host` with a name), or `--uac <project>`. Your own pane is
  allowed (the common "remind me"), unlike a plain send. A pane id is resolved once at creation (`pane_gone`
  as today); a name or `--uac` is resolved at each firing, so a recurring reminder follows a UAC that moved
  host or pane. Recurring reminders take a name or `--uac`, not a pane id (panes do not outlive their agent).
- Owner: the vegad of the host where the reminder is created. It stores one definition file per reminder in
  `$XDG_STATE_HOME/vega/reminders/<id>.json` (written by the CLI, deleted by `cancel`) and its firing state in
  `reminders/state/<id>.json` (written by vegad only), so the CLI and vegad never write the same file. vegad
  re-reads the definition right before it publishes: a cancelled reminder never fires.
- Firing publishes a normal mail to the target's inbox, sender `tool vega-remind`, body
  `reminder <id> (<when>, set by <pane> on <host>): <text>`, plus `late: due <slot>` after a catch-up. From
  there the usual delivery applies: pointer when idle or done, draft guard, `--now` urgency, bounces. In
  local mode (no broker) a target on this host gets the local tool-mail line; another host waits for the broker.
- At least once: vegad writes the slot as `pending`, publishes with `Nats-Msg-Id = <id>/<slot>` (JetStream
  drops the duplicate of a retried publish), then moves it to `last_slot`. A publish that fails stays pending
  and retries each tick. A one-shot reminder is deleted once its slot is `last_slot`.
- Undeliverable at firing (the name or UAC does not resolve, the pane closed): one board note naming the
  reminder; a one-shot ends, a recurring one stays for its next slot.
- Who may: any agent on the host, no approval (no code runs, P7: the box is the boundary). Inbox-only hosts
  (deimos) cannot create reminders. Limits: text ≤ 2 KiB, at most 100 active reminders per host
  (`reminder_limit` error naming `vega post reminders`).
- AXI output. Create: `scheduled: <id>`, `to: <target>`, `next: <local time> (<in>)`, `help: vega post
  reminders cancel <id>`. List (TOON): `id, when, next, to, by, text` (text truncated to 60 chars), by
  default the reminders this pane created or that target it; `--all` the host's. Empty state:
  `0 reminders on <host> (all: <n>)`.

## Jobs

```toml
# .vega/schedule.toml (committed; read with `git show HEAD:.vega/schedule.toml`)
[[job]]
name = "fristen-execute"            # [a-z0-9-]+, unique in the file
run = "scripts/fristen-execute"     # a file in the repo, relative to the repo root
args = ["--batch"]                  # optional, plain strings, no shell
daily = "07:30"                     # or weekly = "mon 03:00"; exactly one
hosts = ["proteus"]                 # required: no implicit "every host"
timeout = "15m"                     # optional, default "1h", max "24h"
notify = "failure"                  # optional: failure (default) | always | never
pin = ["justfile"]                  # optional: further repo files whose blobs enter the hash
catchup = true                      # optional, default true; false skips a slot missed by > 15 min
```

**Repo-bound.** cwd is always the root of the project's main checkout on this host (git-sync.md); there is no
`cwd` key. `run` is a relative path: absolute paths and any `..` component are an error at parse time. Before
each run the runner resolves it in the working tree: after following symlinks it must still lie inside the repo
root, be a regular executable file, and be tracked at `HEAD` with the same blob id. vega never resolves `run` on
PATH, so `just x` becomes a two-line script in the repo; what that script calls is the project's business.
No main checkout on the host: listed as `no checkout`, never run from a sentinel worktree. The file is
re-read every minute; one that does not parse (unknown keys included: `daly` must not mean "never") lists as
`invalid` and runs nothing of that project.

**Trust.** vega is public and collaborators can push, so a checked-in job is remote code execution on every
host with the checkout unless the host says yes. Rule: a job runs only if this host approved its hash.

- Hash = SHA-256 over the canonical `project id, name, run, args, hosts` plus a sorted `(path, blob id at
  HEAD)` pair for `run` and each `pin` entry. A changed script or pinned file needs a new approval; `daily`,
  `weekly`, `timeout`, `notify` and `catchup` do not (they cannot change what runs or where).
- Approvals live in `$XDG_CONFIG_HOME/vega/schedule-approved.toml`, `"<project id>/<name>" = "<hash>"`,
  per host, never in a repo: an approval on proteus never lets a job run on ganymede.
- `vega schedule approve <project>/<job>` prints the definition and the diff against the approved one, then
  stores the hash. Same model as `mise trust` and direnv `allow`. A new hash lists as `unapproved` and vegad
  mails the UAC once per new hash.
- The user approves, typed in a shell or with `!`. The guide tells agents never to approve; the UAC records the
  request as a `slayer-decision` bead when the user is away. No technical block (P7: it defends against
  remote pushes, not local agents; env markers would also block the user's `!`).
- Run-time checks, each a mailed skip: `run` or a `pin` file differs from its blob at HEAD (`skipped (modified:
  <path>)`), the main checkout is not on the default branch (`skipped (checkout on <branch>)`).
- Residual risk, stated in the guide: the hash covers `run` and the pins, not every file they reach; unpinned
  committed code is as trusted as the project's default branch.

**Runner.** vegad spawns `vega-schedule exec <project>/<job> --slot <ts>` in its own session (setsid) and does
not run the script itself. The runner takes the job's flock, creates the slot's run file `<slot>.json`
(`outcome = "started"`; it exits at once if the file exists, so a respawn is idempotent), re-checks hash and
files, runs the script with stdout and stderr appended to `<slot>.log`, enforces the timeout (SIGTERM to the
process group, SIGKILL after 10 s) and fills the run file (end, exit code or signal, outcome). Env: vegad's
(no agent markers, no shims) plus `VEGA_SCHEDULE_JOB`, `VEGA_SCHEDULE_SLOT` (RFC 3339) and
`VEGA_SCHEDULE_NOTIFY` (a file path); stdin `/dev/null`.

**Crash safety.** Per job `state.json` (atomic replace): (1) write the slot as `pending`; (2) spawn the runner
or record the skip; (3) once the run file exists, move it to `last_slot`. After a restart a `pending` slot
without a run file is spawned again; a run file still `started` whose lock is free becomes `lost` and mails like
a failure. A slot runs at most once and never ends silently; a half-run job is reported, never retried.

**Overlap.** A slot due while the job runs is recorded `skipped (running)` and not queued. `vega schedule run`
uses the same runner and lock and never touches `last_slot`. Jobs of different projects run in parallel.

**Logs.** `$XDG_STATE_HOME/vega/schedule/<project>/<job>/<UTC slot>.{log,json}`; keep the last 20 runs and
nothing older than 30 days, pruned on each run. vegad's own log gets one line per start and end.

**Results by mail.** Through `vega post` as `tool vega-schedule` to the project's live UAC, resolved through the
registry on any host (as PagerDuty does); in local mode only a UAC pane on this host; no UAC: a board note.
`notify = failure` (default) mails `failed` (exit ≠ 0), `timeout`, `lost`, `spawn error` and the skips above
(never `skipped (running)`), and one `recovered` line on the next success after a failure. `always` also mails
each success, `never` nothing. Body: job, host, slot, outcome, exit code, duration, the last 20 lines of output
(≤ 2 KiB), the log path, `vega schedule show <job>`. A non-empty `$VEGA_SCHEDULE_NOTIFY` file is mailed even on
success ("3 deadlines today"). Job output is untrusted data in the mail.

**CLI (AXI).**

```
vega schedule                        # home: jobs for this host, plus the reminder count
vega schedule show <project>/<job>   # definition, hash, approval, last 5 runs, tail of the last log
vega schedule approve|revoke <project>/<job>
vega schedule run <project>/<job>    # now, through the runner; respects lock and approval
vega schedule log <project>/<job> [--run <slot>] [--full]
```

Home (TOON): `job, when, next, last (outcome, age), state`, with `state` one of `ok`, `unapproved`, `changed`,
`invalid`, `no checkout`, `running`, `vegad stopped`; each non-`ok` row's next step once below the table, then
`reminders: <n> on this host (vega post reminders --all)`. Empty: `0 jobs: no .vega/schedule.toml declares
this host (<n> projects checked)`. Host-local in v1.

## Layout

`crates/vega-schedule`: slots (once/daily/weekly, DST), catch-up state, job parse and path rules, hash,
approvals, run files, reminder store. `plugins/schedule`: `vega schedule`, `vega-schedule exec`, `--skill`.
`plugins/post`: the reminder flags and `reminders` command, and `src/schedule.rs`, the vegad timer thread for
both kinds.

## Why not launchd or cron

Generated plists or crontabs are per-OS, drift from the repo, and still need trust, mail, listing, logs and
overlap built around them; vega left launchd on purpose (vegad.md, Lifecycle). Host-local job config loses the
jobs on a fresh clone and keeps the UAC from proposing one by commit. A checked-in file without approval lets
any collaborator run code on the host. Cost of the choice: nothing fires while herdr (and so vegad) is not
running.

## Implementation plan (beads, epic vega-lhc.204)

1. ~~Spike: EventKit from vegad~~ (vega-lhc.204.1, dropped: the project's business).
2. **Model** (vega-lhc.204.2): `crates/vega-schedule` slots, DST, catch-up, reminder store, job parse with the
   repo-bound path rules, hash, approvals, state.
3. **Reminder CLI** (new): `vega post send --at|--in|--daily|--weekly`, `vega post reminders [cancel]`, the
   guide lines in `plugins/post/SKILL.md`. Needs 2.
4. **vegad timer thread with reminder delivery** (new): the 30 s tick, pending/last_slot, publish with
   dedup id, local mode, undeliverable notes. Needs 2, 3. Reminders ship here, before any job code.
5. **Runner** (vega-lhc.204.3): `vega-schedule exec`, repo-root cwd, file checks, lock, timeout, run files.
6. **Jobs in the thread** (vega-lhc.204.4): main checkout discovery, minute re-read, spawn, restart recovery.
   Needs 4, 5.
7. **Job mail** (vega-lhc.204.5): failure, recovered, always, notify file, unapproved note. Needs 6.
8. **`vega schedule` CLI** (vega-lhc.204.6): home, show, approve, revoke, run, log, `--skill`. Needs 2.
9. **brainz rollout** (vega-lhc.204.7): brainz commits its scripts and jobs, the Slayer approves on proteus,
   one real run each; CHANGELOG and vegad.md. Needs 7, 8.
10. Later, on a consumer's request: KV mirror for a cross-host list, `cron`/`every`, a board column.

## Open questions

- Should recurring reminders expire (`--until`), or live until cancelled as in v1?
- Is one mail per failed run right for a daily job broken for a week, or one per day at most?
