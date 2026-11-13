# vega in the herdr-app UI

Slice 5b of the [hc migration](hc-migration.md) (vega-0h4c.10): what vega shows in herdr-app and how the
Slayer acts on it. Decided with the Slayer on 2026-11-07. Background: the herdr TUI sidebar (machines tree,
priority-sorted agents list, vega tokens from `report-metadata`) and the former dashboard Inbox (preserved at `herdr-tui-final`).

## Profile windows (vega-0h4c.85, vega-0h4c.86)

`config/herdr-app/profiles/<name>.toml` declares `layout`, `mods`, `[settings]`,
`[shared]` seeds and `[[tab]]` groups (`name`, one or two `[[tab.terminals]]`
entries with `name`, `machine`, `argv`, optional `optional = true`). Terminal slot
names are unique across groups. Legacy flat `[[terminals]]` remains supported;
a profile cannot mix flat terminals and groups. The personal
`herdr-app.profiles` config unit resolves embedded defaults and local overrides
under `$XDG_CONFIG_HOME/vega/config`, and writes `$XDG_CONFIG_HOME/herdr-app-<name>`.
Mod folders come from the same sources (local third-party folders are supported).
Settings overrides merge into the main settings source. Only a missing `shared.json`
is seeded with the selected layout and `frame.welcomed = "finished"`; running app
state is never edited. An existing manual home may still need its welcome dismissed.

`vega window <name>` is a `vega-window` binary in `plugins/config`. It reuses live
hc terminals by `vega.window=<name>` and `vega.window.slot=<terminal-name>`, starts
missing entries with `hc run --bg --no-parent --no-notify --on <machine>` and explicit
argv, then resolves `{ "terminal": "profile:<terminal-name>" }` layout slots to
terminal addresses. It opens/raises `herdr-app` with the profile's `HERDR_APP_HOME`
and selects that layout. Direct terminal blocks need no `herdr.space`/`herdr.tab`
labels: all homes still share hc inventory, but this window creates no shared spaces.
The main sidebar excludes terminals labelled `vega.window` from its lists and trees.

The shipped `status` profile has top tabs proteus, ganymede, io (btop), and tokens
(ganymede token-burn tui), giving each TUI the full body below the bar. io is optional:
when its labelled live slot is absent, `hc exec --on io` checks btop in its login
shell before starting; absence or an unavailable machine warns and omits that group.
Other start failures remain errors. Live slots skip the availability check.

Grouped layouts contain exactly one `id: "profile:tabs"` node. The launcher fills
that stack with stable group IDs, using one direct terminal block per slot (two
split horizontally). `vega-window-tabs` is a small top-nav mod included in the
profile's mods; it draws groups from the live layout and calls `layout.select`.
Model tabs (`tab.new`) belong to one machine's space, so layout tabs avoid shared
spaces, placeholder shells, and adopting cross-machine terminals into the wrong
core. No duplicate model tabs are created on rerun. Herdr owns active stack click
state; regenerating the same tree preserves it. The profile's keys.json derives
from the configured main keys source, remapping tab.next/prev/select bindings to
mod commands (Ctrl-Tab, Cmd-Shift-[ / ], Cmd-1..9 and ctrl-s n/p/1..9); terminal
bindings are retained. App-owned shared state is never rewritten.

Tests use temporary homes and fakes; live validation belongs to the UAC.
Closing a window leaves its terminals running; reopening reuses them. Failed starts
leave successfully created terminals available for the next invocation.

## Last-focus navigation (vega-0h4c.70)

`ctrl-s l` or `cmd-;` toggles the last agent, `ctrl-s shift-l` the last space,
and `ctrl-s ;` the last pane in the current tab. Press again to return (A → B → A).
Pane history includes mod panes; agent history includes qualified remote addresses
and ignores shells. Closed targets are skipped; without a previous live target the
command does nothing. The keys deliver `vega-sidebar.last-agent`,
`vega-sidebar.last-space` and `vega-sidebar.last-pane` through `nextCommand()`.

History stays in module memory, shared by all sidebar copies on herdr-app 0.1.14+,
and resets on mod reload or app restart. Renders sample focus; a 300 ms background
loop also samples because the API does not guarantee a render for every focus
change. Copies share the polling budget and only read agent inventory when the
terminal changes. Broadcast key deliveries are batched for one microtask so only
one shown sidebar acts; hidden or removed copies cannot strand a retained owner.
Popup, mail and project-window copies do not participate. Sampling never forces redraws.
Run `vega config apply herdr-app` to apply the updated mod and keys.

