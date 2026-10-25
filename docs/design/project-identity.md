# Project identity

Approved design for vega-lhc.14 (Slayer decision vega-lhc.14.2), revised to a
flag-day cutover. One project is one
normalized Git fetch remote URL, independent of checkout path and host. This
is the replacement for path identity in lanista-catalog.md,
pagerduty.md, vegad.md, git-sync.md and beads-sync.md, implemented by
.14.3–.14.6; deployment follows the cutover runbook.

Local consumers are implemented by vega-lhc.14.5; the pure projection APIs and
startup-coordinator handoff are in
[project-local-consumers-cutover.md](project-local-consumers-cutover.md) and
[project-local-sync-cutover.md](project-local-sync-cutover.md). Startup wiring,
journal, audited host plans and rollback are implemented by .14.6 in
`plugins/post/src/identity.rs`; see the registry cutover operator runbook.

## Inventory (before the cutover)

| Path:line | Path-based identity or dependency to replace |
| --- | --- |
| `crates/vega-herdr/src/lanes.rs:38,57,70,85` | `project_key` = canonical common dir relative to HOME; spawn/adopt ownership and lane files use it. |
| `crates/vega-herdr/src/lib.rs:617,627` | `project_dir` derives a directory from the key; `in_project` compares herdr `repo_key` or cwd containment. |
| `crates/vega-herdr/src/state.rs:43,116,124` | Lossy filename encoding; UAC lookup/enumeration by path key. |
| `plugins/uac/src/main.rs:232,296,475,552,591,661,700,722,892,1218` | Own project, agent membership, claim/release, tracker lookup, answer, and `uac-status` cache. |
| `crates/vega-registry/src/lib.rs:80,139,182,206` | Lossy `<host>.uac.<project>` token; agent `repo_key`; UAC publication and local origin-name lookup treating key as git dir. |
| `plugins/post/src/reach.rs:78,89,104,122,144`; `plugins/post/src/main.rs:1217,1319,1324` | `--uac` exact path/folder/origin_name matching and `.`; gone-pane routing. vega-lhc.162 adds origin_name, not remote identity. |
| `plugins/lanista/src/main.rs:549`; `crates/vega-registry/src/pins.rs:33,49,129,145` | Project pins use path identity, already with reversible hex key encoding; cross-host latest/clear matching. |
| `plugins/post/src/pagerduty.rs:386,406,425,441,471,600` | Discover local paths, read committed ownership, group services and route incidents/board notes by path. |
| `crates/vega-bd-sync/src/store.rs:56,131,135,141,158,176`; `plugins/bd/src/main.rs:245,728,849,918` | Provisioned project key, safe filenames for projects/state/backup, init/status/resume selectors. |
| `shims/bd/src/main.rs:85,179` | Independent common-dir key computation; project/git-state lookup for sync hints and the Dolt push/pull guard. |
| `plugins/post/src/gitsync.rs:19`; `plugins/uac/src/main.rs:1160` | Pass path-shaped UAC claims to `git::repos` for local Git sync/status. |
| `crates/vega-bd-sync/src/git.rs:41,53,125` | Key interpreted as local common dir, deduplicated repositories, git-sync state filename. |
| `plugins/post/src/bdsync.rs:78,98`; `plugins/post/src/main.rs:3659,3673,3698,3901,3914,3933,4182` | Sync notifications/UAC delivery, vega home-project detection, decision cache, pane/workspace membership, badges and board placement. |
| `crates/vega-herdr/src/badges.rs:169`; `crates/vega-herdr/src/notes.rs:40,87`; `plugins/board/src/badges.rs:44` | Badge filenames, note project equality and board's independent common-dir key implementation. |
| `plugins/mux/src/main.rs:968,1336,1485` | Spawn/resume record lanes through path project identity; bd actor remains local. |
| `plugins/onboard/src/upgrade.rs:351`; `plugins/post/src/hygiene.rs:281` | Upgrade UAC selection and hygiene display assume a key is a path. |
| `plugins/post/src/auth.rs:212,301` | Inbox-only grants permit exact registry keys; new key spelling needs explicit grants. |

## Identity and local location

