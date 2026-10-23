# Project registry v2 cutover

This is the registry/UAC/post/lanista portion of
[project identity](project-identity.md). It ships as a flag-day protocol change.
The completed `identity` branch includes the registry protocol, every local
consumer and the .14.6 startup migration. Historically .14.4/.14.5 were partial
builds held on this branch and were not deployable on their own. The finished
build may now run against unmigrated state only through the approved startup
plan: it stays alive quiesced until migration and verification pass. The UAC
lands and deploys the finished cutover on all hosts with the Slayer's OK using
the runbook below. Legacy access is restricted to audited cutover tooling;
there is no compatibility reader or dual protocol. Offline hosts must remain disconnected until upgraded and migrated. The tooling does
not itself deploy hosts or change broker permissions.

Registry UAC keys are `<host>.uac.v2.<hex(ProjectId)>`; pin keys are
`<host>.lanista.v2.<hex(ProjectId)>.<hex(seat)>.<hex(lane)>.<hex(tier)>`.
Payloads contain `schema: 2` and the full project ID. Readers reject old keys
and mismatched key/payload pairs. Agents retain pane-keyed registry keys and
local `repo_key`/cwd metadata, adding `schema: 2` and `project` when resolution
succeeds. Local UAC and status files use `<kind>/v2/<hex(ProjectId)>.json`;
pane-keyed records are unchanged. The ID APIs `project_uac_of` and
`project_uac_records` serve registry/routing consumers. The existing local
`uac_of` and `uac_records` APIs project `common_dir` metadata from those v2 files
for filesystem consumers; they do not read legacy files or interpret IDs as
paths. Local claim/consumer host comparisons use `post_host`; bd actors keep
their existing local host spelling. Local claims retain common_dir and published
origin/folder aliases (including merged alias lists); routing reads aliases without opening remote paths.

Owner mappings resolve legacy Git directories on their own host. A separate
Git directory (for example `~/src/brainz.git` with its checkout under
`~/Documents/brainz`) must have `core.worktree` set to the actual checkout
when discovery starts from the Git directory. The resolver accepts checkout
paths directly and reports `project_checkout_unknown` with this repair hint
when a separate directory has no checkout metadata.

Inbox-only hosts can retain local UAC, status and badge records. These migrate
and roll back locally like owner records; their plan still has no registry
backup, projection or owned publications. No broker write rights are required.

## Exact inbox-only grants

Only orrery may modify the broker, with the Slayer's own approval. The UAC
must compare the existing allowlist against the planned projection before
cutover. Never grant registry enumeration, watches, writes, deletes or wildcard
subjects to deimos. Catalog and theme subjects do not change.

The direct-get subject pattern is
`$JS.API.DIRECT.GET.KV_VEGA_REGISTRY.$KV.VEGA_REGISTRY.<exact-key>`.
For proteus and `github.com/durandom/vega`, these concrete examples are:

```text
$JS.API.DIRECT.GET.KV_VEGA_REGISTRY.$KV.VEGA_REGISTRY.proteus.uac.v2.6769746875622e636f6d2f647572616e646f6d2f76656761
$JS.API.DIRECT.GET.KV_VEGA_REGISTRY.$KV.VEGA_REGISTRY.proteus.lanista.v2.6769746875622e636f6d2f647572616e646f6d2f76656761.6167656e7473.636f646578.627564676574
```

The pin example is seat `agents`, lane `codex`, tier `budget`. Repeat only for
the writer/project/seat/lane/tier tuples deimos is authorized to read, and set
`VEGA_LANISTA_PIN_HOSTS` to those writer hosts. `auth-config --registry-read`
accepts these exact keys and rejects legacy or malformed project keys. The
cutover coordinator can print `Plan::direct_get_subjects()` from each durable
owner plan to list exact candidate subjects from the snapshot and migrated
projection; orrery selects the authorized subset, never grants the whole list
by default.

Pin resolution with an explicit host list does exact direct gets. Current
`post --uac` discovery enumerates beats/agents/UACs and therefore remains
unavailable to an inbox-only host, which is also forbidden to send mail.
There is no new inbox-only publication right: reconcile and pin mutation need
owner write rights that deimos does not hold. Beat and agent keys retain their
existing spellings; UAC and pin keys are the project-key grant changes.

The local broker fixture `project_pins_cross_hosts_under_rendered_grants`
verifies v2 UAC and pin direct gets, denial of other keys, writes and stream
info, and cross-host pin resolution under rendered authorization. This tests
permission generation in isolation; live permission verification belongs to
orrery before the UAC cutover.

## Quiesced projection and rollback handoff

