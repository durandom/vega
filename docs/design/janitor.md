# vega janitor: a map of every checkout on a machine

Status: v1 (local index, find, list, report) built 2026-10-10. v2 (vega-lhc.137, absorbs vega-7xm): SQLite
store, then cross-host find/list, README search, history; see Storage and the sections after it.
`mux spawn --cwd <name>` is a follow-up. Bead: vega-egy. Decision (the Slayer, 2026-10-10): a vega plugin, not
its own project. Decision vega-nic.13 (2026-10-19): all three queries, in the order cross-host, search,
history.

## Problem

proteus holds ~870 checkouts under `~/src` (737 clones, 125 worktrees, 4 bare repos, measured 2026-10-10).
Agents and the Slayer think in repo names ("lanista"), tools need paths. Nobody sees where a repo lives, which
copy is current, or which checkouts are dead. Drift example: lanista sits at `~/src/durandom/b4arena/lanista` on
proteus (branch merged, not installed) and at `~/src/durandom/lanista` on ganymede (editable uv install from an
old merged branch).

## Scope of v1

Read-only. janitor runs only read-only git commands, always with `GIT_OPTIONAL_LOCKS=0` (so `git status` does
not rewrite the index), never fetches, never deletes. `report` names the command a human would run.

## Scan

**Roots.** `VEGA_JANITOR_ROOTS` (colon-separated), default `~/src`. An env var, not a config file: one
setting does not justify a config format, and dotfiles (chezmoi) can set it per machine.

**Walk.** Depth-first from each root, directories only, symlinks not followed, subtrees spread over all cores.
A directory is classified before it is skipped:

| Found | Kind |
| --- | --- |
| `.git` directory | `clone` |
| `.git` file, `gitdir:` under `…/worktrees/…` | `worktree` |
| `.git` file, `gitdir:` under `…/modules/…` | submodule: ignored, it belongs to its parent |
| `HEAD` file + `objects/` + `refs/` (e.g. `brainz.git`, `spellkave/.bare`) | `bare` |

- Depth limit 5 below the root (a checkout at `~/src/a/b/c/d/e`). Measured on proteus: no checkout lies
  deeper, and walking depth 7–9 alone costs ~30 s.
- Never entered: `.git`, a bare repo, and `node_modules target .venv venv .terraform .build vendor dist
  build __pycache__ .tox .mypy_cache .pytest_cache .gradle .next`. Other hidden directories are checked (a
  bare `.bare` is found) but not entered.
- The walker descends into checkouts: nested clones are real (`rally-ng/references/*`, `castra/repos/*`) and
  should be findable by name.
- Worktrees outside the roots (e.g. `~/.herdr/worktrees/…`) are found through their repo: every
  `<common>/worktrees/*/gitdir` names its worktree's `.git` file. One whose `.git` file is gone is `prunable`.

**Per checkout** (parallel over all cores, ~3 git calls each):

1. `git status --porcelain=v2 --branch --ignore-submodules=all`, cut off after 3 s: branch, upstream,
   ahead/behind (against the last fetched upstream, no fetch), dirty count (untracked included: they are what a
   cleanup would lose). Cut off, the checkout is `slow`: dirty shows `?`, and branch, upstream and
   ahead/behind come from `symbolic-ref` and `rev-list --left-right --count HEAD...@{u}`. Killing status is
   safe: with `GIT_OPTIONAL_LOCKS=0` it holds no lock and writes nothing.
2. `git log -1 --format=%ct`: last commit date.
3. `git config --get-regexp ^remote\..*\.url$`: remote URL (`origin` first); an `upstream` remote marks a fork.
4. Per repo (common dir, shared by its worktrees): default branch (`refs/remotes/origin/HEAD`, else local
   `main`, else `master`) and `git for-each-ref refs/heads` with `--merged=<default>` and
   `%(upstream:track)`: which local branches are merged or have a gone upstream.

**Remote normalisation.** `git@github.com:o/r.git`, `https://github.com/o/r`, `ssh://git@github.com/o/r.git`
all become `github.com/o/r`; display drops `github.com/`, so GitHub repos show as `owner/repo`.

**Cache.** `$XDG_STATE_HOME/vega/janitor/janitor.db` (default `~/.local/state/vega/janitor/`), SQLite (see
Storage). Each entry stores a fingerprint: the mtimes of `<gitdir>/HEAD`, `<gitdir>/index`,
`<common>/FETCH_HEAD`, `<common>/packed-refs` and `<common>/refs/heads`.

**Refresh rules.**

- Every command loads the cache and stats each entry (a few ms for 900). Entries whose fingerprint changed are
  re-queried, vanished ones dropped. That catches commit, checkout, add, fetch, pull.