Add one shared resolver in `vega-herdr::project`, returning `ProjectId` and a
separate local `RepoLocation { common_dir, checkout }`. All project comparisons
use the ID; Git, bd, filesystem access and herdr integration use the location.
A host keeps an ID → **list** of locations, with the old common-dir key recorded only in the
migration journal. Never open a remote host's path on the reader's filesystem.
Publish `project` on agent and UAC entries; keep `repo_key`/cwd as location
metadata for herdr compatibility. Update every inventory row, including copies
of key computation; changing `lanes::project_key` alone is insufficient.

Use `git rev-parse --path-format=absolute --git-common-dir` and canonicalize it.
Read identity configuration from the common repository config, ignoring
worktree-specific/branch overrides. Linked worktrees, plain `.git`, `.bare`
with sibling worktrees, and standalone bare repositories all resolve alike.
Bound local Git subprocesses as in origin-name publication; no network lookup.
Return structured errors, never process::exit/panic/unwrap/expect on Git/config
IO. Daemon callers log once and skip the affected project; CLI callers report
the error and fail fast (preserve vega-lhc.140 daemon isolation).

Remote choice, in order:

1. Common-config `vega.projectRemote` names a configured remote explicitly.
2. Otherwise use `origin` if present, regardless of branch upstream/push remote.
3. Otherwise use the sole remote. Multiple remotes without origin require
   `vega.projectRemote`; return `project_remote_ambiguous`, never choose by sort.
4. No remotes: ID is `local:<host>:<canonical absolute common-dir>` (host is the
   registry host identity). HOME abbreviation is display only. These projects
   deliberately do not coalesce across hosts.

The selected remote's single fetch URL counts, not pushurl. Multiple fetch URLs,
missing explicit selection, malformed/unsupported URLs or failed config reads
are actionable errors, **not** permission to fall back to a local identity.
A fork with origin=fork and upstream=parent is a different project from parent;
selecting upstream explicitly opts into sharing the parent's project. Different
remote names with the same URL are the same project. Changing selected URL is
an identity change requiring the explicit rekey migration below.

Normalization is offline and credential-free:

| Input | Project ID |
| --- | --- |
| `git@github.com:durandom/orrery.git` | `github.com/durandom/orrery` |
| `ssh://git@github.com/durandom/orrery.git` | `github.com/durandom/orrery` |
| `https://github.com/Durandom/Orrery.git/` | `github.com/durandom/orrery` |
| `https://git.example/Team/Repo.git` | `git.example/Team/Repo` |

Parse HTTPS/HTTP, SSH, git:// and SCP syntax structurally; remove transport,
userinfo (never store/log credentials), trailing slash and one terminal `.git`.
Lowercase DNS host; lowercase repository path for github.com only, preserve case
on other servers. Remove transport-default ports (SSH 22, HTTPS 443, HTTP 80,
git 9418); preserve nondefault ports as `host:port/path`. Preserve namespace
components; reject empty paths, query/fragment, dot segments and encoded path
separators. Normalize unreserved percent escapes, preserve other escaped bytes
with uppercase hex. Bracket IPv6 hosts. Do not infer redirects, renamed repos,
SSH Host aliases or transport gateways; use the same canonical remote URL in
those clones. Relative/absolute local-path and file:// remotes become
`file:<host>:<canonical absolute target>` (relative to the common dir); they are
remote-derived local identities, never silently treated as absent remotes.

Same ID at `/Users/a/src/x` and `/srv/repos/x/.bare` joins one project. Several
live UACs still produce the existing ambiguity error; host freshness and claim
checks remain, and no cross-host winner is guessed. Same-host claims across
clones also conflict. `--uac` accepts full ID and `.`, plus published origin_name
and folder aliases from .162; aliases never determine identity. Deduplicate
aliases before counting candidates; collisions list full IDs and host/pane.

PagerDuty reads ownership from local locations, then groups by ID. Equal service
sets across clones coalesce; divergent committed claims for the same ID are a
configuration error with a board note, not a union. bd-sync keeps each local
clone's data, stop/resume and Git sync state independent: key records by
`(ProjectId, canonical common_dir)` on that host. Project status groups those
records; resume requires a location when several match. Never merge Dolt stores
or change database/remote names as a side effect of identity migration.