1. Stop project writers on every host. Snapshot owned registry UACs and pins
   and persist the snapshot before deleting anything, including expired pins
   and clear tombstones. Resolve old paths on their owning host only.
2. Prepare `cutover::Plan` for each host from its snapshot, journaled old-key
   to ProjectId mapping, and publication from migrated local state. Missing
   mappings and same-host collisions fail preparation. Persist each plan and
   backup in the startup journal; these library helpers do not persist it.
3. Call `cutover::validate` with **all** host plans before any `Plan::apply`.
   Different UAC panes/hosts for one ID and divergent pins for the same
   ID/seat/lane/tier block migration. No timestamp winner resolves a migration
   conflict. At steady state pins retain greatest `(at, host-token)` ordering.
4. Apply each owner's plan using its owner connection: delete only that owner's
   old project keys, then publish the v2 projection. Ordinary reconcile leaves
   legacy UAC and pin keys intact; only this backed-up cutover deletes them. Replay the persisted plan
   after interruption; never replace its backup with a partial live snapshot.
   Retain merged aliases in migrated local UAC records before normal reconcile
   resumes. Ordinary reconciliation republishes agents and other migrated local state.
5. Verify exact direct gets and owner publication per host, then resume that
   host's workers. Orrery removes obsolete exact grants before deployment.
   The UAC deploys hosts one after another; during that interval old hosts are
   invisible to v2 routing, which briefly fails without durable data loss.
   External writers stay stopped until the UAC verifies every host. A failed
   host stays quiesced. The .14.6 coordinator restores local files and old registry
   payloads from durable backups before old binaries resume; keep backups.

`Plan::apply` is a projection operation. Durable owner startup and rollback
live in `plugins/post/src/identity.rs` (.14.6); the UAC coordinates deployment
with an audited all-host bundle rather than a distributed runtime barrier. Local consumer
migrations are implemented by vega-lhc.14.5; see
[the local consumer handoff](project-local-consumers-cutover.md). PagerDuty now
groups local ownership by ID and rejects divergent committed clone claims.
Access live legacy state only through audited cutover tooling during that
coordinated migration.

## Operator runbook (.14.6)

The UAC coordinates this single flag-day deployment with the Slayer's OK.
Each host has a local startup gate checking its files and the audited inventory;
there is no distributed lock or broker provisioning. Stop vegad and every project writer on all hosts
(claim/lane/badge/note/pin mutations, Git/bd sync and manual Git/Dolt writes).
Keep external writers stopped until verification. Let old heartbeats age past
the 90-second freshness window before starting new daemons.

The first migration needs the audited `approved.json` and a reachable broker; until
it completes, offline local-mode jobs stay off. Complete this online cutover once
before using offline local mode. An offline empty-host bootstrap is not supported.

1. **Preview.** Run the completed build directly from the deployment checkout:
   `vega post identity mappings --output mappings.json`, then
   `vega post identity preview --mappings mappings.json --output host-plan.json`.
   Mappings discovers local claims/status, badges, notes, hygiene, PagerDuty and
   bd references, plus this owner's legacy registry UAC/pin payloads (including
   expired/cleared pins). It uses the local resolver, expands `~/` on this host,
   and writes the JSON array `{old_key, project, location: {common_dir, checkout}}`
   with canonical absolute paths. Both commands are read-only on source records;
   owners need a reachable broker; inbox-only hosts skip registry enumeration.
   Resolution errors list keys with hints and exit 1 without replacing the export.
   Restore missing checkouts or select `vega.projectRemote` for ambiguous remotes.
   Orphan lossy filenames cannot recover paths: restore their project metadata
   from backup before retrying. Keep writers stopped between mappings and preview.
   Preview takes the relay lock and exports original file bytes, all owned
   registry payloads (including expired/cleared pins), and complete pure plans.
   Output must be outside the live vega state root. Source records are unchanged.
   Treat exports as private backups; historical bd backups remain untouched.

   Include offline hosts in the inventory: bring them online quiesced for preview
   and upgrade, or keep them disconnected until a new audited inventory. Never
   let an old binary rejoin. Inbox-only hosts generate an empty mapping array and
   `--registry-read <exact-v2-key>` for each authorized project key; they must
   have no local project writer state. Theme/catalog permissions stay unchanged.

   Orrery, with the Slayer's approval, compares/updates actual effective broker
   grants, removes obsolete exact project grants and exports a JSON object
   mapping each inbox host to `{ "publish": [<full effective allowlist>],
   "subscribe": [<full effective allowlist>] }`. Export actual permissions,
   never seeds. With no inbox hosts use `{}`. Run on the UAC:
   `vega post identity audit --plan a.json --plan b.json --host a --host b --grants grants.json --output approved.json`.
   Repeat plan/host for the full inventory. Audit rejects missing/duplicate hosts,
   mixed schemas, conflicting claims/divergent pins, missing exact reads, broad
   or obsolete registry grants, writes, enumeration and watch subscriptions.
   Printed candidates are for orrery's authorized-subset comparison only.
   That authorized subset may be empty; no new project read grant is required.

