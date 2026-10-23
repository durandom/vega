# Beads sync: one command per clone, vegad keeps it synced

Status: built, 2026-10-11. Beads: vega-lhc.75, vega-lhc.76 (creating databases), vega-lhc.77 (adopting). Part of [vegad](vegad.md). Code: `crates/vega-bd-sync`
(project store, bd runner, sync cycle, doctor), `plugins/bd` (`vega bd`), `plugins/post/src/bdsync.rs`
(vegad's thread), `shims/bd` (per-worktree actor). Reference setup: spellkave `docs/beads.md`.

## Problem

Beads must sync across machines (proteus, ganymede, Io) without a single-writer rule. spellkave
does it by hand: per-clone server mode, a Dolt remote on titan, `bd dolt pull` at session start and
`bd dolt push` after every write, written into PRIME.md. Agents forget the push and strand work on
one machine; each new project repeats the bootstrap by hand.

## Topology

```
 proteus                      ganymede                     titan (orrery owns the container)
 ┌────────────────────┐       ┌────────────────────┐       ┌──────────────────────────────┐
 │ bd ─► local dolt   │       │ bd ─► local dolt   │       │ dolt sql-server 2.4.0        │
 │       sql-server   │       │       sql-server   │       │  remotesapi :8080 (tailnet)  │
 │ vegad: pull 60 s,  │◄─────►│ vegad: pull 60 s,  │◄─────►│  one database per project    │
 │  push after writes │       │  push after writes │       │  SQL :3306 (CREATE-only user)│
 └────────────────────┘       └────────────────────┘       └──────────────────────────────┘
```

Per clone (all worktrees of a clone share it): `bd init --prefix <p> --database <DB> --remote
<remotesapi>/<DB> --server --stealth --skip-agents --skip-hooks --non-interactive`. bd puts `.beads`
next to the git common dir (the main worktree of a `.git` clone, `.bare/.beads` in a bare layout);
worktrees find it through git. Stealth: nothing enters the repo's git history.

### Why not shared-server mode

Every `bd` call talking to one central sql-server is true multi-writer without merges, but
spellkave tried it on 2026-08-10 and reverted the same day: 2-6 s per bd command over the tailnet
against sub-100 ms locally. It also needs the remote for every read. Local server plus replication
keeps bd fast and offline-capable; the price is merges, which bd resolves field by field and which
stop on a true conflict (below).

## Provisioning: `vega bd init`

1. Defaults from the git common dir: database `REPO` (upper case, `_` for other characters),
   prefix `repo`. `--database` and `--prefix` override; the remote base is `VEGA_BD_REMOTE`, else
   `~/.config/vega/bd.remote` (required; for example `http://dolt.<tailnet>:8080`).
2. Already provisioned with the same remote: register again (idempotent). Beads with another or no
   remote: `already_initialized`; `--adopt` moves it (below).
3. Probe the remote without downloading: `dolt fetch` of a branch that does not exist into a
   throwaway repo. An existing database answers `invalid ref spec`, a missing one
   `database not found`. A missing one is created first (below).
4. `bd init` as above. It always clones fresh: never copy a `.beads` dir between machines (its
   server config and lock files travel along, rally-ng 2026-10-11); `vega bd status` flags a
   foreign `dolt-server-config.yaml` and a stale `dolt-server.pid`.
5. Seed: if the clone is ahead of the remote (an empty database: bd just wrote the schema), push
   at once.
6. Register `$XDG_STATE_HOME/vega/bd/projects/v2/<hex(ProjectId)>/<hex(common_dir)>.json`
   with the bd binary: vegad's PATH has no bd. Identity is the normalized fetch
   remote; execution and independent clone state use the local common directory.

### Creating a missing database

remotesapi cannot create a database (verified against a local dolt 2.4.0 sql-server with
remotesapi, titan's version): a push to a missing one fails `database not found`, with or without
`--user root`, and remotesapi has no other create call. So `vega bd init` creates it over SQL:

- **Endpoint and user**: `VEGA_BD_SQL` or `~/.config/vega/bd.sql` (`host:port`, required for creation;
  for example `dolt.<tailnet>:3306`),
  user `vega` with `GRANT CREATE ON *.*` only (orrery orr-rcg; the tailnet grant for 3306 covers
  proteus and ganymede). No read, write or drop. MySQL has no create-database-only privilege:
  global CREATE also allows `CREATE TABLE` in existing databases; vega never sends that.
- **Statement**: ``CREATE DATABASE IF NOT EXISTS `<DB>` `` and nothing else. The name is checked
  first (ASCII letters, digits, underscores and hyphens; `bd::valid_database`).
  Hyphens are legal in Dolt names but require SQL identifier quoting. Explicit names stay identical
  in the remote URL and local database path; no underscore mapping is applied. Other characters
  stay outside vega's supported URL/path-safe subset. `IF NOT EXISTS` leaves a database
  another clone created a moment earlier untouched; the seed race below handles the rest.
- **Credential**: the vega secret `titan-dolt` (env `VEGA_TITAN_DOLT_PASSWORD`, for `vega-bd`,
  hosts proteus and ganymede; [secrets.md](secrets.md)), read in process with
  `Store::get("titan-dolt", "vega-bd")`. No `vega secrets run` wrapper: the plugin needs the
  value for one call only, and the in-process lookup is the path secrets.md gives vega plugins.
  Not there yet: init stops with `credential_missing`, naming the `vega secrets set titan-dolt
  ...` line for the Slayer. Other secrets errors pass their message and fix on as
  `credential_unavailable`.
- **Client: `dolt sql --host`**, not a Rust MySQL crate. dolt is already required (the probe uses
  it), so this adds no dependency (a MySQL crate brings a large tree, TLS stacks included) and
  speaks the server's own dialect. The password goes through `DOLT_CLI_PASSWORD`, never argv;
  dolt's errors do not echo it (`Access denied for user 'vega'`). Without that variable dolt
  prompts on the TTY, so it is always set. `--no-tls`: titan's sql-server has no TLS (`TLS
  requested but server does not support TLS`); the tailnet encrypts.
- **Errors**: `create_denied` (wrong password or grant), `create_unreachable` (no tailnet grant
  for 3306, server down), `create_failed` (anything else; nothing changed).
- After `CREATE DATABASE`, `bd init --remote` bootstraps from the empty database (it clones the
  initial commit), so the seed push is a fast-forward. spellkave needed one `--force` push only
  because its local database existed before the remote one.
- remotesapi has no authentication: a push to an existing database works anonymously; the tailnet
  grant (`tag:dolt-client` → `tag:dolt:8080`) is the only access control.
- A remote with named users (e.g. a DoltHub-style hub): set `DOLT_REMOTE_USER` and
  `DOLT_REMOTE_PASSWORD`. dolt reads only the password from the environment and defaults the
  user to `root`, so the probe fetch and the `--adopt` fetch and push add `--user
  $DOLT_REMOTE_USER` (`bd::remote_args`); `bd dolt pull`/`push` read both variables themselves.
  The password never goes into argv.

Tested: `bd::tests::creates_a_missing_database_with_create_only` runs a local sql-server with
remotesapi and a CREATE-only `vega` user (probe `Missing`, wrong password `Denied`, create, probe
`Exists`, create again `Ok`). A full `vega bd init` against such a server, with a throwaway
secrets store (`XDG_CONFIG_HOME`), created the database, seeded it and reran idempotently.

### The seed race

Two clones that run `bd init` against an empty database before either pushed each write their own
schema and project id. The second one's pull then merges the first one's identity into its
database, and bd refuses to open it (`PROJECT IDENTITY MISMATCH`; reproduced). Hence step 5: the
first `vega bd init` seeds the remote, later ones clone the seeded schema. If a push is rejected
there, init fails with `seed_race`; the clone holds no beads yet, so removing its `.beads` and
rerunning is safe. vegad stops a project whose count fails on an identity mismatch.

### Adopting an existing database: `vega bd init --adopt`

For a project whose beads predate the remote: embedded (vega's own) or server mode, with no
remote or another one (vega-lhc.77, `crates/vega-bd-sync/src/adopt.rs`). One run on the machine
that holds the beads; every other machine then runs plain `vega bd init` and clones.

1. Only a local Dolt repo under `.beads` qualifies (`metadata.json`: `dolt_mode` embedded or
   server, no external `dolt_server_host`); else `adopt_unsupported`. The issue prefix stays the
   database's own (`--prefix` that differs: `usage`).
2. The remote database is probed and created if missing, as in step 3 above.
3. Server mode: `bd dolt stop`, so the dolt CLI owns the files.
4. **Push** (`adopt::push`): a temporary remote `vega-adopt` in the local repo, `dolt fetch`, then
   by the heads:
   - remote head in the local history (or no `main`): plain push;
   - local head in the remote history (an earlier run pushed it): nothing to push;
   - the remote holds only Dolt's initial commit (just created by `CREATE DATABASE`, or never
     pushed to): **one force push**. Unavoidable: the initial commit of a database created on
     the server has no common ancestor with a local history (`no common ancestor`); this is the
     case spellkave hit;
   - anything else: `remote_diverged`, nothing pushed. vega never forces onto foreign history.

   The temporary remote is removed whatever happens. Nothing local changed so far.
5. **Backup**: `cp -pR .beads $XDG_STATE_HOME/vega/bd/backup/<key>-<unix time>/.beads` (outside
   the repo, so `git status` stays clean). Then the backup's and the live repo's `main` must both
   equal the pushed head; a bd write in between fails with `beads_changed` (backup removed,
   nothing changed; rerun). Run it when no agent writes beads.
6. **Clear**: remove the data dir (`embeddeddolt/` or `dolt/`) and the database files
   (`metadata.json`, gate locks, `dolt-server*`) from `.beads`. `config.yaml`, `PRIME.md`,
   `README.md`, `.gitignore` and `backup/` stay.
7. **Clone back**: the same `bd init --server --remote ...` as a fresh clone, so the adopting
   machine ends up exactly like the others: data in `.beads/dolt/<DB>` under the remote's name,
   `origin` the remote (another previous remote is dropped; it is in the backup). bd keeps the
   `project_id` (it is in the cloned database) and rewrites `config.yaml` (a note says so when the
   old one had settings). Then seed (bd init's own commit, a fast-forward) and register.

Why clone back instead of converting in place: bd 1.3.1 has no embedded-to-server migration, and
the clone is the path every other machine takes; an in-place conversion would be a second layout
to keep right. The cost is one round trip of the database over the tailnet.

**Rollback** (the output's last line prints it with the real paths): the history stays on the
remote either way.

```
bd dolt stop                                  # the new server
mv <repo>/.beads <backup>/beads.adopted        # kept for inspection, out of the repo
cp -pR <backup>/.beads <repo>/.beads           # the old database, config and PRIME.md
rm $XDG_STATE_HOME/vega/bd/projects/<key>.json # vegad stops syncing it
```

`<backup>` is `$XDG_STATE_HOME/vega/bd/backup/<key>-<unix time>`. Verified on a throwaway repo:
the restored server-mode database answered `bd ready` as before, with its old remote.

The remote database stays (the `vega` SQL user cannot drop); the Slayer drops it on titan if
wanted. A failure after step 6 names both ways out: plain `vega bd init` (clones the pushed
history) or the rollback.

Tested on throwaway repos against a local sql-server with remotesapi: embedded source onto a
just-created database (forced once, a second clone sees the same `bd ready` and project id, a
write syncs both ways), server-mode source with another remote onto a database holding part of
its history (fast-forward), foreign history (`remote_diverged`, local untouched, no backup),
missing database without the secret (`credential_missing`), rerun (only registers).
`adopt::tests::pushes_into_a_remote_database` covers the push plans against such a server.

## Sync rules (vegad)

A thread in vegad (`bdsync.rs`, like the CI watch) runs `vega_bd_sync::sync::cycle` every 5 s:

- **Pull** every 60 s (`bd dolt pull`), then count ahead/behind with
  `bd sql` (`dolt_log('remotes/origin/main..main')` and the reverse) and **push** if ahead.
- **Local writes**: bd auto-commits every write; Dolt's chunk journal (`noms/vvvv…`) changes on
  each commit, the manifest does not. vegad stats the newest mtime in
  `.beads/dolt/<DB>/.dolt/noms`; a change triggers a count, and when ahead a pull then a push, so
  a write reaches the remote within about 5 s. The mtime is recorded before vegad's own pull or
  push, so a write in between is never missed; vegad's own journal writes cost one extra count.
- **Never force.** A push rejected as non-fast-forward (another machine pushed between our pull
  and push) retries with a pull on the next tick.
- **Unreachable remote** (gRPC `Unavailable`, `DeadlineExceeded`, a timeout): an `error`, retried
  on the next interval; work continues locally.
- **Transient pull failure**, no conflict: `cannot merge with uncommitted changes` (an agent's write
  landed between bd's `auto-commit before pull` and its merge), a remote's `http status: 400`,
  `invalid connection` (bd restarted its local server on a new port) or `pull merged nothing but the
  local branch is behind a fast-forwardable remote tip` (a peer pushed after bd's fetch). vegad
  pulls again at once, the last one by a fast-forward through bd's server
  (`bd sql "CALL DOLT_PULL('origin', 'main', '--ff-only')"`, the manual `dolt pull origin main`
  without a server restart); if that fails too, an `error` retried on the next interval, never a
  stop. A stop an older vegad recorded for one of these lifts itself (vega-lhc.111, vega-lhc.130).
- bd runs as `BEADS_ACTOR=<host>/vegad`, so merge commits name vegad, and with
  `BEADS_FSCK_TIMEOUT=5m` unless vegad's env sets it: bd's default pre-push fsck limit stopped
  spellkave's 894 MB store (`fsck did not complete within the configured timeout`). pull and push
  get 7 minutes.

## Conflicts

`bd dolt pull` resolves most merges itself (field-level three-way merge of issues, union of
labels, comments and events). A true conflict (the same field changed on two machines) makes it
exit 1: `merge conflicts in issues require operator resolution; merge aborted and working set
restored`. Then, and on a push failure other than the two above:

1. vegad marks the project `stopped` and syncs nothing for it.
2. Attention: a herdr notification (the board's list) and, if the project's UAC is registered on
   this host, mail from `tool vega-bd` with the fix.
3. `vega bd status` and `vega uac status` show the state and the reason.
4. The UAC resolves, verified on a throwaway project:
   ```
   cd .beads/dolt/<DB>
   dolt pull origin main
   dolt conflicts cat issues                     # what differs
   dolt conflicts resolve --ours|--theirs issues
   dolt commit -m "resolve: ..."
   vega bd resume                                 # vegad pushes within seconds
   ```
   A resume without a resolution conflicts again and stops again.

   `vega bd resume` without `--project` resumes only the local clone containing
   the current checkout. `--project` accepts a unique project ID or database
   (`VEGA`), or a local checkout/common-directory path. Exact ID/database matches
   take precedence over relative paths. If several clones share that ID or
   database, use the `location` column from status to select one; the others keep
   their stopped state. `~` expansion, trailing slashes, linked checkouts and the
   parent of a `.bare` repository work for local paths.

## Status

`vega bd status` (and `vega bd`): grouped by project ID, one row per local clone:
`project,location,database,ahead,behind,last_sync,state`
(`ok`, `pending` before the first sync, `error`, `stopped`), the problems, and doctor findings.
`vega uac status` prints the same table as `beads_sync`. Counts are vegad's last ones. Both also
show vegad's git sync of the same projects ([git-sync.md](git-sync.md)).

## Claim identity (bd shim)

`shims/bd` replaces the dotfiles' `~/.local/libexec/bd-shim/bd` (spellkave sk-jay0.7.24): it sets
`BEADS_ACTOR=<host>/<worktree dir>` from the directory bd runs in, then execs the real bd, so
`--claim` (a compare-and-swap on the assignee) distinguishes worktrees and machines. Same escape
hatches: a caller-set `BEADS_ACTOR` wins, `--actor` beats both, `BD_REAL` names the binary.
An absent/nonexecutable override, a link to this shim, or a candidate in the shim directory is
skipped in favor of PATH discovery. Discovery checks inode identity as well as the directory,
and child PATH excludes the shim's directory, aliases and links back to this executable before
any helpers or passthrough run. An executable override that is a copied shim or recursive wrapper
can still be selected; `VEGA_BD_SHIM_ACTIVE` then stops re-entry with exit 126 and a stderr hint to
set `BD_REAL` to the original binary on the next call. If the shim cannot identify its executable,
it exits 126 before consulting `BD_REAL` or PATH; invoke the original bd directly by absolute path.
These are documented P2 deviations. Known gap: `bd -C <dir>` derives from the current dir.

Internal hostname/git metadata probes have a two-second deadline; timeout kills their process
group and reaps the direct child. A failed hostname probe uses `unknown`; a missing git toplevel
leaves just the host actor. Missing git metadata suppresses hints and may prevent project detection
for the manual-sync guard below. The user's actual bd command is exec'd without a shim timeout,
preserving its stdin, stdout and exit status once the shim's guards pass.

Difference: vega's shims are agent-only (P3), so a human's shell no longer derives an actor; bd
falls back to `git config user.name` there. Agents do the claiming, so that is acceptable; a human
who claims sets `BEADS_ACTOR` or `--actor`. The dotfiles shim stays until the UAC removes it.

## Manual push and pull (bd shim)

Agents kept running `bd dolt push`/`pull` by hand in synced projects (habit, old PRIME.md files,
skills): they raced vegad, hit the fsck timeout and were rejected as behind (vega-lhc.123). So the
shim, a documented deviation from P2 like the gh shim's `gh run watch` denial, checks
`bd [global flags] dolt push|pull` (not `--help`, `--global`, `--db`, `--database`): when the
project of the current dir (or `-C <dir>`) is provisioned (`projects/<key>.json`, key as above):

- sync not stopped: exit 0 without running bd, one stderr line (vegad pushes within ~5 s, pulls
  every 60 s; `vega bd status`).
- sync stopped: exit 1 with the stop reason; the UAC resolves it (Conflicts above).

`VEGA_BD_MANUAL=1`, unprovisioned projects and every other bd command pass through with the
PATH cleanup and recursion/identification guards described above. If the project metadata probe
fails or times out, project detection returns no project and the manual-sync guard passes through.
vegad and `vega bd init` run the real bd (`Project.bd`), never the shim; the UAC's conflict
resolution uses `dolt` directly.

## Not done here

- Running `vega bd init --adopt` on vega's own `.beads` (embedded, no remote) and rally-ng's,
  and updating vega's `.beads/PRIME.md` storage section: the UAC does that after landing.
- Windows (Io): vegad and the shim are Unix-only today.


Project records use normalized fetch-remote IDs; filesystem execution retains
owner-local locations. The startup journal applies plans before workers start,
preserving per-clone stopped state, timestamps and notification causes.
Interrupted startup replays the saved plan; rollback restores original bytes
while writers stay stopped. Historical bd backups remain untouched. The UAC
coordinates preview, audit, deploy, verify and rollback: see
[the operator runbook](project-registry-cutover.md#operator-runbook-146).
