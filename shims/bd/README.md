# bd shim

Sets `BEADS_ACTOR=<host>/<worktree dir>` from the directory bd runs in, then runs the real `bd`.

bd's claim is a compare-and-swap on `assignee`, and bd's default actor (`git config user.name`) is the same in every
worktree on every machine, so two agents could both win a claim. A per-worktree actor makes the second claim fail.

- A caller-set `BEADS_ACTOR` wins; an explicit `--actor` beats both (bd's own precedence).
- `BD_REAL` names the real binary. Discovery skips vega shim builds and links or aliases to the running shim.
- The shim excludes shim directories from PATH before running helpers or the real bd.
  A recursive wrapper exits 126 with a hint to set `BD_REAL` to the original binary.
  If the shim cannot identify its executable, it also exits 126 before consulting `BD_REAL` or PATH.
  Invoke the original bd directly by absolute path.
- Internal hostname/git metadata probes have a two-second deadline; timed-out process groups are killed
  and direct children reaped. A missing git toplevel leaves just the host actor; a failed hostname probe uses
  `unknown` as the host. Unavailable git metadata suppresses hints and can bypass project sync detection.
  The user's bd call keeps its own runtime, stdout and exit status once the shim's guards pass.
- Outside a git checkout the actor is just `<host>`.
- `bd -C <dir>` derives the actor from the selected worktree. A group member without its own workspace uses the coordination clone registered on this host.

In a project vegad syncs (`vega bd init`), `bd dolt push` and `bd dolt pull` race vegad, so the shim skips them:

- Sync ok: exit 0, one stderr line (vegad pushes within ~5 s of a write, pulls every 60 s; `vega bd status`).
- Sync stopped: exit 1 with the reason; the UAC resolves it.
- A provisioned common directory now resolving to a different project ID: exit 1.
  Quiesce its writers and perform the journaled rekey; do not run Dolt push/pull.
  Resolver failures and unregistered/non-git directories still run the real bd.
- `VEGA_BD_MANUAL=1`, unprovisioned projects and every other bd command pass through, subject to the
  recursion and executable-identification guards above. A project metadata timeout also passes through.

Before bd runs, one stderr line each when the project's beads sync is stopped, when this main checkout is behind
with vegad's fast-forward skipped, or when this worktree is behind the upstream ([git-sync.md](../../docs/design/git-sync.md)).

Design: [beads-sync.md](../../docs/design/beads-sync.md) (Claim identity; Manual push and pull).

Agent guide: `vega skills bd`

Project registrations and sync hints use schema-2 ID/location records. Vegad
journals/applies the migration before workers start; stopped clones remain
stopped and historical bd backups stay intact. Recovery and rollback:
[operator runbook](../../docs/design/project-registry-cutover.md#operator-runbook-146).

Regression tests use an embedded-mode metadata fixture, PATH aliases, a recursive wrapper, and a hard link.
For a real embedded Dolt smoke test, run
`BD_EMBEDDED_TEST_BIN=/absolute/path/to/original/bd RUST_TEST_THREADS=1 nice cargo test -p bd-shim`.
The real-bd test is skipped unless this variable is set; bd is not a build prerequisite.