2. **Deploy order.** Copy the same audited bundle to
   `$XDG_STATE_HOME/vega/identity-cutover/approved.json` on every host (default
   `~/.local/state/vega`). Deploy the completed binary to all hosts with writers
   stopped; start owner daemons, then inbox-only daemons. Before spawning workers
   or ordinary publication, vegad checks source bytes and owner-local identities,
   then durably saves `identity-cutover/journal.json`. It never replaces this
   original backup. It applies atomic target files, deletes legacy sources,
   publishes the registry plan and verifies exact v2 direct gets and deletions.
   It republishes/verifies pane-keyed agents and notes, then writes
   `identity-cutover/complete.json`. Inbox-only startup checks authorized exact
   values with its own credentials and probes forbidden enumeration/ungranted
   reads. First migration requires the broker.
   Negative probes require explicit permission-denial events for their exact
   subjects: timeouts, disconnects and allowed requests fail startup. Registry
   aliases and earliest claim times are retained in local UAC files before
   ordinary reconciliation can publish them.
   A failed startup keeps vegad alive quiesced, reports each changed error once
   in its log/status, and retries every five seconds without spawning project
   workers or publishing. Self-exec remains available for a repaired binary;
   even `relay --once` waits for a valid plan before its normal one-shot work.
   Identity CLI subcommands still fail fast. Interruption replays the journal.
   Each owner finishes locally
   while the UAC holds external writers stopped. Hosts are deployed one after
   another: each migrates, verifies, then resumes its local workers. During
   this interval routing to old hosts briefly fails because they are invisible
   to v2 readers; durable data is retained. There is no migrate-only phase or
   cross-host runtime lock.

3. **Verify.** On an owner run `vega post identity verify`: every planned owner
   must have a fresh schema-2 heartbeat, all planned UAC/pin direct gets must
   match and no legacy project keys may remain. Check inbox daemon completion,
   every host's completion marker/logs, and compare stopped bd locations,
   trackers, notes/watermarks and pin times/tombstones against previews. Then
   release external writers. Unapproved offline hosts cannot migrate/rejoin.
   Completed hosts retain offline local mode; every registry reconcile checks
   fresh old/unplanned hosts and every live legacy project key before ordinary
   publication, including byte-identical backed-up keys that reappear. Only the
   initial migration permits planned legacy keys while other owners cut over;
   startup publishes this owner's verified v2 heartbeat before completion.
   While other owners still have legacy keys, ordinary publication stays
   blocked but the completed owner's heartbeat refreshes every 15 seconds.
   Detection does not replace keeping old hosts disconnected.

4. **Rollback.** Stop all writers and daemons again. On every host with a journal,
   run the new build's `vega post identity rollback`. Under the relay lock it
   persists rollback intent before restoration, removes new targets/publications,
   restores original file bytes and all owned registry payloads, then removes
   completion. Repeat after interruption; rollback intent blocks forward startup.
   Orrery restores old grants with the Slayer's approval; deploy old binaries
   on all hosts, verify the old projection, then release writers. Keep journals,
   previews and historical bd backups. Never remove intent to resume new code
   or resnapshot partial state. Snapshot rollback is for the quiesced flag-day
   window; after writers resume, recovery needs separate coordination to retain
   newer work.

## Rehearsal on state copies

Run mappings and preview on each host with `XDG_STATE_HOME` pointing at a
separate state copy, excluding locks and logs. Audit all previews with the
real grants export. Startup and the rollback CLI can mutate registry state;
use the opt-in file rehearsal for copies instead:

```sh
VEGA_IDENTITY_DRYRUN_ROOT=/absolute/copy/vega \
VEGA_IDENTITY_DRYRUN_BUNDLE=/absolute/approved.json \
RUST_TEST_THREADS=1 nice cargo test -p vega-post \
  identity::tests::real_copy_startup_and_rollback_without_broker -- --ignored --exact
```

Run it on each host with the audited bundle. It uses the startup journal
preparation, file application and restoration helpers, skipping every broker
operation. It verifies local repository locations, persists the journal,
interrupts and replays file application, writes a simulated completion marker,
then restores original bytes twice and checks that rollback intent blocks
forward startup. The marker is simulated; broker permissions and publication
still need cutover verification. Keep rehearsal journals separate from live
state. Before repeating, make a fresh copy and preview: rollback intent is
retained.
