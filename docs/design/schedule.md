# vega schedule: per-project jobs run by vegad

Status: design, 2026-10-24. Bead: vega-lhc.197 (request from the brainz UAC). Part of [vegad](vegad.md).
Builds on [pagerduty.md](pagerduty.md) (committed config read from HEAD), [git-sync.md](git-sync.md) (the main
checkout), [gate.md](gate.md) (config from the reviewed target).

## Problem

Projects need recurring commands on a given host. First consumers (brainz):

| Job | When | Host | Note |
| --- | --- | --- | --- |
| `just fristen-execute` | daily | proteus | EventKit (Reminders/Calendar): needs the user's GUI login session |
| `just vault-reindex` | weekly | proteus only | |

Today each project would hand-write a launchd plist or crontab: invisible to the UAC, no failure mail, no catch-up
after sleep on cron, nothing in `vega uac` or the board.

## Decision (summary)

1. Jobs are declared in a checked-in `.vega/schedule.toml`, read from the main checkout's committed `HEAD`.
2. vegad runs a job only after a human on that host approved the job's exact definition hash
   (`vega schedule approve`). A changed `cmd`, `cwd`, `env` or `pin` needs a new approval; a changed time does not.
3. `daily = "HH:MM"` and `weekly = "<day> HH:MM"` in host local time. No cron in v1.
4. A missed slot (sleep, vegad down) runs once at the next chance. One run per job at a time; a slot that comes due
   while it runs is skipped. Default timeout 1 h.
5. Mail to the project UAC on failure only, or when the job writes to `$VEGA_SCHEDULE_NOTIFY`.
6. vegad, not generated launchd plists: vega already left launchd (vegad.md, Lifecycle), and everything except the
   timer (trust, mail, list, logs, locking) must be built either way.

## Declaration

```toml
# .vega/schedule.toml (committed; read with `git show HEAD:.vega/schedule.toml`)
[[job]]
name = "fristen-execute"          # [a-z0-9-]+, unique in the file
cmd = ["just", "fristen-execute"] # argv, no shell; resolved on vegad's PATH
daily = "07:30"                   # or weekly = "mon 03:00"; exactly one of them
hosts = ["proteus"]               # required: no implicit "every host"
cwd = "."                         # optional, relative to the repo root, must stay inside it
timeout = "15m"                   # optional, default "1h", max "24h"
notify = "failure"                # optional: failure (default) | always | never
env = { BRAINZ_MODE = "batch" }   # optional, plain values only (secrets: `vega secrets run` inside cmd)
pin = ["justfile"]                # optional: repo files whose blob ids enter the approval hash
catchup = true                    # optional, default true: run a missed slot once (see Running)
```

- Which checkout: the project's main checkout on this host (git-sync.md: the worktree with the default branch),
  found through the same project discovery as the PagerDuty poller. No main checkout on the host: the job is
  listed as `no checkout`, never run from a sentinel worktree.
- Re-read every minute, like `.vega/pagerduty.toml`. A file that does not parse lists as `invalid` with the error
  and runs nothing of that project; vegad never guesses a partial file.
- Unknown keys are an error (a typo like `daly` must not silently mean "never").

## Trust

vega is public and collaborators can push. A checked-in file that makes vegad execute code is remote code
execution on every host that has the checkout, unless something on the host says yes.

**Rule: a job runs only if this host approved its hash.** Hash = SHA-256 over the canonical form of
`project id, name, cmd, cwd, env` plus the git blob id of each `pin` file at HEAD. `when`, `timeout`, `notify` and
`hosts` are not hashed: they cannot make the job run other code. Approvals live in
`$XDG_CONFIG_HOME/vega/schedule-approved.toml` (host-local, never deployed, not in any repo):
`"<project id>/<name>" = "<hash>"`.

- `vega schedule approve <project>/<job>` prints the definition and the diff against the approved one, then
  stores the hash. A new or changed job lists as `unapproved` with that command as the next step; vegad also
  sends the UAC one note per new hash, so nobody has to poll.
