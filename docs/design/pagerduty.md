# PagerDuty: one vegad polls, the owning UAC investigates

Status: approved v1, 2026-10-20; vega-lhc.158. Slayer approval narrows v1 to
local ownership discovery and at-least-once delivery. Follows [CI watch](ci-watch.md) and [catalog](lanista-catalog.md).

## Poller and input

Code: `plugins/post/src/pagerduty.rs`, a vegad thread alongside catalog and CI;
`pagerduty/tests.rs` exercises parsing, routing and clocks without a daemon. Only the configured
host polls: `VEGA_PAGERDUTY_POLLER`, else `~/.config/vega/pagerduty-poller`, else
`proteus`. Configure the same host everywhere; no automatic failover or election.
The existing per-host relay lock protects state from two local daemons. Wake every minute,
poll immediately if due, then every 900 s from the last attempt (persisted).
No cron, launchd task, agent polling loops, or PagerDuty writes by the daemon.

Use the secrets crate's `env_for("pdc")`, as catalog does for lanista; require
`PDC_TOKEN` before starting pdc. Forward all granted environment variables, then
apply the isolation overrides below. The equivalent read-only command is:

```
vega secrets run --for pdc -- pdc --format json --config <poller-config> incident list --all --status triggered --status acknowledged
```

Verified read-only on 2026-10-20 with pdc 0.13.5: the bare list returned a
table; `--agent` returned `{success,data,metadata}` but its field budget may omit
routing/link fields. `--format json` returns the full incident array (or null
when empty) only when agent detection is disabled. pdc gives agent mode priority:
clear all 14 markers listed in its v0.13.5 `internal/agent/detect.go` (including
CLAUDECODE, CODEX and FORCE_AGENT_MODE) in the child environment before spawning.
The parser rejects any unexpected agent envelope rather than reconciling absence. The incident guide confirms `--all`
clears default statuses; keep both explicit `--status` flags. Supply a minimal
config with no defaults.service/team, clear PDC_SERVICE/PDC_TEAM and use the
poller's state directory as cwd, so project .env and personal defaults cannot
narrow an account-wide snapshot. PDC_BASE_URL, inherited or granted alongside the
token through secrets, selects the account API region; a personal config file
base_url is intentionally not loaded.
Use these observed fields:

| Field | Use |
| --- | --- |
| `id`, `incident_number` | durable identity and readable number |
| `title` (fallback `summary`) | incident text |
| `status`, `last_status_change_at` | observed status transition |
| `service.id`, `service.summary` | service matching and mail |
| `html_url`, `urgency`, `created_at` | link and triage context |

