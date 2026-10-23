---
name: vega-bd
description: Provisions a project's beads (bd) database on the configured Dolt remote with one command and shows its sync state; vegad pulls and pushes automatically. Use when setting up beads in a repo or on another machine, when bd data seems stale across machines, or when a beads sync stopped on a conflict.
---

# vega bd: beads synced across machines

Each clone has a local Dolt server. vegad pulls every 60 s and pushes after writes
to the configured remote. Never run `bd dolt pull` or `bd dolt push`.

```
vega bd init                     # provision this project (once per clone)
vega bd init --database VEGA --prefix vega
vega bd init --adopt             # move this project's existing local beads to the remote
vega bd status                   # projects: ahead, behind, last sync, state
vega bd resume                   # after resolving a conflict: vegad syncs again
```

- `init` clones fresh from the remote. Never copy a `.beads` dir between machines: its server
  config and lock files travel along; `status` flags them.
- `--database`: ASCII alphanumerics, underscores, hyphens; preserved locally/remotely.
- A database missing on the remote is created first (SQL `CREATE DATABASE` as a CREATE-only user,
  password from the vega secret `titan-dolt`). `credential_missing`: the secret is not on this
  host yet; report it to the Slayer, do not work around it.
- A project that already has beads with another or no remote is not touched
  (`already_initialized`); `vega bd init --adopt` moves it: pushes its history to the remote
  database (one force push only onto an empty one; `remote_diverged` on foreign history), backs
  up `.beads` (path printed), clones it back in server mode, registers it. The UAC runs it, with
  no agent writing beads meanwhile; other machines then run plain `vega bd init`. The output
  ends with the rollback line.
- `state` `stopped`: a merge conflict or a rejected push. Its UAC gets mail. The fix: in
  `.beads/dolt/<DB>`, `dolt pull origin main`, `dolt conflicts resolve --ours|--theirs issues`,
  `dolt commit -m ...`, then `vega bd resume` (in a checkout, or `--project` with the key or the
  database `vega bd status` lists). Multiple clones require `--project <local path>`; other clones stay stopped.
- `state` `error`: the remote was unreachable or a pull failed without a conflict; vegad retries
  and fast-forwards by itself, never a stop.

## Git sync

vegad fetches repos and UAC claims every 5 min, fast-forwarding clean default-branch
checkouts, never worktrees. `status` shows problems and `worktrees_behind`; `bd`
warns on stderr when behind.

Migration/recovery: `vega post identity --help`.

Endpoints: environment overrides first-line files under `$XDG_CONFIG_HOME/vega`
(normally `~/.config/vega`). `init` needs `VEGA_BD_REMOTE` / `bd.remote`
(`remote_unconfigured` if absent). Creation also needs `VEGA_BD_SQL` / `bd.sql`
(`host:port`; `sql_unconfigured` before credentials or SQL). No estate defaults.
