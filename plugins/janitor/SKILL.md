---
name: vega-janitor
description: Keeps a cached map of every git checkout on this machine (clones, worktrees, bare repos under ~/src) plus the other hosts' checkouts, and resolves a repo name to its path, lists dirty, stale or duplicate checkouts and reports cleanup candidates. Use when looking for where a repo is checked out, before cloning a repo that may already exist, or when asked which checkouts can be cleaned up.
---

# vega janitor: where every checkout lives

Read-only: it never fetches, changes or deletes anything; `report` names the command a human would run.

```
vega janitor                  # counts and what needs attention
vega janitor find lanista     # repo name, owner/repo or a fuzzy part -> path(s)
vega janitor find vega --paths    # bare paths, one per line: cd "$(… | head -1)"
vega janitor list --dirty     # also --stale (with stale_since, last_dirty), --dups; --limit 0 for all
vega janitor report           # merged/prunable worktrees, merged branches, duplicate and stale clones
vega janitor skills discord   # which checkouts install or own a skill; --desc also searches descriptions
vega janitor skills --mine    # counts per skill, most used first, without upstream repos' own skills
vega janitor search "map of git checkouts"   # clones by what their README says
vega janitor embed            # vectors for meaning search (network, key: vega secret triton-embedding)
vega janitor history lanista  # appeared, vanished, dirty, clean, branch, commit; vanished ones too
vega janitor scan             # re-walk the roots after cloning somewhere new
```

- `find` and `list` also show other hosts' checkouts (`host` column, `other hosts as of`): vegad
  carries them through the registry; `--paths`, `report` and `skills` stay on this host.
- Rows show branch (`merged`/`gone` against the default branch), dirty file count, ahead/behind
  its upstream as of the last fetch, last touched, remote (`owner/repo` for GitHub), the herdr
  pane working in it and, in `find`, tools installed from it (uv, cargo).
- `find` takes the best tier of: exact name or `owner/repo`, substring, fuzzy. Clones first. No
  hit: it re-walks the roots once, then exits 1 with `not_found`.
- Each call refreshes checkouts whose git state moved; new ones are found daily or by `scan`.
- `skills` reads the harness skill dirs (`.agents`, `.claude`, `.pi`, `.codex`) and owner sources
  `skills/<name>/SKILL.md`; `vega janitor skills --help` has the columns and `--mine`.
- `search` ranks this host's clones by README words (offline), fused with meaning after `embed`;
  endpoint away: words only, said on stderr.
- Before cloning a repo, run `vega janitor find <name>`: it may already be checked out.
- Never delete a checkout an agent works in (the `pane` column) or one that is dirty or ahead.

`embed` requires `VEGA_JANITOR_EMBED_ENDPOINT` or the first line of
`$XDG_CONFIG_HOME/vega/janitor-embed.url` (normally `~/.config/vega/janitor-embed.url`),
as well as the `triton-embedding` secret. No endpoint is assumed; unset returns
`endpoint_unconfigured` before credential access.
