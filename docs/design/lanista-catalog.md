# lanista catalog: built on ganymede, distributed by vegad

## Project model pins (vega-lhc.156)

`vega lanista pin <lane> --tier <tier>` resolves a runtime project override,
then the embedded global seats (`plugins/lanista/seats.json`). `source` names
`project (<writer host>)` or `global seats`; an active override also prints
`expires` as Unix seconds or `never`. Project overrides change no repo file.

```
vega lanista pin codex --tier budget --set gpt-6.1-sol
vega lanista pin zai --tier budget --set glm-5.2 --until 2h
vega lanista pin codex --tier budget
vega lanista pin codex --tier budget --clear
```

Identity is `vega_herdr::project::resolve`: the normalized Git fetch remote
`ProjectId`, as for `vega uac`. Worktrees, duplicate clones and hosts at
different checkout paths share an ID when their selected fetch remotes match.
Local/file identities remain host-scoped. See the flag-day
[registry cutover handoff](project-registry-cutover.md); deploy all hosts only
after the startup migration is ready.
Outside a git project, reads use global seats and mutations fail.

### Registry and merge rule

Normal hosts may publish only `$KV.VEGA_REGISTRY.<host>.>` (auth-config in
`plugins/post/src/auth.rs`), so a top-level `project.*` key is forbidden.
Pins use `<host>.lanista.v2.<hex(id)>.<hex(seat)>.<hex(lane)>.<hex(tier)>`.
Components encode UTF-8 bytes in lowercase hex, avoiding punctuation collisions.
Values contain `schema: 2`, the full ID in `project`, `seat`, `lane`, `tier`, `model` (null for clear),
`at` (Unix nanoseconds), and `expires` (Unix seconds or null).

For each (project, seat, lane, tier), greatest `(at, host-token)` wins across
hosts, including clears and expired records. Mutations timestamp after the
greatest timestamp they read and at least the writer's current time. Concurrent
writes use wall clocks; lexicographically greater host tokens break exact ties.
Clocks are assumed synchronized within seconds. No heartbeat is required:
pins survive an offline writer. Resolve selects the newest record first and
ignores its model at `now >= expires`, preventing older pins from returning.

On resolve the writer best-effort prunes its expired model payload with a
revision-checked update, retaining timestamp and expiry as a tombstone. Other
hosts cannot prune it under their grants and ignore it immediately. Tombstones
remain to suppress older overrides. Vegad reconciliation preserves the
`lanista` namespace, which the CLI manages independently.

### Expiry and availability

Default expiry is capacity's binding account window reset (least remaining
headroom); lane `agy` maps to `antigravity`. Model-scoped windows do not bind.
A missing or passed reset, or a sample older than 30 minutes, requires explicit
`--until`. Positive durations accept `s`, `m`, `h`, `d`; `never` keeps a pin
until the next set/clear. Clearing needs no quota sample.

Normal hosts enumerate keys and direct-get pin values within five seconds.
A failed read visibly falls back to global seats (`registry: unavailable: ...;
using global seats`); mutations fail when the registry cannot be read/written.
No local cache claims to be the cross-host truth.

Inbox-only hosts such as deimos cannot enumerate or write and normally use
global seats. If already granted exact keys via
`auth-config --registry-read <reader>=<key>`, set `VEGA_LANISTA_PIN_HOSTS` to
the comma-separated writer hosts to read. This uses only exact direct gets,
without stream info. List every writer whose overrides must be considered;
a denied key makes the whole read fall back rather than partially merge.
This feature changes no broker grants; provisioning read grants is separate.

### Consumers

Gate's unpinned candidates call `vega lanista pin` from the author's project
before spawning a review clone. `.vega/gate.toml` retains the tier intent;
explicit `<kind>:<lane>/<model>` candidates intentionally bypass tier lookup.
Mux spawn has no automatic lanista selection: callers choosing a tier resolve
inside the target project's checkout and pass the returned model as harness
arguments. For a remote spawn, resolve on the target host and project so its
registry visibility is respected.

