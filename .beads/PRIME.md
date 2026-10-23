# Beads Workflow Context — vega

> **Context Recovery**: Run `bd prime` after compaction, clear, or a new session.
> This file is the tracked `bd prime` override (`.beads/PRIME.md`). Durable rules live here,
> not in `bd remember`.

# Storage

A local Dolt server per clone (`.beads/dolt`, not in git), replicated to the `VEGA` database on
the configured remote (for example `http://dolt.<tailnet>:8080/VEGA`). vegad pulls and pushes (`vega bd status`):
no manual `bd dolt pull`/`push`, and git does not carry beads. A new machine runs `vega bd init`;
never copy `.beads` between machines.

Template for projects provisioned with `vega bd init` (docs/design/beads-sync.md): a local Dolt
server per clone, replicated to the project's database on its configured Dolt remote. vegad pulls
every 60 s and pushes within seconds of a write: do not run `bd dolt pull` or `bd dolt push` (the
bd shim skips them in agent sessions). `vega bd status` shows the sync state; a `stopped` project waits for the UAC to resolve a conflict.

# Rules

- **Language**: all bead content in English, except `slayer-decision` beads: those are in the
  language the user speaks with the UAC (docs/design/uac-role.md).
- **Field shape**: `--description` = why + what, with file paths. `--acceptance` = one testable
  "Done when …" including the gates (`mise run check`). `--design` = pointers to `docs/design/*.md`.
- **Live log**: `bd update <id> --append-notes`. Never `--notes` on an existing bead: it replaces.
- **Discovered work**: `bd create ... --deps discovered-from:<current-id>`; never silently widen scope.
- **Hierarchy**: new beads get a parent (`--parent=<epic>`) when an epic exists.
- **Quoting**: pass notes via a quoted heredoc (`--append-notes "$(cat <<'EOF' ... EOF)"`), never a
  double-quoted string: backticks and `$(...)` would run.
- **Do NOT use `bd edit`** (opens $EDITOR). Do not track tasks in TodoWrite or markdown.
- **Merges**: worktree agents never merge, push or install; the main vega session does.
- **Gate**: in a repo with `.vega/gate.toml` (vega's own included), a sentinel runs `vega gate start` in its
  worktree before its done report and names the verdict in it; the UAC's push reuses that pass
  (docs/design/gate.md).

## Close-Reason Contract

`bd close --reason` is the permanent handoff record. It must contain:

1. **What changed**: files and approach (2–4 sentences).
2. **Gate results**: each gate with its real result (e.g. "mise run check: exit 0").
3. **Commit hash(es)** of the landed work.
4. **Follow-ups**: IDs of beads filed for remaining work.

A bare `bd close <id>` without `--reason` is a protocol violation.

## Essential Commands

- `bd ready` / `bd blocked`: unblocked work / what waits
- `bd show <id>`: full context (`--long` after compaction)
- `bd update <id> --claim`: claim before starting
- `bd create --title=... --description=... --acceptance=... --type=task|bug|feature|epic|chore|decision --priority=0..4`
- `bd dep add <issue> <depends-on>`: issue is blocked by depends-on
- `bd close <id> --reason "..."`: see the contract above
- Append `--json` for structured output.
