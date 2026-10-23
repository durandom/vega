# vega janitor

Keeps a cached map of every git checkout on this machine (clones, worktrees, bare repos), plus the other hosts'
checkouts via vegad. It answers "where is repo X checked out?" before an agent clones it again, and lists dirty,
stale or duplicate checkouts as cleanup candidates. Read-only: it never fetches, changes or deletes anything.

Commands: `vega janitor`, `find`, `list`, `report`, `skills`, `search`, `embed`, `history`, `scan`.

Design: [janitor.md](../../docs/design/janitor.md).

Agent guide: `vega skills janitor`

`embed` requires `VEGA_JANITOR_EMBED_ENDPOINT` or the first line of
`~/.config/vega/janitor-embed.url` (under `$XDG_CONFIG_HOME` when set), plus the
`triton-embedding` secret. Missing endpoint configuration returns
`endpoint_unconfigured` before credential access.