## Login keychain windows (vega-oiwq)

The `vega` status mod reads vegad's local `keychains.json` registry projection every five seconds
in one local read. It uses the same `XDG_STATE_HOME` as vegad, as for badges. There is no per-host
exec polling. The right side shows one monochrome lock for the focused terminal's machine (local
terminals: this host; Slayer 2026-11-10), coloured with theme
`ansi2` (unlocked), `ansi1` (locked), or `dim` (unknown). Tooltips give host, state, and `since`.
Stale snapshots and stale probe samples become unknown rather than retaining a red/green claim.
The projection reads `<host>.status.keychain` for all hosts. Samples carry their own `at`, so
inbox-only hosts such as puck appear without a `<host>.beat`. Their vegad sends an exact keychain
command to its theme control host; that private daemon validates and relays only the display
sample. Puck receives no registry write permission. Unknown inbox-only status keys are ignored.

A newly observed lock opens one global non-modal window, deduped across mod copies and restarts
by `host + since` in shared state. A click on a red lock opens it again. The mod starts an
unlabelled hc terminal on that host (`hc run --bg --no-parent --no-notify`, `--on` for peers);
it is never adopted into a tab. An existing live dedicated terminal is reused when reopening.
The window layout contains a draggable title bar with Close, and that terminal exactly once.
The title bar is a `vega` copy with `keychainTitle` props and no background pollers. The window
does not take focus automatically. The title bar shows the lock in its state colour, the host
and the state with its `since` time. The window stays until the Slayer closes it (Slayer
2026-11-10): an unlock only turns the title green; Close closes the window and kills only its
dedicated terminal. While the window is open and the host is locked, the mod
reads that host's keychain itself every 5 s (`security show-keychain-info`, through `hc exec` for
peers, one probe per host across mod copies), so the lock turns green right after the password;
a newer locked sample from vegad wins again.

The terminal explains the lock (host, why agents fail, the exact command
`security unlock-keychain ~/Library/Keychains/login.keychain-db` it runs on that host, that the
password is typed only there, without echo, never in argv), waits for Enter, then runs it with
echo off, retries on a failure after the next Enter, and on success says so and waits for the
window to be closed.

