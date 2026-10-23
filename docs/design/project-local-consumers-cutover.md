# Remaining local project consumers

vega-lhc.14.5 completes the local consumer portion of
[project identity](project-identity.md), building on the
[registry protocol](project-registry-cutover.md) and
[per-location sync storage](project-local-sync-cutover.md). The completed
`identity` branch includes the .14.6 startup journal, application, rollback and
all-host audit. Deploy it as a coordinated flag-day cutover using the
[operator runbook](project-registry-cutover.md#operator-runbook-146).

## Runtime contracts

- Lane ownership and filenames use project IDs. Each JSONL record carries
  `schema: 2` and `project`; actor/pane/since retain their local contracts.
  `record_if_uac` uses the ID UAC API, including across local clones, and reports
  identity/configuration failures to its CLI caller. bd actors remain local.
- Badges use `badges/v2/<hex(ProjectId)>.json` and validate schema and ID.
  The board uses the shared resolver. Notes carry schema 2 and project IDs;
  their pane-keyed/source-host IDs, causes and watermarks do not change.
- vegad resolves its owner-local herdr snapshot before joining membership by
  ID. It never reads a remote registry path. Pane/workspace joins, board
  placement, home-project detection and hygiene displays use IDs. Failed
  local resolution skips that project and logs a diagnostic once per change.
- Decisions execute in the claimed local checkout. Cache entries use ID and
  common directory; journal writes in that clone invalidate the cache before
  its normal refresh interval. A sibling clone never wins by registry order.
  Resolution failures retain the cached decisions. After a daemon restart,
  failures retain and refresh the last saved decision count even beyond the
  board's normal freshness window, rather than publishing an unknown zero.
- v2 UAC claims retain primary `common_dir` and `checkout`, plus a `locations`
  list after migration or claim updates. `project_uac_locations` projects all
  owner-local locations with their expected ID. Git sync checks that ID before
  using the location, deduplicates linked worktrees by common directory and
  retains independent clones. The older local `uac_records`/`uac_of` location
  projection is also backed only by these v2 records.
- PagerDuty reads committed ownership from local locations and groups by ID.
  Equal service sets coalesce. Divergent sets for one ID are an error with a
  board diagnostic, never a union or a guessed winner. A registered clone's
  read or resolution failure blocks its expected
  ID while unrelated projects remain routable. Routing receives IDs
  directly. Its state version is 2; delivery receipts retain their original
  recipient, incident status and timestamp after project references migrate.
- Upgrade delivery remains restricted to panes inside the installed checkout;
  UAC discovery compares IDs. Hygiene daily records use IDs and schema 2.

The bd shim preserves real bd stdout/status in unregistered or non-git
locations and when identity cannot resolve. A registered common directory
resolving to a different ID denies manual Dolt push/pull until writers are
quiesced and an explicit journaled rekey is performed. This is documented in
P2 and the shim README. `VEGA_BD_MANUAL=1` retains its existing bypass.

## Local migration plan for .14.6

`vega_herdr::cutover::prepare(snapshot, mappings, local_host)` is a pure
projection. Snapshot paths are relative to the **vega state root**, whereas
`vega_bd_sync::cutover::prepare` takes paths relative to its **bd subdirectory**.
If the registry host is a configured alias for the old local hostname, use
`prepare_with_aliases` with aliases explicitly verified on this owning host;
foreign host ownership is never inferred from a payload.
Both return serializable plans of complete target bytes and legacy source
paths to remove. They do not wire startup, write files, merge Dolt data or
implement rollback. Resolve mapping locations on their owning host only and
validate every owner plan before applying any of them.

The herdr plan handles legacy UAC claims (including tracker and aliases),
uac-status, lanes, badges, note project references, hygiene daily keys and
PagerDuty receipt project references. It retains every local claim location
and alias when the same pane owns duplicate clones, with earliest claim time.
Different panes/hosts or incompatible tracker/other durable claim fields
block the plan. Lane actors retain every start record; readers take the
first start per actor. Incompatible badge, status and hygiene collisions also
block instead of choosing a timestamp winner. Missing mappings, lossy
filename collisions and malformed records block preparation.

Persist the exact original snapshots, mappings and both plans in the single
startup journal before applying writes or deleting sources. Replaying a
persisted plan after interruption does not rediscover identity or resnapshot
a partial migration. A rollback restores original bytes and removes newly
created v2 targets from the journal while writers remain quiesced. Keep
historical bd backup directories. Pane state and `notes/seen.json` do not
need projection and are not changed by these helpers. Note IDs, read/clear
watermarks, causes, timestamps, stop/resume flags, Git sync fields and
PagerDuty delivery receipt times remain intact.

Tests cover merged local claims, conflicting ownership/trackers, reversible
filenames, unmapped notes, forks, original-byte preservation and partial-plan
replay. Consumer tests exercise duplicate clones, bare and linked checkouts,
badge ownership, per-clone decision cache invalidation, PagerDuty equal/divergent
claims and upgrade delivery's local checkout restriction. The startup barrier,
interrupted-apply/rollback and offline-host rejoin fixtures are implemented in
.14.6 (`plugins/post/src/identity.rs` and `plugins/post/tests/identity_cutover.rs`).
The UAC audits all plans before deployment; each host migrates, verifies and
resumes locally. Old hosts are briefly invisible to v2 routing during deployment.

## Main integration before cutover (.14.11)

Orchestration membership uses schema-2 registry agent `project` IDs, with no
`repo_key` or cwd fallback. Explicit waits and asks remain keyed by host/pane,
thread and harness session; they carry no project reference to migrate.
Landing authority resolves the checkout ID and checks the v2 project UAC claim.
Landing receipts remain keyed by canonical Git common directory because they
journal operations in one local clone. Mux reopen spawn records remain pane-keyed
and retain the local cwd and harness session for resuming that exact directory.
Neither record kind requires a cutover mapping or filename rewrite. Hygiene's
24-hour decision threshold uses the existing schema-2 project state and badges.
Codex SSH argument forwarding continues to use local environment configuration.

### Cutover numbering audit

The UAC's .14.11 task brief (w3V:p1, 2026-10-21 12:43 CEST), recorded in
vega-lhc.14.11, explicitly directs: "Renumber CHANGELOG blocks so main's blocks
stay as they are and identity's blocks come after main's highest number."
Accordingly, the merge retains main's 50–56 byte-for-byte and renumbers only
identity's held 50–53 to 57–60. This applies the nondeployable integration
contract in [the registry cutover handoff](project-registry-cutover.md).

Before publication, the UAC checked installed builds and confirmed by post
message 6443 (from w3V:p1, 2026-10-21 12:56 CEST): proteus runs main 67437b1;
ganymede and deimos run 21e17d6; io is offline, last build 159c713. The UAC also
checked that identity commit 45590e8 is not an ancestor of main 67437b1 and
confirmed that only main builds have been installed, so no host's onboarding
watermark has consumed the held identity blocks. This is the UAC's deployment
audit, not an inference from the local worktree. Its confirmation is retained
in vega-lhc.14.11. Future published blocks retain their immutable numbers.

Second sync (2026-10-22, vega-lhc.14.13): main blocks 50–61 remain byte-for-byte
unchanged; identity blocks 57–61 are now 62–66, above main’s highest block.
The same UAC instruction applies before the flag-day deployment.
The held blocks carry the 2026-10-22 integration date so changelog dates remain
monotonic after main’s 2026-10-22 blocks; their entry text is unchanged.