- Once a day (last walk older than 24 h) the next call re-walks the roots (~2 s): new checkouts are queried,
  vanished ones dropped, known ones kept as cached. Not a full rescan: that costs ~45 s.
- `find` re-walks once before it reports a miss (a fresh clone is the usual cause).
- Not caught: editing a tracked file without any git command. `vega janitor scan` re-queries everything.
- No cache yet: the first call scans and says so on stderr.
- `VEGA_JANITOR_TRACE=1` prints the scan's phase timings on stderr.

**Budget.** Home view from cache under 2 s. Measured on proteus, 2026-10-10, 867 checkouts plus 46 prunable
worktrees: home view 0.24 s, `find` 0.05 s, `report` 0.23 s from cache; 0.7 s when a few checkouts moved.
Full scan 41–51 s: walk 2 s, `git status` ~40 s, repo info ~9 s. The status phase is filesystem-bound (twice as
much system as user time); 69 checkouts hit the 3 s cap. A 1 s cap saved only 8 s and left 155 at `?`.

**Slow status finding.** Many slow checkouts are small (one has 57 files and takes 14 s): their index holds
stale stat data (e.g. after a copy or restore), so every read-only status rehashes all files. One normal
`git status` rewrites the index and makes it fast. janitor may not write, so `report` lists them with that
command (`slow_status`).

## Fields