- Same model as `mise trust` and direnv `allow`: per host, per exact content, revoked by any change.
- Who approves: the user, typed in a shell or with `!` in a pane. P7 makes the box the boundary: a local agent can
  write a crontab anyway, so the approval defends against remote pushes, not against local agents. The guide
  tells agents to ask the user and never approve on their own; the UAC records the request as a `slayer-decision`
  bead when the user is away. No technical agent block (the env markers would also block the user's `!` call).
- Definitions come from committed `HEAD`, not the working tree: an uncommitted edit cannot change what runs.
- Residual risk, stated in the guide: the hash pins the entrypoint, not the code it reaches. `just fristen-execute`
  runs whatever the committed `justfile` and scripts say. `pin` closes that for the files a project names
  (brainz: `justfile` and the script it calls); everything else is as trusted as the project's main branch
  (its review rules decide what lands there). The job also refuses to start when the main checkout's HEAD is not
  on the default branch (`skipped: checkout on <branch>`), so a local experiment branch never runs unattended.

## When

- `daily = "HH:MM"`, `weekly = "mon|tue|…|sun HH:MM"`, 24 h clock, the host's local time zone.
- DST: a local time that does not exist (spring forward) runs at the first valid minute after it; a time that
  occurs twice (fall back) runs at the first occurrence.
- No cron expressions in v1: no consumer needs them, and a cron parser plus DST rules is the largest part of the
  code. A later `cron = "…"` key is additive. Sub-daily intervals (`every = "6h"`) likewise wait for a consumer.

## Running

**Catch-up.** A scheduler thread in vegad wakes every 30 s and, per approved job, computes the latest slot ≤ now.
If that slot is newer than the job's `last_slot`, vegad handles it once, crash-safe in three steps (`state.json`
per job, atomic replace): (1) write the slot as `pending`; (2) spawn the runner, or record the skip in the slot's
run file; (3) once the slot's run file exists, move `pending` to `last_slot`. The runner creates the slot's run file
under the job lock as its first act, with `outcome = "started"`, and exits at once if it already exists, so a respawn
is idempotent. After a crash or restart, a `pending` slot without a run file is spawned again; one with a run file
completes step 3. A run file still `started` whose job lock is free (the runner died) becomes `lost` and mails like
a failure. Guarantee: a slot runs at most once and never ends silently; a slot whose runner died is reported, not
retried (a half-run job may not be safe to repeat).
Many missed slots (a week asleep) give one run, the most recent. After the first approval the clock starts at the
approval: no instant run of slots before it. `catchup = false` skips a missed slot older than 15 min instead (for
jobs where late is worse than never).
After reboot nothing runs until herdr starts vegad; the first tick catches up.

**Runner.** vegad does not run the command itself. It spawns `vega-schedule exec <project>/<job> --slot <ts>` in
its own session (setsid). The runner takes the job's lock, re-checks the hash, runs `cmd` with stdout and stderr
appended to the run's log file (no pipe to vegad), enforces the timeout (SIGTERM to the process group, SIGKILL
after 10 s) and fills the slot's run file `<slot>.json` (start, end, exit code or signal, outcome). So a vegad
self-exec or restart in the middle of a job loses nothing: the next tick reads the run file.
`vega schedule run` uses the same runner.

**Overlap.** One flock per job (`$XDG_STATE_HOME/vega/schedule/<project>/<job>/lock`). A slot that comes due while
the job runs is not queued: vegad records it as `skipped (running)` and advances `last_slot`, so the run that
ends later does not pick it up again; the next run is the next slot. A runner that finds the lock taken (a manual
`vega schedule run`) records the same skip. Manual runs never touch `last_slot`. Jobs of different projects run in
parallel; `hosts` keeps one project's job off other hosts.

**Environment.** vegad's env (no agent markers, no shims on PATH: vegad.md, Lifecycle) plus `env`, plus
`VEGA_SCHEDULE_JOB=<project>/<job>`, `VEGA_SCHEDULE_SLOT` (RFC 3339) and `VEGA_SCHEDULE_NOTIFY` (a file path).
stdin is `/dev/null`.

