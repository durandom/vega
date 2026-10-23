# vega board: UI layout

The board's screen: one tab row, a body, a contextual footer. Decided by the Slayer (vega-lhc.92): everything
that needs the Slayer lives in one **Inbox** tab; the bead tabs keep two presets; the footer shows only the keys
that apply. Symbols and badge colours come from `theme.toml` (`[symbols]`, `[symbols.colors]`,
[theme-symbols.md](theme-symbols.md)); the mockups use the emoji-free letters `N D W` for the notes, decisions
and waiting symbols.

## Layout

Three bands: the tab row (1 line), the body, the footer (1 line, more when it wraps).

~80 columns, List tab:

```
 List  Table  Kanban  Inbox 6   N2 D3 W1          lane|all  repo|global  12/40
 ● Open (9)
   P1 ⧉ vega-lhc.92  vega board redesign: Inbox tab ...
   P2 ⧉ vega-lhc.93  ...
 ◐ In Progress (3)
   ...
 claimed
 K view · j/k move · Enter detail · m answer · c claim · x close · h/l fold
 D lane|all · / filter · ? help · q quit
```

~140 columns, Inbox tab:

```
 List  Table  Kanban  Inbox 6   N2 D3 W1                                       all kinds  repo|global  6/6
 vegad on proteus: running · pid 812 · 0.4.1 · binary current
 W  asking  vega-lhc.92-board  asks: tabs or chips first?                        [wAP:p2]
 W  input   flist              permission: Bash(cargo test)                       [w3V:p4]
 D     12m  vega-lhc.95        Merge order for the sync branches?                 decision
 N      3m  proteus · ci       vega-lhc.88 green: 4 checks                        [w3V:p2]
 N      2h  deimos · quarantine  reply from deimos: done (for you only)          [w1:p1]
 D      1d  vega-lhc.80        Keep the old N overlay as an alias?               decision
 Enter jump · m answer · c clear (older notes of that host too) · j/k move · r refresh · K view · ? help · q quit
```

- **Tab row.** Left: the tabs `List · Table · Kanban · Inbox <n>` (n = rows in the Inbox), then the UAC
  pane's badges (vegad's counts, non-zero only; on the Inbox the badge of the kind filter is drawn selected,
  its colour as the background), then the state marks (`+closed`, `+auto`, `+live`,
  `/filter`, a theme mistake in red, `vegad stopped · V`, `MOVE`). Right-aligned chips: on the bead tabs
  `lane|all`, `repo|global` and `shown/total`; on the Inbox the kind filter (`all kinds`, or the kind),
  `repo|global` and `shown/total`. The active half of a two-way chip is highlighted. When the row would
  overflow (a narrow dock), the other tabs shrink to their letter (`L`, `T`, `K`, `I6`) and the chips show
  only their active half (`all  repo  12/40`); a click still toggles.
- **Body.** The bead views as before (List, Table, Kanban, detail side pane or modal). The Inbox: vegad's
  state line on this host, then one row per item, newest first, waiting panes on top (they are now).
- **Footer.** The status message (green) first, then the keys that apply to the current tab and selection,
  joined by ` · `. When they do not fit, the footer wraps onto further lines at a separator; it never cuts a
  key. Counts are not in the footer (they are chips).

## Inbox: kinds and data sources

| Kind | Symbol | Source | Row | Enter | m | c | x |
|------|--------|--------|-----|-------|---|---|---|
| decision | `decisions` | the loaded beads: open children labelled `slayer-decision` of the one open `uac-tracker` epic (as before) | age (created), id, title | the bead: back to the last bead tab, all beads, detail open | `vega uac answer <bead>` prompt | no: hint "the UAC closes it after your answer (m), or dismiss it (x)" | dismiss: a one-line reason prompt (required), then `bd close <bead> --reason "dismissed by the Slayer: <reason>"` |
| waiting pane | `waiting` | vegad's badge file `$XDG_STATE_HOME/vega/badges/<project>.json`, key `waiting_panes`: the headed panes whose attention is for the user (asking, silent, input), with name, attention and title, as vegad labels them | attention, agent name, title, pane | focus the pane (`vega_herdr::pane`, this machine first) | no: hint "Enter, then answer in the pane" | no: hint "it leaves when the pane moves on" | no: hint "only a decision can be dismissed" |
| note | `notes` | `vega post notes --json` (every host via the registry), loaded in the background; `quarantine` notes (replies from inbox-only hosts) carry the kind and "for you only" | age, host · kind, title, pane | focus the note's pane (this host only) | the note's bead, if it names one | `vega post notes clear --upto <host>=<id>`: a watermark, so the older notes of that host go too | no: same hint |

- The render loop never runs a command or a network call. The badge file is read every 2 s (as for the
  badges); the notes load in a background thread when the Inbox opens, on `r`, and when vegad's unread count
  changes. Opening the Inbox marks the shown notes read, which clears the notes badge.
- `waiting_panes` older than the badge's stale limit (60 s) is ignored, like the counts.
- Without a tracker epic (or with several) the Inbox has no decisions and its vegad line says why.

## Bead tabs: presets

Two presets: `lane` (open `uac-lane` beads, oldest first, creator and age) and `all`. `D` toggles; the lane
summary (`3 open, oldest v-7 4d`) shows in the status message. The decisions preset is gone: decisions are in
the Inbox. The board starts on the Inbox, bead tabs on `all`.

## Keys

Global: `K`/`Tab` next tab, `Shift+Tab` previous, `N` the Inbox, `?` help, `q` quit, `Esc` backs out a layer,
`r` refresh, `S` scope, `V` start vegad, `f` zoom, `A` auto-dock.

Bead tabs: as before (`j k`, `gg G`, `Ctrl+d/u/f/b`, `h l`, `1-9`, `[ ]`, `Enter`, `d`, `v`, `c`, `x`, `p`,
`n`, `m`, `a`, `e`, `s`, `F`, `o`, `/`, `C`, `y`, `0 $`) plus `D` lane/all.

Inbox: `j k ↑ ↓`, `gg G`, `Enter` jump, `m` answer, `x` dismiss a decision, `c` clear, `Esc` clears the kind filter.

The `?` help and `vega-board --skill` list the same keys.

## Mouse

Single left click and the wheel only; no double-click, no drag.

- Tabs: switch. Badges: open the Inbox filtered to that kind (notes, decisions, waiting); a click on the badge
  of the active filter shows all kinds.
- `lane|all`: toggles the preset (as `D`). `repo|global`: toggles the scope (as `S`). `shown/total`: opens the
  filter prompt (as `/`). Inbox kind chip: back to all kinds.
- Rows select (bead rows, Inbox rows); ids in the detail jump. The wheel scrolls what is under the pointer.