| Field | Source | Cached |
| --- | --- | --- |
| path, kind, main (worktree → its repo's checkout) | walk | yes |
| remote (normalised), fork | git config | yes |
| branch, upstream, ahead, behind, dirty, slow | git status | yes |
| last commit, last touched (latest of commit, HEAD and index mtime) | git log, stat | yes |
| merged / gone (current branch), merged local branches | for-each-ref of its repo | yes |
| skills (see below) | harness dirs, `skills/`, `skills-lock.json` | yes |
| owner pane (herdr pane whose cwd is inside the checkout, deepest checkout wins) | `herdr pane list` | no, live |
| installed-from | uv receipts (`editable`/`directory`), `.crates2.json` in `~/.cargo`, `~/.local`, `$CARGO_HOME` (`path+file://`) | no, live |

mise and go keep no cheap receipt that maps an install back to a checkout; they are left out. Disk size
(`du` over ~900 checkouts) breaks the budget and is left out too.

**Skills** (vega-0ft). Read with the git state of a clone or worktree (not bare), so they refresh by the same
rules: a skill added or removed without any git command shows up after `vega janitor scan`. Entries of
`.agents/skills`, `.claude/skills`, `.pi/skills`, `.codex/skills` count when they have a `SKILL.md` or are a
symlink; owner sources are `skills/<name>/SKILL.md`; dot entries are skipped. Per skill and checkout: name
(the entry name), description (frontmatter, first 100 characters), the dirs it appears in (`.agents`,
`.claude`, `.pi`, `.codex`, `skills`), kind per dir (`dir`, `link -> <target>`, `broken`) and the `source`
`skills-lock.json` gives it. Measured on proteus 2026-10-11: full scan with skills 34 s.

## Commands

All output is TOON (AXI 1–6) on stdout; errors are `error{code,message,hint}` with exit 1 (usage: 2).

- `vega janitor`: home view. Host, roots, scan and walk age, counts (clones, worktrees, bare; dirty, ahead,
  behind), then what needs attention (duplicate remotes, merged or prunable worktrees, stale clones, merged
  branches, dirty and slow checkouts), each with the command that shows it.
- `vega janitor find <name> [--paths]`: resolve a name. Best tier only: exact repo name, directory name or
  `owner/repo`; then a substring of path or remote; then a subsequence of the directory name. Clones come
  before worktrees, then most recently touched. `--paths` prints bare paths, one per line. No match: exit 1,
  `not_found`.
- `vega janitor list [--dirty|--stale [--months N]|--dups] [--limit N]`: one row per checkout, default 50
  rows with an "N more" line (AXI 3).
- `vega janitor report [--months N] [--limit N]`: cleanup candidates, each with the command a human would
  run. Worktrees with a herdr pane in them, and dirty, slow or ahead checkouts, are never candidates.
  - `prunable_worktrees` (`.git` file gone): `git -C <repo> worktree prune`
  - `merged_worktrees` (branch merged into the default branch or upstream gone): `git -C <repo> worktree
    remove <path>`
  - `merged_branches` per repo (not the default, not checked out anywhere): `git -C <repo> branch -d …`
  - `duplicate_clones` of one remote: all paths, the most recently touched first
  - `stale_clones` untouched for N months (default 6), no worktree: `trash <path>` (recoverable)
  - `slow_status`: `git -C <path> status` (see above)
- `vega janitor skills [query] [--desc] [--mine] [--worktrees] [--limit N]`: rows
  `skill,checkout,dirs,kind,source,remote`, sorted by skill. Best tier only, as `find`: exact name, then a
  substring of the name (with `--desc` also of the description), then a subsequence of the name. No query:
  checkouts per skill name, most used first. Clones only unless `--worktrees`. `--mine` keeps checkouts
  with no hosted remote (none or a local path) or one whose owner is the user (`VEGA_JANITOR_OWNER`, else
  `git config github.user`, else `gh config get -h github.com user`), hiding upstream repos' own skills
  (openclaw, nanoclaw). Source: the lock's `source`, `own` for an owner source not in the lock, else `-`.
- `vega janitor scan`: full rescan.
- `--skill`, `--help`, `--version` as every plugin.

## Relation to the earlier prototype (~/src/durandom/janitor)

A Python/uv tool from January 2026 (never committed): `discover` finds project directories under a root (also
on titan via ssh + gdu, with disk sizes), records facts in `projects.jsonl` per scan, then a Haiku subagent per
project gathers git facts by shell, reads the README and gives a verdict keep/archive/delete/rename with a
one-page report.

- vega janitor **supersedes its git fact gathering**: deterministic, cached, seconds instead of one LLM call
  per project, and it covers worktrees, duplicates and merged branches, which the prototype does not.
- It **leaves the LLM verdict pass and the disk scans there**: judging what a project is for, renaming, and
  non-git directories (NAS, Time Machine backups, gdu sizes) are a different job. If that pass comes back, it
  should take `vega janitor list` rows as its facts instead of running git per project.
- Taken over: the fork rule (an `upstream` remote) and verdict categories, reduced to read-only candidates
  with a command. Not taken over: disk size, tech stack detection.
- Its scans are not imported: they are a January snapshot of rhdh and titan directories, not checkouts.

## Storage (v2, vega-lhc.137)

SQLite (`rusqlite`, `bundled`: no system library, FTS5 included), WAL, `busy_timeout` 5 s, so two calls at
once do not fail. Why SQLite: rows from several hosts, history and full-text/vector search, each updated per
row instead of rewriting one file. Not speed: the JSON cache loaded in ms; a slow first `find` is `git status`
on moved checkouts (Refresh rules), and SQLite does not change that.

```sql
meta(key TEXT PRIMARY KEY, value TEXT)   -- schema, roots, scanned_at, scan_ms, walked_at
checkout(path TEXT PRIMARY KEY, remote TEXT, touched INTEGER, entry TEXT NOT NULL)  -- this host
peer(host TEXT, path TEXT, kind TEXT, remote TEXT, branch TEXT, dirty INTEGER, ahead INTEGER,
     behind INTEGER, touched INTEGER, at INTEGER, PRIMARY KEY (host, path))     -- other hosts
readme(path TEXT PRIMARY KEY, sha TEXT, text TEXT, vec_key TEXT)                 -- stage 2
readme_fts USING fts5(text, content='readme')                                    -- stage 2
vector(key TEXT PRIMARY KEY, vec BLOB)                                           -- stage 2
event(path TEXT, at INTEGER, what TEXT, detail TEXT)                             -- stage 3
```

- `checkout.entry` is the `Entry` as JSON: the views keep working on the loaded entries (~900 rows, ms);
  SQL serves what needs it (peers, search, history). A schema change bumps `meta.schema`; an older store is
  rescanned, as an older JSON cache was.
- A refresh writes only the rows that changed, in one transaction.
- Migration: the first run imports a current `index.json` (no 45 s rescan) and leaves the file.

## Cross-host find/list (stage 1, vega-7xm)

Goal: `vega janitor find lanista` also answers "ganymede: ~/src/durandom/lanista". janitor itself stays
offline (no NATS client, no network): vegad, which already holds the broker connection, carries files.

1. Export: after every change janitor writes `summary.json` next to the db (atomic): `{host, at, rows:
   [[path (home-relative), kind, remote, branch, dirty, ahead, behind, touched]]}`, prunable worktrees left
   out. ~900 rows x ~80 B ≈ 70 KB, under the NATS default `max_payload` (1 MiB); the bucket sets no
   `max_value_size` (`vega post provision registry`). Over 512 KB vegad logs and skips it.
2. Publish: vegad's registry reconcile adds `<host>.janitor` = that file; the reconcile puts it only when it
   changed (`last`).
3. Bulk reads skip it: `vega_registry::read` lists `*.janitor` keys but does not fetch them, as notes, so the
   15 s reconcile and every reader do not pull ~70 KB per host.
4. Pull: every 5 min vegad gets `<host>.janitor` of each other fresh host and writes
   `peers/<host>.json` (atomic) when its `at` moved; a key that is gone removes the file.
   The same janitor tick runs `vega gate gc` for abandoned review clones. That is best-effort:
   vegad logs one failure and continues its relay loop; a later successful tick resets the notice.
5. Import: janitor replaces a host's `peer` rows when its file is newer than the import recorded in `meta`;
   a removed file drops the host.

Views: `find` and `list` (all and `--dirty`) add a `host` column (this host first) and an `other hosts as of`
line; peer rows show no pane or install. `find --paths` prints this host's paths only (for `cd`); a name
checked out only elsewhere is `not_found` with the hosts in the hint. `report`, `skills` and the home view stay local (peer rows lack the git dirs they need); the home
view adds one line per peer host: rows and age. Broker away: the peer files stay; rows older than a day get a
`stale` note. deimos has no registry rights: no peer files, local only, said in the home view.

## README search (stage 2, optional embed)

- Offline core: when a clone's row is new or changed (and on `scan`), its README (`README.md`, else
  `README*`, first 8 KB, after the repo name) goes into `readme` if its sha moved; a clone without one gets
  an empty row. `vega janitor search <words>` ranks with FTS5 (bm25, any word); works on deimos.