## Shared resolver API (vega-lhc.14.3)

`vega_herdr::project::resolve(path, registry_host)` returns a `Project` containing
an opaque, serializable `ProjectId` and `RepoLocation`. Pass the local registry
host identity explicitly; the resolver does not discover hosts or inspect remote
hosts' paths. `ProjectId::as_str()` is for comparisons/publication/display and
`ProjectId::encoded()` is the reversible hex storage component. The shared
`encode_component`/`decode_component` helpers also support UTF-8 location keys;
consumers must still supply their versioned namespace and payload validation.

`RepoLocation.common_dir` and `checkout` are canonical absolute paths. Linked
worktrees retain separate checkout roots and the same common directory; a bare
repository uses its own root for both. Resolving a plain `.git` directory also
finds its checkout. Fetch configuration comes directly from common-dir `config`
(including Git includes); worktree config and branch upstream/push selection do
not choose identity. Each local Git subprocess has a two-second bound and
returns `Failure { code, message, hint }` without echoing config values, URLs or
Git stderr. Callers choose CLI failure or daemon log-and-skip behavior.

`LocationCatalog::new(registry_host)` creates an in-memory catalog populated only
by its `resolve(local_path)` method. `locations(id)` returns every known local
checkout, including multiple clones; sync consumers group these by common_dir.
Repeated discovery deduplicates canonical locations. A changed ID for a known
common directory returns `project_identity_changed` and leaves the catalog
unchanged: rekey requires the journaled migration, then catalog reconstruction.
The catalog does not persist migration state or replace the startup migration.
All consumers now use IDs with separate owner-local execution locations.

## Migration and rollout

One coordinated cutover: deploy all hosts together. There is no compatibility
release, dual-read/dual-write period or later cleanup release. All consumers use
v2 after cutover; an offline host must upgrade and migrate before rejoining.

Use versioned project storage with reversible UTF-8 hex ID components (including
kind), e.g. `<host>.uac.v2.<hex(id)>`; local project files use the same encoding.
Do not reuse `token`, `safe`, `state_file_on`'s path substitution or
`badges.rs:169`'s encoder (every non-alphanumeric except `-` and `.` becomes `_`).
Keep pane-keyed state encoding unchanged. bd/git-sync files additionally encode
the location. Payloads carry the full ID and schema version for validation.

1. **Prepare the cutover:** provision exact v2 direct-get subjects for inbox-only
   hosts (deimos) and verify those grants with a v2 fixture before changing live
   keys. These hosts cannot enumerate the registry; never grant wildcard or
   read-write access. Catalog/theme grants whose keys do not change stay as-is.
   Quiesce project writers on all hosts and snapshot owner registry pins before
   deleting anything: ephemeral registry storage is not permission to lose pins.
2. **Migrate at vegad startup:** before any project worker or registry publisher
   starts, journal the old-key → ID/location plan and back up local files plus
   owned pin payloads. Resolve paths using only the owning host's Git config;
   missing repositories or unresolved identities block migration. Validate all
   hosts' plans together: duplicate claims with different panes, divergent pins
   collapsing onto one ID/seat/lane/tier, or incompatible bd location records
   block cutover, with no overwrite or automatic winner. After validation,
   atomically migrate UAC records (including tracker), lanes, uac-status,
   badges, bd projects/state/git and note `project` references. Preserve lane
   actor/earliest start, pin timestamp/expiry/clear tombstones, stop/resumed
   flags, note IDs/read watermarks and notification causes. Keep per-clone
   stores separate. Journal progress and a schema completion marker: restart
   resumes idempotently; an error holds project workers/publication stopped
   while the daemon reports it. Rollback restores files and owned pin payloads
   from the journal/backups while writers remain stopped.
3. **Republish and resume:** registry entries are ephemeral. Each owner deletes
   its old project keys at this cutover and republishes UACs, agents, notes and
   pins under v2; no legacy project keys remain active after deployment.
   Each host migrates, verifies its own v2 publication/direct gets, then resumes
   local workers. The UAC deploys all hosts one after another using the audited
   plans and verified exact grants; there is no runtime all-host barrier.
   During this interval unmigrated hosts are briefly invisible to v2 routing:
   routing to them fails without discarding durable data. External project
   writers remain stopped until the UAC verifies the whole deployment.
   A failed host stays quiesced;
   rollback restores the old registry projection from the journal before old
   binaries resume. Keep migration backups and historical bd backup directories.

