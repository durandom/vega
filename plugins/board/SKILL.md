---
name: vega-board
description: Shows the UAC's open Slayer decisions and the repo's beads in a herdr pane, where the user answers a decision as a bead comment. A human tool behind herdr keybindings. Use when the user asks about the board pane, or when a UAC files decisions for the user to answer there.
---

# vega board: decisions and beads in a herdr pane (human tool)

The user opens it from herdr (`vega.board.toggle-dock`, `vega.board.open-board`). Agents do not run the TUI. Layout, data sources, keys and mouse: docs/design/board-ui.md.

- Tabs: List, Table, Kanban (the beads) and Inbox (what needs the user). The board starts on the Inbox; `K`/`Tab` cycles, `N` opens the Inbox and goes back.
- Inbox: one list, waiting panes first, then newest first: the open children labelled `slayer-decision` of the one open epic labelled `uac-tracker` (none with no such epic, or several; the Inbox says so), the headed panes waiting on the user (vegad's badge file), and the notes vega showed as herdr toasts (`vega post notes`, all hosts, quarantine included). Enter jumps (a decision: its detail in the bead tab; a pane or note: focus the pane), `m` answers, `c` clears a note (the store keeps one watermark per host, so that host's older notes go too). Showing the Inbox marks the notes read, which clears the notes badge.
- Bead tabs: `D` toggles `lane|all`. Lane: the open beads labelled `uac-lane` (stamped by `vega uac status`; the board only reads), oldest first, with creator (`<host>/<worktree>`) and age.
- The user answers with `m`, which runs `vega uac answer <bead> "<text>"` in the repo: a bead comment by Slayer plus a mail to the project's UAC. The UAC closes it with the answer. `x` dismisses a stale decision: `bd close` with reason `dismissed by the Slayer: <typed reason>`.
- Tab row: the tabs (`Inbox <n>`), the UAC pane's badges for this repo's project as vegad counted them (unread notes, open decisions, waiting panes, each only when non-zero; a click opens the Inbox on that kind), and the chips `lane|all`, `repo|global`, `shown/total` (a click toggles, toggles, opens the filter prompt).
- Footer: the status message and only the keys that apply to the tab and selection; it wraps when narrow. `?` lists all keys.
- Symbols come from `[symbols]` in `~/.config/vega/theme.toml` (docs/design/theme-symbols.md); a mistake there shows in red in the tab row.
- vegad's state on this host (running, pid, version, binary current or stale) heads the Inbox; a stopped vegad also shows in the tab row. `V` starts it (`vega post relay start`).
- A decision bead shows up when a UAC files it: `bd create --type=decision --parent=<tracker> --labels=slayer-decision ...`.
- `vega-board --selftest` prints the beads as text.

`vega post identity --help`.
