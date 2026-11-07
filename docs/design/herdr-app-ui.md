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
  plus the inherited 30-second timer for duration displays.
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

## Slice 5b verification (2026-11-07)

A scratch hc core (`HC_SESSION=vega5b`, `XDG_RUNTIME_DIR=/tmp/hc-5b`, PID 64837,
versus user core PID 92972) and a separate `HERDR_APP_HOME` tested a focused mod
opening a kept window overlay. Its focus handle stayed focused before and after
opening (`{"before":true,"after":true}`). The implementation therefore auto-opens
windows and never focuses answer inputs automatically. The agents clone was made
with `mod.clone` in that scratch app; no user app config was changed.

Badge schema 2 retains its counts and adds `decision_texts` (id, title, question,
project) and `checkout`. An unavailable bd retains the last decision texts; an
empty successful list clears them. The UI ignores snapshots over 60 seconds old.
Answers use the recorded checkout, independent of the app's current tab. Dismiss
uses the entered text as the mandatory close reason. Agent mail opens a separate
read-only archive (`vega post log --pane`); notification counts open `vega post notes`.
Neither acknowledges an agent's mail.

## Remote machines (vega-0h4c.26)

The status mod reads the same badge files from every connected `hc machine ls`
machine with `hc exec --on <machine> --json -- python3 -c <reader>`, every 15 seconds.
Reads are bounded (8 seconds remotely, 12 seconds for each local subprocess), with
up to eight parallel readers. This uses the app host's existing hc access and needs
no additional NATS publication or ACL. Python 3 must be on PATH on both ends.
Machine names equal registry hosts by convention. Disconnected machines are not
read; their cards disappear. A failing reader does not hide local or other hosts'
rows, and the status tooltip reports read errors. Original snapshot timestamps
still expire after 60 seconds; reads never renew them. Hosts need synchronized clocks.

Rows carry the source machine independently of project ID; decision seen/input/result
keys include that machine, so identical project and bead IDs on two hosts do not
collide. Remote hc waiting IDs become `hc:<machine>/tN`, and focusing uses the
qualified hc address. Legacy TUI waiters are not presented as hc terminals.
Group totals match both machine and project.

Answer and Dismiss run through `hc exec --on <machine> --cwd <recorded checkout>`
on that machine, passing program and arguments without a shell. The transport
decodes hc's base64 stdout/stderr and inner exit code; an outer success does not
mean the decision command succeeded. Local answers retain their existing execution
path. No remote checkout is opened locally. After merge/apply, verify that a new
remote decision opens the inbox without stealing focus, its answer reaches the
remote UAC, and a remote waiting card focuses its terminal.


## Live-tour polish (vega-0h4c.24)

Machine/project groups use short repository names; routing still uses
`vega.project-id`. A listed child appears only in its visible ancestor's tree,
including children with panes in another space. A child whose ancestor is absent
remains a top-level row. Project totals omit zero values.
This tree rule also applies to pinned children: they stay under their visible
parent rather than receiving a second top-level pinned row.

vegad first resolves the foreground cwd. If no project resolves, it uses that
terminal's space folder from the local core's `herdr-app` document (`doc.get`),
without asking the desktop app or inspecting remote paths. Project
attribution never replaces a resolved foreground project with the launch or space
folder: an agent in rally-coach launched from a vega shell stays in rally-coach.
Own-terminal metadata likewise prefers `foreground.cwd`, falling back to `cwd`.
Local-only project
names come from the identity's source/common-directory path, treating `.git` as
its parent repository, so gate clones and linked worktrees share a name.
New launches also retain their harness kind independently of the displayed glyph.
`vega.harness` keeps the raw launch kind; vegad replaces `vega.kind` with the
theme's display glyph, so the two labels can differ after metadata refresh.

Mail windows render read-only message cards from `vega post log --json` (an object
with `total` and newest-first `messages`, full text and a boolean `read`).
Notification windows consume the existing newline-delimited `post notes --json`.
Neither changes acknowledgements. Empty archives and errors remain explicit.

The inbox closes itself on refresh when its selected project has no decision
cards and no live waiting panes. Unrelated projects do not keep a filtered window
open; an in-flight action prevents closing. Successful Answer/Dismiss removes its
card immediately and closes the last one without waiting for vegad. This is local
to the window; reopening can still show an answered bead until its UAC closes it.

`space.new` creates a shell before mux starts a worktree agent. New launches link
that exact placeholder and worker with ownership labels. `mux close` removes the
placeholder only when the space has no other live terminal and its foreground
has not changed to another program; normal app reconciliation then removes the
empty space. Old spaces without ownership labels stay untouched for user-led live
cleanup. No worktrunk hooks or badge schema changes are involved.

The Slayer kept decisions limited to projects with a claimed UAC (item 6:
decisions belong to the UAC) and kept the existing Git tooltip (item 8). The
toast-position upstream request is drafted only in the bead notes.
