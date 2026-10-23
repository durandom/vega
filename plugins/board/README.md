# vega board

A herdr pane for the UAC: an Inbox of what needs the Slayer by default, the beads one key away, answers as
bead comments. Layout, data sources, keys and mouse: [board-ui.md](../../docs/design/board-ui.md). One Rust/ratatui binary, `vega-board`, docked as a split pane (`dock`) or floating as a popup
(`board`).

Agent guide: `vega skills board`

## Provenance

Copied, not forked, from [miiraheart/herdr-beads](https://github.com/miiraheart/herdr-beads) at commit
`72f59e1` (MIT, see `LICENSE`). We extend it beyond upstream (sentinel status is planned), so there is no
upstream sync. The List / Table / Kanban views, live updates and bd bridge are upstream's; see its README for
the full feature tour.

## What vega adds

- **Inbox tab (default).** One list of what needs the Slayer, waiting panes first, then newest first: the
  open children labelled `slayer-decision` of the repo's one open `uac-tracker` epic, the headed panes waiting
  on him (vegad's badge file), and the notes (`vega post notes`, quarantine included). Enter jumps, `m`
  answers, `c` clears a note. The top-bar badges open it filtered to their kind.
- **Lane preset.** On the bead tabs `D` toggles `lane|all`. Lane: the open beads labelled `uac-lane`: what
  the UAC's lanes (sentinels it spawned or adopted) created. `vega uac status` stamps the label; the board
  only reads it. The list runs oldest first, each row ending in its creator (`created_by`,
  `<host>/<worktree>`) and age; `D` shows the open count and the oldest bead with its age, the same numbers
  as the `lane_beads:` line of `vega uac status`.
- **Answer.** `m` on a bead opens a prompt; Enter runs `vega uac answer <bead> "<text>"` in the repo: a bead
  comment by Slayer plus a mail to the project's UAC; its `uac:` line shows in the status bar. The
  pane never closes the bead: the UAC closes it after acting.
- **Comments in the detail** (`Enter` or `d`), oldest first, so a back-and-forth reads top to bottom.
- `--skill`, `--help`, `--version` (clap); a TUI start without a terminal fails with a hint to `--selftest`.

## Install

`mise run install` (main checkout only) installs `vega-board` to `~/.local/bin` and links the plugin into
herdr (`link-herdr.sh`). Bind the actions in `~/.config/herdr/config.toml`:

```toml
[[keys.command]]
key = "prefix+b"
type = "plugin_action"
command = "vega.board.toggle-dock"

[[keys.command]]
key = "prefix+shift+b"
type = "plugin_action"
command = "vega.board.open-board"
```

## Keys

`?` in the board lists all keys; the footer shows the ones that apply. The vega ones: `N` the Inbox, `D`
lane/all, `m` answer, `V` starts vegad. `q` quits; `Esc` backs out a layer.

## Symbols

Every symbol comes from `[symbols]` in `~/.config/vega/theme.toml`: set `nerd` (the default; monochrome Nerd Font
glyphs, the terminal needs a Nerd Font, e.g. DankMono Nerd Font Mono) or `emoji`, plus per-topic overrides. The
topics and both sets: [theme-symbols.md](../../docs/design/theme-symbols.md). The board picks up a change within
2 s and shows a mistake in the file in red in the top bar. The top bar also repeats the UAC pane's badges for this
repo's project, from vegad (`vega_herdr::badges`): unread notes, open decisions, panes waiting on the user.

## Verify without herdr

`vega-board --selftest` prints the tracker's decisions, the lane beads and the grouped beads of the current
repo as text.