**GUI session.** vegad is a descendant of the herdr server, which the user starts in their login session, so on
macOS it shares the Aqua bootstrap namespace a LaunchAgent would get. Whether EventKit grants the permission to
a process with this ancestry (TCC's responsible process after setsid) I don't know: step 1 below proves it on
proteus with the real `fristen-execute` before anything else is built. If it fails, the job declares
`session = "gui"` and the runner hands it to `launchctl asuser` (fallback, not the default).

## Output and logs

- Per run: `$XDG_STATE_HOME/vega/schedule/<project>/<job>/<UTC slot>.log` and `.json`. Keep the last 20 runs per
  job and nothing older than 30 days; pruning happens on each run.
- vegad's own log gets one line per start and end, never the job's output.
- State that survives restarts: `last_slot` (the last slot started or skipped) and `pending` in `state.json`,
  outcomes in the run files, approvals in config.

## Mail

- Recipient: the project's live UAC, resolved through the registry on any host (as PagerDuty does). In local mode
  (no broker) only a UAC pane on this host gets it (local tool-mail delivery). No UAC: a durable board note.
- Sent as `tool vega-schedule`: on `failed` (exit ≠ 0), `timeout`, `lost`, `spawn error` and `skipped (checkout …)`;
  never for `skipped (running)`. Body: job, host, slot, outcome, exit code, duration, the last 20 lines of output
  (at most 2 KiB), the log path and `vega schedule show <job>`.
- A failing daily job mails each failed run; the next success after a failure mails one `recovered` line.
- `notify = "always"` mails every run. Any job may write text to `$VEGA_SCHEDULE_NOTIFY`: if the file is non-empty
  at exit, its first 2 KiB are mailed even on success (`fristen-execute`: "3 deadlines today").
- Job output is untrusted data in the mail; the UAC rules already treat tool mail bodies that way.

## `vega schedule` (AXI)

```
vega schedule                     # home: jobs for this host
vega schedule show <project>/<job> # definition, hash, approval, last 5 runs, log path, tail of the last log
vega schedule approve <project>/<job>
vega schedule revoke <project>/<job>
vega schedule run <project>/<job>  # run now through the runner; respects the lock; no approval, no run
vega schedule log <project>/<job> [--run <slot>] [--full]
```

Home view (TOON, AXI 1, 4, 5): one row per job declared for this host in any known project:
`job, when, next, last (outcome, age), state`. `state` is `ok`, `unapproved`, `changed` (approved hash differs),
`invalid`, `no checkout`, `running` or `vegad stopped`; every non-`ok` row carries its next step once below the
table. Empty state: `0 jobs: no .vega/schedule.toml declares this host (<n> projects checked)`. Exit 0 unless the
command itself failed. v1 is host-local; jobs of other hosts appear only after a KV mirror (later step).

Plugin layout follows CI watch: `crates/vega-schedule` (parse, next slot, hash, approvals, run files),
`plugins/schedule` (`vega schedule` and `exec`, `--skill` guide), `plugins/post/src/schedule.rs` (vegad thread).

## Why not launchd

| Option | For | Against |
| --- | --- | --- |
| **vegad scheduler** (chosen) | one place for trust, mail, list and logs; same code on Linux hosts; already running wherever agents work | runs only while vegad runs (after a reboot, from herdr start); EventKit ancestry unproven (step 1) |
| vega-generated launchd plists | runs without herdr; `StartCalendarInterval` coalesces missed runs on wake; TCC in the GUI domain is known | macOS only (Linux needs systemd timers: a second generator); plists drift from the toml and need install/uninstall; trust, failure mail, list, logs and overlap still have to be built around it; vega removed launchd on purpose (vegad.md) |
| cron / hand-written plists per project | zero vega code | no catch-up on cron, no UAC view, no failure mail, no trust check: exactly the request's problem |
| Host-local job config only (`~/.config/vega/schedule.toml`) | no trust problem at all | jobs leave the project; a second host or a fresh clone loses them; the project UAC cannot propose a job by commit |
| Checked-in file, no approval | simplest | any collaborator push runs code on the host |

Recommendation: the vegad scheduler. Revisit only if a job must run while nobody has started herdr, or if step 1
shows that EventKit refuses vegad's descendants and `launchctl asuser` does not fix it.

## Implementation plan (beads)

1. **Spike: GUI session on proteus.** Run `just fristen-execute` from a process started like vegad (setsid child of
   the herdr server) and record whether EventKit works; try `launchctl asuser` if not. Done when the bead notes
   the result. Decides `session = "gui"`.
2. **crates/vega-schedule: model.** Parse and validate `schedule.toml` (unknown keys fail), next-slot and
   latest-slot with DST rules, canonical hash, approvals file, `state.json`. Unit tests for DST, catch-up
   arithmetic (a skipped slot never runs later; a crash between the steps neither repeats a slot nor
   ends it silently), hash stability.
3. **Runner: `vega-schedule exec`.** Lock, hash re-check, default-branch check, env, log file, timeout with process
   group kill, run file, pruning. Tests with fake jobs (exit codes, timeout, overlap).
4. **vegad thread.** Discovery of main checkouts, minute re-read, 30 s tick, catch-up, spawn the runner, read
   run files and `pending` after restart. Tests with a fake clock and a temp state dir.
5. **Mail.** Failure, recovered, always and `$VEGA_SCHEDULE_NOTIFY`; registry routing, local-mode delivery, board
   note without a UAC; one note per new unapproved hash.
6. **CLI.** `vega schedule` home, `show`, `approve`, `revoke`, `run`, `log`; `--skill` guide (with the trust rules
   for agents) and `assert_guide` test; doctor conformance.
7. **brainz rollout.** brainz commits its two jobs (with `pin`), the Slayer approves them on proteus, one real run
   each; CHANGELOG and vegad.md updated.
8. Later, each on a consumer's request: KV mirror for a cross-host list, `cron`/`every`, board column.

## Open questions

- Should `pin` be required (at least the file that defines `cmd`'s target), or stay optional?
- Is one mail per failed run right for a daily job that stays broken for a week, or one per day at most?
