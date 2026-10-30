Flag-day tooling; stop vegad and all project writers before preview/rollback.
Mappings discovers local references and owned legacy registry UACs/pins,
resolves paths on this host, and writes preview's JSON array of
{old_key, project, location: {common_dir, checkout}}. Owners need the broker;
inbox-only hosts skip enumeration. Errors list unresolved keys with hints
(exit 1, export unchanged). Restore missing checkouts/project metadata or select
vega.projectRemote for ambiguous remotes; lossy filenames cannot recover paths.
Keep writers stopped between mappings and preview. Outputs stay outside live state.
Preview exports original bytes, owner pins and complete plans without changing
source records.
Audit requires --plan once per host and --host once per deployment host,
including offline and inbox-only hosts. --grants takes the orrery JSON export
of each inbox host effective publish/subscribe allowlists ({} without readers). It validates duplicate claims and pins.
Copy its bundle to $XDG_STATE_HOME/vega/identity-cutover/approved.json on each
host. Startup journals its plan before mutation, replays after interruption,
and writes complete.json only after v2 republish/direct-get verification.
Cutover errors keep vegad alive quiesced with workers/publication off; it
reports changed errors once and retries every five seconds (also --once).
The identity CLI subcommands fail fast. Owning-host startup and registry
reconciliation reject reappeared legacy keys even when they equal the original
backup; inbox-only hosts cannot enumerate and skip that check.
Inbox-only preview uses --registry-read once per exact authorized v2 key;
startup checks reads and forbidden enumeration after owners have started.
Only orrery changes grants, with the Slayer's approval. Keep backups.
Every subcommand loads vegad.env (VEGA_POST_HOST, broker) like `relay`.
A non-empty process VEGA_POST_HOST wins over the file, selecting that host's
relay lock and identity state; otherwise they match the pinned vegad host
(see docs/concepts.md). A late host rejoins with a re-audited bundle
(project-registry-cutover.md, Late host rejoin).

Examples:
  vega post identity mappings --output mappings.json
  vega post identity preview --mappings mappings.json --output host-plan.json
  vega post identity audit --plan a.json --plan b.json --host a --host b --grants grants.json --output approved.json
  vega post identity rollback

After deployment: vega post identity verify (owning host, all owners fresh v2).
After a late-host rejoin: vega post identity verify --host io (projection of io only).
After a remote change (new project ID): vega post identity rekey --help.
