# vega bd

Keeps a project's beads (bd) database in sync across machines. Each clone runs its own local Dolt server and
replicates to one database per project on the configured Dolt remote; vegad pulls and pushes, so agents never run
`bd dolt pull` or `push` themselves. vegad also fetches the repos and fast-forwards clean main checkouts.

Commands: `vega bd init`, `vega bd status`, `vega bd resume`.

`vega bd init --database oparl-lite-2` accepts ASCII letters, digits, underscores and hyphens.
The explicit name is kept on the remote and locally, with backticks in SQL. The default still
uppercases the repo name and replaces other characters (including hyphens) with underscores.

Design: [beads-sync.md](../../docs/design/beads-sync.md), [git-sync.md](../../docs/design/git-sync.md).

Agent guide: `vega skills bd`

`init` requires `VEGA_BD_REMOTE` or the first line of
`$XDG_CONFIG_HOME/vega/bd.remote` (normally `~/.config/vega/bd.remote`);
missing configuration returns `remote_unconfigured`. Creating a missing database
also requires `VEGA_BD_SQL` or `vega/bd.sql` under that config directory
(`host:port`); missing configuration returns `sql_unconfigured` before SQL or
credential access. Environment overrides win; there is no automatic estate remote.
## Overview JSON (schema 1)

`vega bd status --json [--project <project-id>]` (or `vega bd --json`) emits
`{schema: 1, projects: [...]}` without invoking bd, doctor, or git sync.
It lists provisioned locations on this host; exact project filtering retains
multiple clones. Rows have `project`, `common_dir`, `checkout`, `database`,
`state` (`pending`, `ok`, `error`, `stopped`), `last_pull` and `last_push`
(Unix seconds, 0 means never), `conflict` (active stop reason|null), and `sync`
(recorded `head`, `ahead`, `behind`, `pull_tried`, `pulled`, `pushed`, `synced`,
`seen_write`, `error`, `stopped`, `stop_reason`). A stop may be a rejected push:
show its reason rather than assume a merge conflict.

JSON is supported for status only. Errors use `{error: {code, message, hint}}`
(exit 2 usage, 1 runtime). Empty lists are arrays; TOON remains the default.
Consumers should ignore future additive fields.
