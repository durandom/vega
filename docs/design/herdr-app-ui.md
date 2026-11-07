# vega in the herdr-app UI

Slice 5b of the [hc migration](hc-migration.md) (vega-0h4c.10): what vega shows in herdr-app and how the
Slayer acts on it. Decided with the Slayer on 2026-11-07. Background: the herdr TUI sidebar (machines tree,
priority-sorted agents list, vega tokens from `report-metadata`) and the `vega board` Inbox
([board-ui](board-ui.md)).

## Decisions

1. **`vega-agents` replaces the shipped `agents` mod** in the vega layout (`config/herdr-app/layouts/vega.json`).
   It starts as `herdr-app do mod.clone '{"name":"agents","as":"vega-agents"}'` and keeps what the original
   does: sub-agent tree from `terminals().parent`, priority sort, "Needs you", click to focus. vega adds:
   grouping by machine and project (a sort option, replacing the TUI "machines" tree), the UAC badge, the
   harness icon, mail count, hand (waiting), scale (decisions) and git state.
2. **Every vega icon is clickable on its own** (`on_click` with `cx.stop_propagation()`): mail opens that
   agent's inbox, hand focuses the agent's tab, scale opens the decision in `vega-inbox`. In the TUI sidebar
   these icons were not clickable.
3. **The shipped `spaces` mod stays unchanged.** Project totals show on the group rows of `vega-agents`.
4. **Decisions go to a `vega-inbox` window, not the chat.** An overlay of `kind: "window"` with `key` and
   `keep: true`, so it stays until closed and survives a restart. It shows decision cards with an answer
   input and Answer / Dismiss buttons (`vega uac answer <bead> "<text>"`, `bd close <id> --reason ...`), and
   the waiting panes, each with a click to focus.
5. **Only a new decision opens `vega-inbox` by itself,** non-modal, together with a toast whose action
   opens the window. A waiting pane does not open it: herdr-app already toasts and sounds on `blocked`, and
   the hand icon stays in the sidebar. Before building: check that an auto-opened window does not take the
   keyboard focus; if it does, fall back to toast plus badge.
6. **Notes (unread mail) are not in `vega-inbox`.** They stay a mail count in the sidebar; a click opens them.
   This keeps the noise of GitHub durandom/vega#8 out of the window.

## Data path

- **Per terminal:** vegad writes the tokens it reports to herdr today (`report_metadata`,
  `plugins/post/src/main.rs`) as hc labels on hc terminals: `hc label <t> vega.notes=1 vega.waiting=...`
  (`key=` removes). The mod reads them from `terminals()[i].labels`; `render()` runs again on every change,
  no polling.
- **Decisions and waiting panes:** the vegad badge file `$XDG_STATE_HOME/vega/badges/v2/<project>.json`,
  extended with the decision texts (bead id, title, question, project). The mod watches the folder with
  `watchFolder()`.
- **Answers:** `exec()` of the vega CLI, JSON where a command needs to return data.

## vega board

The board Inbox moves to `vega-inbox`; the herdr plugin (dock, popup, `prefix+u`) goes with the TUI. The
bead tabs (List, Table, Kanban) stay a TUI that runs as an hc tab when needed. `vega open` is still open
(vega-0h4c.11).

## Limits of herdr-app mods

- Toasts take one click action and no buttons, hence the window.
- No tree component: tree lines are drawn as the shipped `agents` mod does.
- A mod cannot stream `hc events`; `terminals()` and `watchFolder()` cover what vega needs.