- Embed stage: `vega janitor embed` sends READMEs without a vector to the OpenAI-compatible endpoint
  (`VEGA_JANITOR_EMBED_ENDPOINT`, or `~/.config/vega/janitor-embed.url`; no default, model
  `qwen3-embedding-0.6b-q8-0`, 1024 dims, as brainz `scripts/vault-embed.py`). Key: the vega secret
  `triton-embedding` for tool `janitor` (the Slayer adds it; missing: `unknown_secret` with the hint). Vectors are content-addressed (`sha256(model, text)`), so a moved
  or duplicated clone costs no call. Never run by a read command; manual or by a later vegad job.
- Rows `path,remote,score,about` (score: the fused rank, 1.00 = first everywhere; about: the README's first
  line of prose). `--words` skips the embedding call.
- With vectors and a reachable endpoint (2 s), `search` embeds the query (Qwen3 instruct prefix) and fuses
  vector and FTS ranks (RRF, as brainz `hybrid`); otherwise FTS only, with a stderr note.
- Cosine over all vectors in Rust (900 x 1024 floats: ms); no sqlite-vec extension to bundle or load.
- This host's READMEs only: peer summaries carry no text.

## History (stage 3)

- Every refresh and walk diffs old against new entries and appends `event` rows: `appeared`, `vanished`,
  `dirty` (0 → n), `clean` (n → 0), `branch` (old → new), `commit` (HEAD oid moved on the same branch;
  the commit date in seconds misses two commits within one second). This host only.
  A cut-off status (`slow`) knows no dirty count and records no dirty or clean. The first save of a store
  (scan or JSON import) records every checkout as `appeared`: history starts there.
- Events are as fine as the refresh: a change shows up when a call sees it, so a checkout dirtied and
  cleaned between two calls leaves nothing.
- `vega janitor history <name> [--limit N]`: rows `when,path,what,detail`, newest first, for `find`'s hits
  plus vanished paths whose directory name, repo name or path matches ("where was it").
- `list --stale` gains `stale_since` (touched + months) and `last_dirty` (last `dirty` event).
- Kept one year; older events are dropped on write.

## Later: `vega mux spawn --cwd <name>`

`vega mux spawn --cwd lanista` resolves a name that is not a path through `vega janitor find --paths` (exact
tier only; several hits → an error listing them). Not built now.

## Rejected

- Own project (usable without vega): the Slayer chose a plugin; it reuses the registry, herdr and the guides.
- `fd`/`find` subprocess for the walk: an extra dependency for no speed gain over a pruned `read_dir` walk.
- Fetching for exact ahead/behind: writes to every repo and hits the network; the last fetch is good enough
  for a map, and the views say "as of the last fetch".
- Background refresh daemon: a stat pass per call plus a daily re-walk meets the budget.
- `git status -uno` for speed: it misses untracked files, the very thing a cleanup would lose, and saves
  little on the slowest repos (their cost is rehashing tracked files).
- A daily full rescan in the hot path: ~45 s on proteus.
- janitor talking to NATS itself: a broker dependency and connect latency in every call; vegad already
  holds the connection and carries files both ways.
- sqlite-vec: a loadable extension to ship for ~900 vectors that a loop scores in ms.
- Peer rows in the registry's 15 s bulk read: ~70 KB per host per reader every 15 s for a view read rarely.