Steady-state pins retain greatest `(at, host-token)` selection, including host
tie-breaking; the stricter divergent-pin check above applies to migration
collisions. Later explicit URL changes or adding the first remote require the
same preview, journal/backup and conflict-checked local rekey while affected
writers are stopped; never silently join identities or retain legacy reads.
The flag-day commands implement legacy-to-v2 projection only. Explicit later
v2-to-v2 rekeys with retained journal generations are tracked in vega-lhc.14.7;
keep affected writers stopped after a remote change and retain the original
cutover journal rather than resnapshotting it.

### Rekey after a remote change

Adding a project's first remote (or changing its selected remote URL) changes
its ID; every record keyed by the old ID then fails with
`project_identity_changed` and its worker skips it. vegad's git sync, bd sync
and decisions log this once per transition and record, not per pass; the line
names the registered and the current ID and the fix. `vega bd status` and
`vega uac status` show it on the project's row until it is fixed. Re-claiming
the UAC alone (vega-lhc.198) leaves the old-ID records behind, so the failure stays.

Until vega-lhc.14.7 ships a journaled v2-to-v2 rekey, the operator step on the
affected host, with `OLD` the encoded old ID (hex of the ID, the state files'
directory or file name):

1. `vega post relay stop`; stop the project's writers.
2. In the checkout: `vega bd init` (an existing database with the same remote is
   only re-registered under the new ID); in the UAC's pane `vega uac claim`.
3. Move the `OLD` records out of `$XDG_STATE_HOME/vega` (keep them as a
   backup): `uac/v2/OLD.json`, `bd/projects/v2/OLD/`, `bd/state/v2/OLD/`,
   `bd/git/v2/OLD/`.
4. `vega post relay start`; `vega bd status` lists the project once.

## Deliberately local

Keep herdr repo_key, cwd/workspace paths, worktree creation/trust/hook approvals,
Git operations and per-clone sync/cache/journal/lock paths local. bd actor
`<host>/<worktree-dir>` and pane lineage/done/parked/silent records retain their
contracts. Gate content keys, source paths, secrets and deployment checkout
paths do not become project IDs. Upgrade delivery still targets panes inside
the upgraded checkout; only finding its UAC uses ID. Display labels may remain
folder names. No filesystem path is a cross-host project identity except the
explicit host-scoped local forms above.

## Build sequence (child beads)

Approval is recorded in vega-lhc.14.2; the UAC closes that decision after this
revision lands to release the build chain below.

1. **vega-lhc.14.3:** shared resolver/types, normalization fixtures and local location catalog.
2. **vega-lhc.14.4:** registry/UAC/post/lanista v2 integration, cutover republishing and exact grants.
   Implementation handoff: [project-registry-cutover.md](project-registry-cutover.md).
   The UAC keeps .14.4/.14.5/.14.6 on the `identity` integration branch until
   the whole cutover is complete; these intermediate builds never land on main
   or run on unmigrated hosts.
3. **vega-lhc.14.5:** bd/git-sync per-location storage and bd shim guard/hints, PagerDuty,
   lanes/mux, board/notes/badges,
   upgrade/hygiene consumers and their migrations.
4. **vega-lhc.14.6:** coordinated startup migration, cutover audit/rollback, integration fixtures and guide updates:
   lanista-catalog.md, pagerduty.md, vegad.md, git-sync.md and beads-sync.md in
   docs/design, plus shims/bd/README.md and affected runtime guides.

Each step runs `mise run check` and the gate. Fixtures cover two hosts at
different paths, duplicate clones, forks, ambiguous remotes/UAC names, no remote,
all Git layouts, lossy-token collisions, exact grants, interrupted startup
migration/rollback, offline-host rejoin guards, divergent pins, expired/cleared
pins and stopped bd-sync. This revision changes design only; deployment and
implementation belong to the UAC and the build children.
