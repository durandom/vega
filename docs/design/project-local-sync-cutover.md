# Project local sync cutover

The first part of vega-lhc.14.5 separates project identity from local bd and Git
sync state. The completed `identity` branch includes the .14.6 startup journal
and audit tooling. Deploy it only through the coordinated
[operator runbook](project-registry-cutover.md#operator-runbook-146).

A bd registration retains `key` as the full project ID and adds `common`, the
canonical absolute common directory on this host. Execution still uses `dir`,
`data`, `bd`, `database` and `remote`. These do not change during migration.
Linked worktrees share a common directory; independent clones do not share
state. Workers skip a location whose resolver reports an error or whose current
ID differs from its registered ID; changing a remote requires journaled rekey.

Files below `$XDG_STATE_HOME/vega/bd` are now
`<projects|state|git>/v2/<hex(ProjectId)>/<hex(common_dir)>.json`. Each contains
`schema: 2`, `project`, `common_dir` and `value`; readers validate selectors
against the envelope. There are no legacy reads or writes. New adoption backups
use `backup/v2/<hex(ProjectId)>/<hex(common_dir)>/<timestamp>/.beads`;
historical backup directories stay intact.

`vega bd status` and the UAC report group records by ID and show a separate
location column. `resume --project` accepts a unique ID or database name, or a
local repository path. Multiple locations matching an ID/database are an error;
use a checkout or common-directory path to select one. Re-registering a location
preserves its original provisioning and resume timestamps. The bd shim derives
both ID and location with the shared resolver for its Dolt guard and stderr
hints. Its actor remains the local host and worktree directory.

Git sync consumes the explicitly local UAC location projection
(`project_uac_locations` through `git::claimed`), verifies each location still
resolves to its recorded ID, and deduplicates by ID and common directory. It
never interprets an ID as a local path. Stop notifications carry project IDs;
UAC routing uses `project_uac_of`, while recovery instructions name the affected
local location. Existing notification causes are retained; a healthy clone
cannot clear a stopped sibling's shared legacy cause.

## Startup coordinator handoff (.14.6)

Status uses the full recorded clone inventory, including locations whose
resolver fails or whose identity changed. It projects the current failure code,
message and repair hint alongside any saved problem without writing state;
stopped clones retain their stopped label. Sync workers validate and filter
that inventory before touching Git or Dolt. Failed locations therefore remain
visible without changing stop/resume flags or timestamps.

`vega_bd_sync::cutover::prepare(snapshot, mappings)` is pure. Snapshot paths are
relative to the host's `vega/bd` directory. Each mapping supplies the exact old
key and a project ID/location resolved on this host. Include every old
`projects`, `state` and `git` JSON record; missing mappings, lossy source filename
collisions, conflicting identity for a location and incompatible destination
records reject preparation before any mutation. Database names, Dolt paths,
resume/stop flags, timestamps and Git state are preserved independently.

The coordinator must persist the original snapshot, mappings and returned
`Plan` in its single journal before writing any target or removing any source.
`Plan::writes` contains complete v2 bytes and `Plan::remove` the old source
paths. Replay that durable plan after interruption; never replace the original
backup with a partial live snapshot. The UAC audits all host plans before
deployment; each host resumes its workers after its own migration and
verification succeed. Rollback restores exact original bytes and removes newly
created v2 files from the journal while writers remain stopped. The helper does
not wire startup, apply files, access Dolt data, or
implement startup/rollback. Those are implemented in
`plugins/post/src/identity.rs` (.14.6). The UAC audits all host plans and deploys
one host after another; each host migrates/verifies before resuming local workers.