Status: built, 2026-10-19. Bead: vega-lhc.136 (Slayer decisions vega-nic.12: ganymede builds; transport A:
the broker's object store, 2026-10-19). Part of [vegad](vegad.md). Code: `crates/vega-herdr/src/catalog.rs`
(names, state file, rules), `plugins/post/src/catalog.rs` (vegad's thread, put, direct-get reader),
`plugins/post/src/auth.rs` (grants), `plugins/lanista` (display).

## Problem

`lanista fetch` ran by hand on proteus only; the other hosts' catalogs went stale unnoticed (proteus: 8 days
old on 2026-10-19). Every host fetching would need every source key on every host.

## Host independence (checked 2026-10-19)

`~/.cache/lanista/model_index.json` (~23 MB, 4049 models) is the only file lanista's queries read from its
cache; `sources/*.json` are fetch inputs, `opinions/` belongs to `lanista refresh-opinions` (not synced).
The index holds no host-specific data except `aliases_path`, an absolute path lanista writes as metadata and
never reads back. Fetch-time inputs from `~/.config/lanista`: `aliases.json` and `sources/gkisokay.json`, both
seeded from lanista's package data (identical to the seeds on proteus). The age comes from `generated_at`
inside the file, so a copy keeps its age.

## Schedule

vegad on the builder host runs `lanista fetch` once a day (24 h after a fetch that worked, 1 h after one
that failed, killed after 15 min) with the secrets `vega secrets run --for lanista` would give it (same
crate, no subprocess). A secret the host cannot decrypt is skipped and named in the log; lanista then skips
that source. The builder is `VEGA_LANISTA_BUILDER`, else `~/.config/vega/lanista-builder`, else `ganymede`.
A clock in the state file, not cron or launchd: vega installs no OS schedulers (vegad.md, Lifecycle).

## Transport

The broker's JetStream object store, bucket `VEGA_LANISTA` (stream `OBJ_VEGA_LANISTA`, 128 MiB, S2
compression), one object `model_index.json` with metadata `generated_at` and `source`.

- **Builder:** after a fetch, or when the bucket lacks the local index, a standard object store `put`
  (replaces the object, purges the old chunks). A failed put retries every 15 min.
- **Every other host:** every 15 min one direct get of the object's info. When its `generated_at` is newer
  than the local index's, the chunks by `next_by_subj` direct gets, checked against size and SHA-256, then
  written atomically into lanista's cache. A newer local index (a hand-run `lanista fetch`) stays.
- **No consumers.** The standard object store `get` creates an ordered push consumer. A consumer create
  right lets a host set any `deliver_subject`, and the server publishes past permissions, e.g. into another
  host's inbox (vega-mux-broker-spike.md). Readers therefore get only `$JS.API.DIRECT.GET.OBJ_VEGA_LANISTA`
  and `...OBJ_VEGA_LANISTA.>`, which reach nothing outside the bucket. This includes the inbox-only host
  deimos: the catalog is public data flowing toward it, and a direct get lets it send nothing.
- **Writer grants** (`auth-config --lanista-writer ganymede`): stream info, message get and purge of
  `OBJ_VEGA_LANISTA`, publish `$O.VEGA_LANISTA.>`. An inbox-only host cannot be the writer.
- **Admin:** stream info always; create only with `auth-config --create lanista` while provisioning
  (`vega post provision lanista`), as for the registry bucket.

`plugins/post/src/catalog_broker_test.rs` runs this against a local nats-server with the rendered grants.

### Rejected

- **SSH push from ganymede** (the `vega deploy` path): ganymede reaches none of proteus, deimos (no tailnet
  DNS) and io-wsl over SSH today, and its herdr machine list has only io.
- **SSH pull by each host:** gives corp-managed deimos an SSH login on a personal host.
- **Relay through proteus** (pull from ganymede, push with the deploy targets): no broker change, but the
  copies wait for proteus, which is not always on; that defeats building on ganymede.

## Freshness display

vegad writes `~/.local/state/vega/lanista.json` (source, generated_at, received/checked/fetched/published
times and results). `vega lanista` shows, per host:

```
catalog: 4049 models, 0 days old, from ganymede
catalog_sync: from ganymede: checked 4m ago (ok), received 9h ago
```

On the builder: `catalog_sync: builder: fetched 9h ago (ok), published 9h ago (ok)`. The source is the
state's host while the local index is the one vegad wrote, else this host. Older than 2 days adds
`(stale: see catalog_sync)`.

## Rollout (the Slayer)

1. ganymede gets the Artificial Analysis key: add it to the `artificial-analysis` secret's hosts on the admin
   host (`vega secrets`); without it the fetch skips that source.
2. Broker: regenerate the auth config with `--create lanista --lanista-writer ganymede` (plus the usual
   flags), reload, `vega post provision lanista` with the admin seed, then regenerate without
   `--create lanista` (keep `--lanista-writer ganymede`) and reload.
3. Install on every host (`vega deploy`); vegad upgrades itself in place.


Project records use normalized fetch-remote IDs; filesystem execution retains
owner-local locations. The startup journal applies plans before workers start,
preserving per-clone stopped state, timestamps and notification causes.
Interrupted startup replays the saved plan; rollback restores original bytes
while writers stay stopped. Historical bd backups remain untouched. The UAC
coordinates preview, audit, deploy, verify and rollback: see
[the operator runbook](project-registry-cutover.md#operator-runbook-146).