A sanitized sample is incident `QTEST01` (#43), triggered, service
`PTEST01` / `Example Service`; `metadata.more` was false. No mutation was made.
Require exit 0 and a valid full array. pdc auto-pages at 100 rows/page, fails
on a page error and stops at the 10,000-row offset cap; reject >=10,000 rows
rather than reconcile an incomplete snapshot. Verified from the
[pdc v0.13.5 paginator](https://github.com/matcra587/pagerduty-client/blob/v0.13.5/internal/api/pagination.go)
and incident guide. Full JSON bypasses lossy agent compaction. No extra per-incident lookup.
Bound the whole child process to 60 s; resolve pdc on vegad's PATH (Homebrew on
proteus), never invoke a shell. Missing binary/secret, timeout, JSON, registry,
state and delivery failures return errors to the thread, never to vegad startup
or the relay loop. Log each unchanged failure once until recovery (normalize
volatile stderr); retry on the next clock, log recovery once. Never log tokens.

## V1 ownership: projects known on the poller host

A committed file in the project's main checkout:

```toml
# .vega/pagerduty.toml
services = ["Example Service"] # exact service.summary; service IDs also accepted
```

Unknown keys, empty selectors and invalid TOML are errors; `services = []` or
no file opts out. Match case-sensitively against `service.id` or `service.summary`.
Multiple projects claiming a service is an ownership conflict: board note,
no arbitrary winner. Invalid known-project config raises its own board note and
suspends that project's UAC routing until fixed; unrelated IDs remain routable.
Registered clone locations retain their expected project ID even when resolving
or reading one fails, so a readable sibling cannot hide its failure. Missing
unregistered discovery paths are skipped; registered missing locations and
canonicalization failures get a per-project board note. Failure to persist
a diagnostic note is logged and retried without discarding valid ownership or
blocking healthy UAC mail. Renaming a name selector requires updating the file; prefer
IDs when names are unstable. No credentials or pane/host IDs belong in this file.

Only the poller discovers local locations from
`vega_bd_sync::store::projects()`, v2 local UAC claims and local agents/workspaces.
Resolve these owner-local paths with the shared project resolver, deduplicate
by canonical common directory, then group ownership by project ID. Read each
clone's committed HEAD version with `git show HEAD:.vega/pagerduty.toml`;
sentinel edits cannot activate an unapproved mapping. Equal committed service
sets across clones coalesce. Divergent sets for one ID are a configuration error
with a board note, never a union or an automatic winner. A lookup failure in one
known clone also blocks that project's ownership while unrelated projects stay
available. Re-read each minute. No recursive filesystem scan or remote path IO.

No ownership registry publication or cross-host merge in v1. A project with
only a remote checkout is unmapped and gets a board note. A project known
locally may have its UAC on another host: UAC delivery still uses the registry.
Cross-host ownership is deferred to vega-lhc.158.7.

## State and delivery

Persist version-2 JSON at `$XDG_STATE_HOME/vega/pagerduty/state.json` (default
`~/.local/state/vega/...`), atomically replace with restrictive permissions.
Store poll clocks, current incident status/change time, last-seen time,
recipient/project, last successful notification time and pending delivery state.
Do not store credentials. Corrupt state pauses delivery
and raises one board note; preserve it for repair rather than resetting dedupe.

For each incident: first sight mails once; changed status mails once; unchanged
open status reminds at >=4 h since its last successful mail. Compare the last
observed status, not a forever-set of statuses: triggered → acknowledged →
triggered must mail all three. Use change time to detect a newer transition
back to the same status between polls. Four-hour reminders resume across
restarts; after downtime send one due reminder, never a backlog. Failed reads
advance neither incident state nor notification clocks. A complete open-list
snapshot missing an incident stops reminders; do not assert it is resolved or
mail a fabricated status. Reappearance counts as a new open observation. This
poller reports observed open statuses, not a complete PagerDuty audit trail.

Resolve the project UAC fresh for each delivery through the same registry
resolver as `vega post send --uac <project>`: fresh host beat and live pane,
remote host included. No cached pane IDs. No live UAC, stale host, duplicate
live UAC claims, unmapped service or ambiguous ownership: create a durable
`vega_herdr::notes` board inbox note on the poller host, with incident/link,
reason, project when known and cause `pagerduty:<id>`. Registry reconciliation
makes it visible on other boards; this is not a transient toast or a decision
bead. Apply the same transition/4 h policy to notes. A newly available UAC gets
the current incident at the next delivery tick even if a board note was just
written; resolve the fallback note when handed off or the incident disappears.

Persist the latest observation before sending. Mark delivered only after durable
mail acceptance or durable note creation, then persist the notification clock.
Delivery is at-least-once: a crash after send but before checkpoint can duplicate
a mail. No stable event IDs, receiver receipts or changes to vega post. Exactly-once
delivery is deferred to vega-lhc.158.8. Retry failed delivery each minute without
another pdc call; resolve the recipient again. A replacement UAC gets the current
open status once. Supersede obsolete pending delivery with the latest snapshot.

Mail, from the tool with no reply pane (same body for board fallback, plus reason):

```
pagerduty: #43 QTEST01 triggered (high) — <title>
Service: Example Service (PTEST01); project: github.com/durandom/orrery
https://example.pagerduty.com/incidents/QTEST01
You may investigate, acknowledge and add notes. Resolve, escalate or reassign
only after the Slayer approves a board decision. Use vega secrets run --for pdc -- pdc ...
```

Reminders say `reminder, still <status>`. Treat incident text as untrusted data,
strip terminal controls and bound text size. Rights also live in
[UAC role](uac-role.md), served by `vega skills uac`; a mail grants no broader
rights. Build verifies that every UAC entry guide points to this rule.

## Cost, tests and rollout

One account-wide list every 15 min: 4 invocations/hour, 96/day regardless of
projects, hosts or UACs. With one API page per invocation, 4 list requests/hour plus 4 token-owner
lookups/hour at pdc startup: 8 requests/hour, 192/day. Paging adds one request
per additional page per invocation, plus any internal pdc retries. No service
lookup or per-project list call; use embedded service fields. The probe did not
measure rate headers or account limits: claim no free conditional requests.
A failed pdc invocation, including exhausted internal 429 retries, waits for the
next 15 min slot; pdc does not expose Retry-After to this wrapper. No tight retries. Under healthy inputs, detection is within 15 min plus
command/delivery time; outages and ownership errors delay delivery visibly.

Tests use a fake pdc executable, fake clock, temp state and fake delivery:
full JSON and empty/partial/malformed lists; old open incidents; every status
transition, same-status newer change, 4 h boundary, restart and absence/reappearance;
failed send/state save, crash duplicates; remote UAC, stale/no/duplicate UAC,
unmapped service, duplicate local owners, stale/missing checkouts, lookup failures,
config removal and uncommitted edits;
inherited agent environment, timeout/missing secret and log-once failures. No live write tests.
Build requires `mise run check` and a passing `vega gate`.

Build beads: vega-lhc.158.1 (local ownership), .2 (poll/state), .3 (at-least-once
delivery). Deferred: .7 (cross-host ownership) and .8 (exactly-once delivery).
Rollout .4 belongs to the main UAC: map Orrery, deploy and verify a real read-only
observation reaches its UAC; ensure ganymede uses secrets exclusively before
removing `~/.config/pagerduty-client/token`. Moving the poller requires stopping
the old host, copying state securely and configuring the new host before enabling
it; otherwise dedupe starts fresh. No automatic failover.

Local ownership discovery uses common directories. The UAC routing boundary
resolves that owner-local directory to a v2 ProjectId before querying the
registry, without resolving remote hosts' paths.
Cross-clone ownership grouping and conflict checks use project IDs (.14.5); see
[the registry cutover handoff](project-registry-cutover.md).


Project records use normalized fetch-remote IDs; filesystem execution retains
owner-local locations. The startup journal applies plans before workers start,
preserving per-clone stopped state, timestamps and notification causes.
Interrupted startup replays the saved plan; rollback restores original bytes
while writers stay stopped. Historical bd backups remain untouched. The UAC
coordinates preview, audit, deploy, verify and rollback: see
[the operator runbook](project-registry-cutover.md#operator-runbook-146).