Development stays in the worktree: run `herdr-app check config/herdr-app/mods/vega` and
`node --test scripts/herdr-app-mods.test.mjs`. For a live tour use an isolated `HERDR_APP_HOME`
and scratch hc core as in [slice 5b verification](#slice-5b-verification-2026-11-07), with fake
keychain data and a harmless terminal script. Never copy mods into the user's app home or apply
config from a sentinel worktree. The UAC handles deployment/config apply after landing; the app
reloads applied mod files live. Real unlock validation belongs to the Slayer in the popup.

Verification (2026-11-10): an isolated app home and hc core in the worktree's `target/` rendered
red/green locks and one popup containing a harmless waiting terminal, with no app errors. A fake
unlocked cache update closed the popup. The scratch app/core were stopped afterwards; no real
keychain was locked or unlocked. Unit tests cover repeated locks, click-to-reopen, colours,
launch/unlock races, and ignoring persisted overlay handles from a previous app session.

## Decisions

1. **`vega-sidebar` replaces the shipped `agents` mod** in the vega layout (`config/herdr-app/layouts/vega.json`).
   It starts as `herdr-app do mod.clone '{"name":"agents","as":"vega-sidebar"}'` and keeps what the original
   does: sub-agent tree from `terminals().parent`, "Needs you", click to focus. Stage 1 removes priority sorting. vega adds:
   the UAC project tree across machines (the default view, replacing the TUI "machines" tree; decision 3),
   the UAC badge, the harness glyph, mail count, hand (waiting), scale (decisions) and git state.
2. **Every vega icon is clickable on its own** (`on_click` with `cx.stop_propagation()`): mail opens that
   agent's inbox, hand focuses the agent's tab, scale opens the decision in `vega-inbox`. In the TUI sidebar
   these icons were not clickable.
3. **`vega-sidebar` is the only sidebar list (Slayer, 2026-11-07, revises "spaces mod stays").** The layout
   drops the shipped `spaces` list and the Settings / Add machine footer. The manually ordered project tree:
   one group per project with a UAC on any machine, in an order the Slayer drags (kept in the mod's
   localStorage); under it the UAC first, its sentinels as tree branches, then the project's other agents on
   any machine. Below, "Spaces": every space without a UAC, a click selects it. Rows follow the TUI symbols
   ([theme-symbols](theme-symbols.md)): status, `u▲c` only on the UAC row, the harness glyph (`✳` `֍` `✈`
   `π`) in its colour, name, git, badges, duration and the machine's `$host` label (`♆₈`) right-aligned in
   its host colour from `~/.config/vega/theme.toml`. No sentinel cog: the tree line marks a child. The
   project row carries git and the positive totals `✉ ⚖ ✋`.
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

7. **The status line is the shipped one without the palette hint.** The `vega` mod draws it in place of
   `workspace-status` and keeps its background work (badge collection, guarded vegad start, inbox auto-open); the
   former `vega · vegad running · ⚖ N` line is gone.

The `vega` mod owns vegad startup (vega-0h4c.42): it runs the idempotent `relay start` locally and, via `hc exec`, on connected hc machines. The
local start runs only when
`HERDR_APP_HOME`, `XDG_CONFIG_HOME` and `XDG_STATE_HOME` are unset or resolve to
`~/.config/herdr-app`, `~/.config` and `~/.local/state` respectively. Scratch apps
with a private app/config/state home never start a daemon. Scratch probes must
stop every app or core they start; the mod does not own or stop the shared vegad.
The herdr TUI startup hook is unchanged.

## Space rename (vega-0h4c.48)

Right-clicking a non-UAC space row opens a small
`kind: "popup"` overlay. Rename replaces the
menu item with an input prefilled with that space's name. Only the explicit
Rename click focuses that input; input state is created in the event handler,
never during render. Enter submits a nonempty trimmed name through
`run("space.rename", {id, name})` and closes the popup. The original space ID is
kept intact, including a remote machine prefix such as `puck/sp…`. Escape closes
the popup without submitting, and a right-click does not select the space.

## Project Config overlay and views (stage 1, vega-0h4c.49)

The epic vega-0h4c.50 keeps project preferences in vega, independent of herdr-app.
`vega-project`, a second binary of the existing config plugin package, owns
`$XDG_CONFIG_HOME/vega/projects.toml` (normally `~/.config/vega/projects.toml`).
It follows the theme preferences pattern: TOML edits preserve comments and unknown keys,
missing files mean empty preferences, malformed data fails without a write, and atomic
replacement preserves file mode and symlinks. An advisory lock serializes writers.
Preferences are personal to the app host, outside the registry and project repository;
syncing the file is a future option if the app runs on several hosts.

```toml
[projects."github.com/team/repo"]
name = "My project" # empty means use the UAC's vega.project label
labels = ["work"]
pinned = true # keep the UAC visible with the Active filter

[views.Work]
labels = ["work"]
machines = ["puck"]
```

`vega project config <project-id> [--name <name>] [--label <label>]...
[--unlabel <label>]... [--pin|--unpin] [--json]` reads or edits one entry. Keys are the full normalized
remote from `vega.project-id`, never a terminal ID or short repository name.
Label additions/removals are idempotent; removal wins if both are requested.
`vega project config --json` returns `{projects: {...}, views: {...}}` in one exec.
An individual read returns `{projects: {<id>: {name, labels, pinned}}}`; absent entries have
empty defaults with pinned false, and a read never creates the file.

Right-clicking a project header or its UAC row opens a popup with a descriptor-driven
tab bar. Stage 1 exposes only Config (display name, comma-separated labels, Save).
This requires the full `vega.project-id` label; legacy UACs without that label remain
visible but cannot open Config until vegad supplies their stable identity.
Later stages add UAC, Inbox, Decisions and Notifications descriptors and renderers.
Inputs load once on opening, never autofocus, and remain editable on save errors.
Save uses argv via `exec("vega", ["project", "config", ...])` on the app host, including
for remote UACs. Escape closes. Non-UAC Spaces rows retain Rename.
The display name overrides the header's terminal project label; clearing it restores
the label. Full IDs keep same-named repositories separate; old short-name manual order
migrates only when unambiguous.

The header's All/view selector replaces the sort menu. Only manual project drag order
remains; Priority and Grouped by space modes are removed. Dragging within a filtered
view preserves hidden projects' saved positions. Unmoved projects start in stable ID order.
Views live in the same TOML file, through `vega project view <name>
[--label <label>]... [--machine <machine>]... [--json]`; filter flags replace both dimensions.
`--delete` removes a saved view. The popup selects, edits, saves and deletes views.
The built-in `all` view is reserved and matches everything; a deleted selection falls
back to All. The active view name and existing drag order are this feature's localStorage UI state;
config and view definitions are never persisted in the mod's shared/local state.
A single CLI read loads both tables at startup and every five seconds; readers retain
their last good snapshot and show errors. Reopening an overlay reads fresh preferences.

Within one dimension, selected labels/machines are alternatives (OR); across dimensions,
both must match (AND). An empty dimension is unrestricted. Machine keys are hc names;
`local` denotes the app host. Machine filters select the UAC's machine for a project group,
keeping its sentinels and associated spaces together across machines. Those rows inherit
their UAC's labels, including children linked only by parent. Unassociated spaces use
their own machine and have no labels: All shows them, a label-filtered view hides them,
and a machine-only view shows those on a selected machine. Filtering also applies to
the collapsed rail and keyboard navigation.

## Data path

- **Per terminal:** vegad writes the tokens it reports to herdr today (`report_metadata`,
  `plugins/post/src/main.rs`) as hc labels on hc terminals: `hc label <t> vega.notes=1 vega.waiting=...`
  (`key=` removes). The mod reads them from `terminals()[i].labels`; `render()` runs again on every change,
  plus the inherited 30-second timer for duration displays.
- **Decisions and waiting panes:** the vegad badge file `$XDG_STATE_HOME/vega/badges/v2/<project>.json`,
  extended with the decision texts (bead id, title, question, project). The mod watches the folder with
  `watchFolder()`.
- **Answers:** `exec()` of the vega CLI, JSON where a command needs to return data.

## Retired TUI UI

The dashboard and path menu were dropped by decision vega-0h4c.11. Their plugin
crates and TUI config are removed; the inbox, sidebar and project window are the
supported UI. The old implementation is preserved at `herdr-tui-final`.

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
rows. The status line shows a short error only after three consecutive failures
for that source; a successful read resets its count (decision 7). Failed sources
retain their last good rows, whose original timestamps still expire after 60
seconds; reads never renew them. When `hc machine ls` fails or times out,
collection continues using the last cached machine inventory. Hosts need
synchronized clocks.

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

Project groups use full `vega.project-id` keys, with display names or short project
labels in headers. Legacy short labels join a UAC only when unambiguous.
A listed child appears only in its visible ancestor's tree,
including children with panes in another space. A child whose ancestor is absent
remains a top-level row. Project totals omit zero values.
Existing terminal pin labels do not change the manual project order. Listed
children remain under their visible parent regardless of those labels.

vegad first resolves the foreground cwd. If no project resolves, it uses that
terminal's space folder from the local core's `herdr-app` document (`doc.get`),
without asking the desktop app or inspecting remote paths; never for a terminal
whose foreground is its own launch shell (hc keep's recovery shell after a reboot).
A UAC claim's pane takes the claim's project over either. Project
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
Completed cards use the same machine/project/id keys as inputs and results, so
answering on one machine leaves another machine's identical bead ID visible.
Remote transport or inner-command failures keep the card open. Waiting panes
retain their qualified machine address, and positive group totals still match
both source machine and project.

For local worktree spawns, mux checks the configured app socket and reads the shown
space once. Without a local app binary, socket or window, it starts the ordinary hc
tab in the caller's space with cwd set to the worktree, preserving the brief, labels
and parent link, and prints one stdout placement line. `--machine` worktrees run wt
and trust preparation on the target; they always use the ordinary remote tab path,
without copying local app space IDs. An available local app keeps the new-space path.

`space.new` creates a shell before mux starts a worktree agent. It also shows the
new space, so mux reads the shown space (`herdr-app ls`) first and selects it again
afterwards: a spawn must not take the person's screen. When reading or reselecting
fails, mux warns on stderr and keeps the spawn; only the screen differs. New launches link
that exact placeholder and worker with ownership labels. `mux close` removes the
placeholder only when the space has no other live terminal and its foreground
has not changed to another program; normal app reconciliation then removes the
empty space. Old spaces without ownership labels stay untouched for user-led live
cleanup. No worktrunk hooks or badge schema changes are involved.

The Slayer kept decisions limited to projects with a claimed UAC (item 6:
decisions belong to the UAC) and kept the existing Git tooltip (item 8). The
toast-position upstream request is drafted only in the bead notes.
## Overview CLI data sources (vega-0h4c.50.4)

The Overview tab calls `exec("vega", [..., "--json"], {cwd, on})` in the selected
project checkout on its source machine. Each command emits one object with
`schema: 1`; errors use `{error: {code, message, hint}}` and nonzero exit codes.
TOON remains the default. Retain unavailable/unknown values rather than turn
them into zeros. No mod changes belong to this backend slice.

| argv before --json | Data | Scope |
| --- | --- | --- |
| `notify list --project <id>` | `asks`, `registry_error` | Open asks for exact project; no acknowledgement |
| `ci --project <id>` | `watches`: target, state, verdict, failed_checks, watchers, head, polled, error | Target host/repo; JSON defaults to cwd project |
| `capacity` | `windows`: provider/window/scope, left/used percentages, binding, reset/forecast seconds, age/stale, override, warns; source/ledger errors | Selected machine's accounts |
| `bd status --project <id>` | `projects`: location, state, last_pull/last_push, conflict, sync details | Provisioned local clones of exact project |
| `uac` | `agents`, receiving-host mail `grants`, `unreachable`, `registry_error` | Cwd project and group; no caller pane required |

Plugin READMEs document nullable fields and units. Lists are arrays even when
empty. IDs are full normalized project identities. Timestamps use Unix seconds,
durations seconds, percentages numbers. Capacity cover includes planned resets
and overrides. Capacity rows additionally have `account_id`, `credits` (fresh
flags, exact decimal balance, Unix observed_at; otherwise null), and `on_credits`.
A thin binding window on spendable credits keeps its quota percentages and sets
`on_credits: true`, `warns: false`. Missing/stale credit data stays unknown;
provider credit units must not be rendered as USD (vega-lweq).

## Inactive sidebar sessions (vega-0h4c.63)

The **Active** toggle beside the view selector hides idle (grey ring) and unknown/no-status
sessions. Working, blocked and done sessions stay: done is unread completion news, including
sub-agents. `kidStatus` preserves done as a blue dot like the tabs and maps error to blocked.
Done counts as unread only while the terminal's `turn` is past its `herdr.seen` label (what
herdr-app's tab bar uses); a seen done reads as idle (`seenDone`), for top-level rows and
sub-agents alike, in the glyph and in the filter. The
focused terminal's agent also stays, even when idle. This filter applies within the selected
label/machine view, and does not change that view.

A UAC stays when it is active itself or has any active live descendant. Only active descendants
are shown beneath it; active grandchildren are promoted past hidden idle parents, with tree
connectors rebuilt for the visible rows. Spaces without a UAC use the same per-agent rule.
Project and space groups with no remaining sessions disappear unless the project is pinned.
Worktree placeholder shells and exited descendants remain excluded. Folds still control expansion; folded rows do not
increase the inactive count.

Each project header has a small needle pin: dim when unpinned, theme accent when pinned.
Its click toggles the pin and stops propagation, as does its left mouse-down, so it does not
fold or drag the header. Pins live in vega project preferences (`projects.toml`, `pinned: true`),
not localStorage. The sidebar reads `vega project config --json`, writes
`vega project config <id> --pin|--unpin --json`, then refreshes; failures use the config error line.
With Active on, a pinned project's group and UAC remain even if idle or without status; idle
sub-agents and other idle agents still hide. Pins respect the current label/machine view and
do not retain projects outside it. Retained pinned UACs do not count in `N idle hidden`.

The toggle defaults off and uses shared state `vega.hide-inactive`, mirrored into each sidebar
copy's `localStorage` key `hide-inactive` as `true`/`false`, with that copy as the restart fallback,
like `vega.view`/`active-view`. A dim `N idle hidden` line counts sessions removed by activity
within the current view, including unknown sessions, excluding UACs retained as context and
the focused agent. Clicking the line disables the activity filter. It remains available in the
narrow rail too, where active descendants remain navigable.

## Project window, status line and sidebar badge (vega-0h4c.50, built live)

Each place has one job: the sidebar navigates, the status line shows the focused project's
context, the project window holds details and actions.

- **Project window** (`vega-sidebar`, overlay kind `window`): opened by right-click on a UAC
  project header, its settings icon, its summary badge, or a status line segment. No terminal
  block inside: a terminal rendered in the window and in its pane at once jitters; agent rows
  focus the terminal in the main view instead (`pane.focus`). The title bar is not
  propagation-stopped, so pressing it drags the window. A pin toggle switches between
  pinned (key `vega-project-<project id>`, one window per project) and follow (key
  `vega-project-follow`; each second the window reopens in place on the focused terminal's
  project, keeping the tab). The window owns a focus handle so Esc closes it.
  Tabs: Overview (UAC, needs-you tiles, beads counts, sentinels with beads, CI watches,
  capacity bars), Inbox (in progress / ready / blocked from `bd`, expandable rows),
  Decisions (answer via `vega uac answer`, dismiss via `bd close --reason`), Asks
  (`vega notify list --json --project <id>`, read locally for every project because the list
  comes from the registry; Ack and Resolve run `vega notify ack|resolve <host>=<id>`, which
  reach the origin host over NATS; Go to pane focuses the asking terminal), Notes
  (`vega post notes --json` for the project or the UAC pane; mark read / clear with `--upto`),
  UAC, Config. Remote projects run `bd` and `vega` through `hc exec` on their machine, as
  vega-inbox does. Reads poll every 15 s and redraw only when the data changed.
- **Status line** (`vega` mod): after machine, branch and agent count, segments for the focused
  terminal's project: name with decisions, asks and notes; beads in progress and ready; CI;
  the capacity of the focused agent's provider (claude, codex, zai for pi); beads sync. Each
  segment opens the project window on its tab. Local projects only for asks, beads and CI.
- **Sidebar**: one summary badge per project (decisions + waiting + notes; blocked colour when
  a decision or a waiting agent is in it). A click opens the most urgent view: Decisions, the
  waiting pane, else Notes. Agent rows keep per-agent signals only (mail, waiting, git).
- **Labels and views**: the Config tab edits a project's labels as pills (Enter adds, the
  pill's ✕ removes, both save at once through `vega project config`). Sidebar project headers
  show up to two label pills; a click sets the ad-hoc view `label:<name>`, a second click goes
  back to All. A saved view matches `all` (a chosen label and a chosen machine; empty
  dimensions match everything) or `any` (a chosen label or a chosen machine: everything on puck
  plus label vega), set by `vega project view --match` and the popup toggle. The active view lives in shared state `vega.view`, because `localStorage` is per
  mod copy and the filter popup is another copy; the sidebar mirrors it into its own
  `localStorage` (key `active-view`) to keep it across restarts. Saving a view shows it.
- **Waiting on you**: the Overview lists the badge row's `waiting_panes` under the tiles with
  Go (focus) and, for `silent` and `asking`, Dismiss; the vegan inbox offers the same Dismiss.
  Dismiss runs `vega mux dismiss <pane>` on the badge row's machine (the parent's vegad host,
  through `hc exec` when remote, pane unqualified to `hc:<pane>`). A live dialog (`input`) and an
  open ask (`unanswered`) are not dismissable (vega-0h4c.56).
- **Inputs in the project window** sit in `inputBox`, which stops the left press: the window
  refocuses its own handle on every press (for Esc) and would take the focus from the input.
- Colours come from theme tokens only (working, blocked, done, idle), for light and dark.

## Models tab (vega-aysa.2)

The project/config window's descriptor list in `mods/vega-sidebar/main.js` includes
Models; `mods/vega/main.js` opens that same window. Models is fleet-wide, read and
edited on the app host. It consumes `vega lanista hosts --json` (`{rows: [...]}`),
without filtering by connected hc machines: inbox-only hosts such as puck remain
visible. Each host group lists seat, lane and tier, effective model, source
(project/host/default/global) and expiry. A project source can continue to mask a
successful host/default edit; the refreshed source makes that explicit.

The Default group is independent of host/project overrides: one read-only
`vega lanista pin <lane> --tier <tier> --default` per unique seat/lane/tier reads
its model, source and expiry. That command currently prints named text fields.
Pi is omitted from Default because the CLI rejects pi default writes; use zai's
default cells for its fallback. Host pi cells still offer only their own allowlist,
even when the displayed effective model is inherited from zai.

Choose model opens an inline dropdown containing only the row's `allowed` models.
Selection runs `vega lanista pin <lane> --tier <tier> --host <host> --set <model>`;
Reset substitutes `--clear`. Default actions substitute `--default` for `--host`.
Nonstandard seats add `--seat <seat>`. Commands use argv without a shell. Controls
stop left mouse-down like `inputBox`, preserving the window's focus handling.
All colours use theme tokens. CLI stdout errors, stderr errors, launch failures,
and parse failures appear inline; failed host reads retain the last good snapshot.

The snapshot and mutation guard use `shared()`/`setShared()` so separate mod copies
see the latest values and cannot submit simultaneous edits. Reads run when Models
is selected, every 15 seconds while selected, on Refresh, and after each action.
`node --test scripts/herdr-app-mods.test.mjs` covers rendering, independent defaults,
puck, allowlists, exact set/clear argv, pi omission, refresh and inline errors.
