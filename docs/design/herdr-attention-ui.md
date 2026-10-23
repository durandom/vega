# Herdr attention UI

Design study for vega-lhc.170, 2026-10-21. Recommendation: a global attention popup,
with a persistent tab-bar summary; add opt-in notifications after the list is reliable.
This document proposes implementation; it does not change herdr, configuration, or binaries.

## Evidence and scope

Read-only source snapshots under `~/src/durandom/vega/repos/`:

| Prefix below | Checkout | Revision |
|---|---|---|
| H | `herdr/` | `143bf5fd4781d164260c63fe28e7dc08054ad32f` |
| B | `herdr-beads/` | `72f59e1318875462b4f9d3f0073b09a9d8b1c045` |
| Q | `herdr-quicklook/` | `b6567740aaceeabf8fd31c002c4936bfbf1523e6` |

H file references describe that checkout, not a promise about every installed version.
H's `docs/next/website/src/content/docs/{plugins,socket-api,configuration}.mdx`
corroborate the source contracts below. No network or upstream contact is needed for this study.
Vega references are relative to this repository. The existing UX is specified in
[board-ui.md](board-ui.md) and [theme-symbols.md](theme-symbols.md).

The problem is discoverability across projects and machines: the hand and scale in a
project's sidebar label are small, and opening each project's board to inspect them costs
context switches. The board already has an Inbox, waiting-pane jump, and decision-answer
prompt. Its `repo|global` chip is a `bd --global` bead scope, not a complete cross-host
attention view: `plugins/board/src/{badges,inbox,main}.rs` still use the current project's
badge file and one tracker. Do not advertise that chip as the solution.

## Available surfaces and hooks

| Surface | What works today | Source and limits |
|---|---|---|
| Addressable terminal overlay | A manifest pane or `plugin.pane.open` with `placement=overlay` runs a real TTY over the active pane, restoring focus/zoom on exit. It has a pane ID and can use normal pane APIs. | H `src/app/api/plugins/panes.rs::open_plugin_overlay_pane`, `src/api/schema/plugins.rs::PluginPanePlacement`; Q `herdr-plugin.toml` and `scripts/{hint,find,recents,open-popup}.sh`. It is a terminal surface, not a native-widget extension. Q documents overlay nesting/z-order and query-RPC hazards; fetch context before opening and keep network work outside input/draw paths. |
| Session-modal terminal popup | `placement=popup`, width/height in cells or percentages; layout stays intact, app receives Escape, exits to close or calls `popup.close`. | H `src/app/{popup.rs,api/plugins/panes.rs}`. Popup is a singleton with no pane ID, no pane lifecycle events, and no `HERDR_PANE_ID`; `pane current` refers to the underlying pane. `ui_busy` is possible; never rename/close that underlying pane as popup cleanup. B `scripts/open-board.sh` opens 85% × 85%; vega mirrors it in `plugins/board/scripts/open-board.sh`. |
| Dock, normal tab, zoomed pane | Plugin placements `split`, `tab`, `zoomed`; normal addressable terminal panes. Suitable for a durable attention dashboard or narrow dock. | H `src/app/api/plugins/panes.rs::{open_plugin_split_pane,open_plugin_tab}`. B `scripts/{open-dock,auto-dock}.sh` implements toggle/split/swap and opt-in `tab.created` auto-docking; B `src/app_live.rs` loads data in the background. Costs screen space. |
| Notifications/toasts and sounds | `notification.show` / `herdr notification show`: title, optional body, position and `none|done|request` sound; delivery is configured as herdr, terminal, system or off. | H `src/api/schema/common.rs::NotificationShowParams`, `src/cli/notification_bindings.rs`, `src/server/headless/notifications.rs::handle_notification_show_api`, `src/client/shell/{notifications,notification_policy}.rs`, `src/terminal_notify.rs`. Text is sanitized/capped at 80/240 chars. Results include shown, disabled, busy, rate_limited, no_foreground_client; acceptance by the server is not proof the human saw it. Public custom notifications carry **no pane target or action callback**. Built-in agent toasts can focus their endpoint/pane, but a plugin custom toast cannot inherit that behavior. |
| Goto / session navigator | `keys.goto` defaults to `prefix+g`; client aggregates connected endpoints, workspaces, tabs and panes. Search and blocked/working/idle/done filters; native selection can switch machines. | H `src/config/model.rs`, `src/client/shell/{state,aggregate_navigation,endpoint_navigation,overlays,overlay_input}.rs`; `aggregate_navigation.rs::navigator_rows`, `overlays.rs::render_navigator_overlay`. No plugin row provider, decision object, or public open-with-query/filter API in plugin v1. Herdr blocked/done is not vega's headed-for-user classification. |
| Keybinding help and menus | `[[keys.command]]` with `type=plugin_action`, a manifest action ID, and description; custom descriptions appear in filterable key help. Actions receive workspace/tab/pane/cwd context. | H `src/{input/keybind_help.rs,config/keybinds.rs,cli/plugin/bindings.rs}`, `src/client/shell/{global_menu,context_menu}.rs`, `src/app/api/plugins/context.rs`. Existing Q actions and B open-board/open-dock actions illustrate this. Native menus are built in; manifest actions do not provide arbitrary native menu rows or dynamic key-help counts. Use a descriptive binding as the discoverable entry point. |
| Sidebar agent/workspace rows | `pane.report_metadata`, `workspace.report_metadata`: TTL-owned custom tokens and styled rows; `agent.view.set/clear` transiently filters/sorts the built-in Agents view, including the combined machines view. | H `src/api/schema/` metadata and agent-view types, `src/agent_view_eval.rs`, `src/client/shell/aggregate_navigation.rs::aggregate_agent_rows`. Vega `crates/vega-herdr/src/badges.rs::Badges::uac_tokens`, `plugins/post/src/main.rs::report_metadata`; [theme-symbols.md](theme-symbols.md). One active agent projection replaces another; clear only your source. Agent views affect agent navigation, not API lists, notifications or global counts. Decision rows cannot be inserted as agents. |
| Tab-bar right status | Configurable text/hostname/time/zoom and periodically executed shell commands; can read an already aggregated count file. | H `src/config/tab_bar.rs::TabBarRightEntryConfig`, `src/app/tab_bar_status.rs`, configuration reference `ui.tab_bar_right`. Defaults: 5 s interval, 2 s timeout; up to 16 entries, first command line capped to 80 displayed chars. Status yields to tabs on narrow terminals. Schema has no click-action or per-segment plugin styling callback. |
| Bottom/status chrome and window title | Built-in status/feedback, plus `client.window_title.set/clear` for the foreground terminal title. A plugin owns its own TUI footer. | H `src/ui/status.rs`, `src/api/schema/` client title methods; no general plugin bottom-bar widget API. Overriding the window title takes ownership from configured titles and is a weak primary attention surface. |
| Plugin/event/socket infrastructure | Manifest argv actions, startup hooks, event hooks, panes and link handlers. `HERDR_BIN_PATH`, socket path, context/event JSON and plugin state/config dirs are injected. CLI or newline JSON socket requests can inspect/focus panes and subscribe to events. | H `src/app/api/plugins/{runtime,env,context}.rs`, `src/api/schema/{plugins,events}.rs`. `PLUGIN_HOOK_EVENT_KINDS` covers workspace/worktree/tab/pane lifecycle and agent detection/status; metadata events and high-volume output events are not manifest hooks. `events.subscribe` has broader events but bounded non-durable history: subscribe, reconcile snapshots, recover after loss. Startup hooks are one-shot initialization, not supervised daemons. Q `scripts/agent-suggest.sh` is an opt-in status hook which invokes `notification show`; Q link handlers route Ctrl-clicked sentinel URLs to manifest actions. No native non-terminal plugin widgets or runtime action registration. |

All commands above refer to controlled CLI/socket operations. Herdr client panes forward
keystrokes to other machines: never type discovery commands, acknowledgements or synthetic
answers into client panes. This study used filesystem reads only for herdr inspection.

## Data and routing constraints

- `crates/vega-herdr/src/badges.rs` saves per-project counts plus `waiting_panes`
  (`pane,name,attention,title,at` envelope), valid for 60 s. It does **not** save decision
  rows, decision age or waiting-start times. Filename normalization is not a project
  identity; preserve the stored project key.
- `plugins/post/src/main.rs::{report_metadata,DecisionCache::list}` already gathers headed
  panes needing the user (`st::for_user`) and open decisions from the single open
  `uac-tracker` epic per project. `crates/vega-herdr/src/badges.rs::decisions` supplies
  ID/title/created time. The decision cache retains the old list on read failure; a new
  projection must track the last **successful** read independently to avoid making it
  look newly verified on every label refresh. Enumerate provisioned projects as well as
  live UAC/board projects; lack of a live UAC is a routing state, not zero decisions.
- `crates/vega-herdr/src/waiting.rs` and `crates/vega-registry/src/lib.rs::View::waiting`
  expose a done/blocked ledger for `vega mux next`. That ledger is broader/different
  from the hand badge (headed asking/silent/input for the user). Reuse identity/freshness
  infrastructure, not that predicate as an interchangeable waiting list.
- Registry UAC/agent records, heartbeat freshness and notes are available in
  `crates/vega-registry/src/lib.rs`; the full decision list and badge waiting list are
  not published today. `plugins/post/src/notes.rs` demonstrates local-plus-registry
  collection with bounded waits and local fallback. Inbox-only/unavailable hosts cannot
  be assumed visible through the registry. Remote paths are never local filesystem paths.
- `crates/vega-herdr/src/hosts.rs` maps saved herdr aliases to registry hosts without
  guessing. Carry `(host,project,kind,id)` everywhere; never resolve a remote bare pane
  ID against the local server first. The same pane ID can exist on several hosts.
- `crates/vega-herdr/src/pane.rs` and `plugins/board/src/jump.rs` focus local panes today;
  remote focus changes the remote server's selection and tells the human to switch
  machines. It does not switch the attached client. Native goto and targeted built-in
  notifications do switch endpoints internally. An externally callable endpoint+pane
  jump would require upstream; vega-8m7 tracks related last-location navigation, not
  an already available jump API.
- `plugins/board/src/{inbox,uac,input}.rs` already offer `m` to submit a decision answer
  through `vega uac answer`. This depends on the displayed repo's cwd. A global list
  needs explicit project/host routing and must revalidate the decision before submitting.
  Answering posts the Slayer comment and mails the UAC; it does not close the decision.
- `plugins/post/src/hygiene.rs` already persists waiting transitions and sends UAC mail
  after 24 h, with daily stale lists (decisions: 7 d). This is different from direct human
  notification at 24 h. `crates/vega-herdr/src/notes.rs` preserves notifications after a
  transient banner; use its cause/resolution concept instead of a second notification log.

## Concrete options

Effort is an engineering estimate including tests and config/docs, in focused developer
workdays; it excludes upstream turnaround. These are alternatives/complements, not claims
that any feature is already implemented in vega.

### 1. Global attention popup on one key — recommended primary surface

A new board entrypoint opens an 85% popup from any project. Default scope is all known
projects/hosts; waiting rows first, then oldest decisions. Show host, project, type, age,
title and stable target. Search `/`, kind filters, `j/k`, `Enter`, `m`, `r`, `q/Esc`.
Symbols/colors reuse `theme.toml`; text labels remain readable without a Nerd Font.

```text
 Attention   Waiting 2  Decisions 3                   all projects · all hosts
 W  proteus  vega    asking   build-agent   asks: retry the job?     w3V:p4
 W  deimos   other   input    review-agent  permission requested     w1:p2
 D  proteus  vega       26h   vega-123      Which release window?
 D  io       other      14m   other-7       Keep the old API?
 deimos: stale (last success 3m ago) · 1 host unavailable
 Enter jump/detail · m answer decision · / search · r refresh · Esc close
```

Use a new versioned vegad attention snapshot per host/project, atomically written locally
and published through the existing registry transport. Include item identity, timestamps,
waiting state generation, tracker validity, project route, successful observation time and
source errors. Merge replicated decisions by canonical project/database identity + bead ID,
retaining host routes; do not collapse unrelated repositories with equal IDs or display
names. Collapse waiting panes only by host + pane, even if worktrees share project labels.

Rendering reads an in-memory model; background loading has bounded timeouts and preserves
selection by stable identity. Snapshots older than 60 s are stale: exclude from actionable
counts, show a stale section and unavailable-host summary. Stale/unknown/invalid-tracker
states cannot render as “nothing needs attention”. The scope means known registered or
locally provisioned sources, not every host that might exist. A host without registry
rights stays explicitly unavailable remotely; no automatic SSH crawl or cross-host path use.

Enter on local waiting: close the popup, then focus the exact pane (avoid popup restoration
undoing focus). Remote waiting: prepare focus on the exact host, close and report “switch
to <machine> in herdr”; keep this limitation visible before activation. A native one-action
remote jump is deferred. Enter on a decision shows detail in this same surface without
stacking a second popup. Initial answer support can open the routed project's existing
board; a later `m` prompt inside the popup shows host/project/bead and submits to that exact
route. Remote answering needs a validated vegad command path, not local `chdir` to a remote
path. Disable unsupported/unknown routes with a reason. Never send text to a terminal pane
as an answer transport. Keep drafts on submission failure, report comment-versus-mail
partial success, and avoid duplicate comments on retry. No automatic decision closure or
permission/trust approval through this UI.

**Today:** all terminal UI, binding, local focus and local answer primitives exist.
**Vega work:** snapshots/publication, global collector, popup mode, routing, lifecycle and
answer-result handling. **Upstream:** none for the honest fallback; endpoint+pane focus API
for seamless remote jumps. **Effort:** 5–8 days read-only list/jump, plus 2–4 for routed inline
answers. Prefer reusing board components over forking the full board TUI.

### 2. Human notifications for new waits and decisions older than 24 hours

Opt-in per viewing host: a short banner “vega: 1 pane needs you” or “2 decisions older
than 24 h — open Attention”, with the configured binding in the body. Never steal focus.
Notify a waiting episode once after a short debounce; notify once when a decision crosses
24 h. Persist episode/threshold keys across restart; first activation seeds existing waits
without a burst and coalesces existing overdue decisions into one summary. Persist pending
versus accepted delivery separately. Coalesce busy/rate-limited results with bounded backoff;
disabled delivery stays disabled. Offline time must not create a notification storm.

Have only the opted-in viewing host deliver the aggregate, avoiding one banner per replica
or attached client. Source transitions remain authoritative on their host. Reuse durable
notes with project/bead/pane/cause, resolve when the item leaves attention, and deduplicate
against the attention list so a reminder does not double-count its decision. Existing
UAC hygiene thresholds remain unchanged. Account for herdr's own blocked notifications;
allow waiting banners to be disabled independently and default sounds to none.

**Today:** custom banners, configured delivery, durable notes and UAC hygiene bookkeeping.
A custom toast is informational: it cannot jump or launch the attention popup on click.
**Vega work:** transition/age tracking, reminder policy, persistence, aggregation and note
resolution. **Upstream:** optional target/action field for clickable custom notifications;
not needed for informational delivery. **Effort:** 2–4 days after snapshots.

### 3. Persistent global count in the tab bar; optional waiting sidebar view

Add an opt-in `vega config` herdr status-command fragment which prints cached global
“Waiting 2 · Decisions 3 · Attention <key>”. The command reads the aggregate snapshot only,
finishes within herdr's timeout and shows “attention stale” when sources are incomplete.
Keep it short for narrow terminals and provide a compact form. Do not assume tab-bar
counts stay visible at all widths or that they can be clicked.

A complementary manifest action could set a source-owned `agent.view` matching vega's
`$attention` tokens and sort waiting agents first. It gives cross-machine native agent
navigation, but lacks decision rows and depends on tokens from each host. Clear only the
owned view on toggle-off; explain that it replaces any active custom view. Avoid making
this the default: filtering out working agents changes navigation beyond this task.

**Today:** command status, custom tokens and agent projection exist in the inspected H
checkout. **Vega work:** fast cached summary and config integration; token-view action is
optional. **Upstream:** clickable/styled status segments would require an extension.
**Effort:** 1–2 days for the summary after snapshots; 1–2 more for the optional agent view.

### 4. Native Attention section inside goto

Extend herdr's client navigator with plugin-supplied rows: waiting panes across endpoints
and decision rows with detail/answer actions. Reuse native search, mouse navigation and
endpoint switching; a single builtin experience could eventually replace the separate popup.

**Today:** goto can find/filter agents and switch connected machines; token-based agent
views can improve the sidebar, but cannot add decision rows to goto. **Upstream:** a native
row/action-provider contract, stable endpoint-aware targets, focus/refresh/error handling,
and decision detail/input hosting (or a callback into a plugin terminal). No such extension
exists in plugin v1. **Vega work:** provider/snapshot adapter after that contract exists.
**Effort:** roughly 1–2 weeks of herdr work plus 2–4 vega days, with uncertain acceptance and
release timing. Do not make the first useful improvement depend on this. No upstream
request is sent by this task.

## Recommendation and delivery

Build option 1 with honest host-switch guidance, then the tab-bar summary from option 3.
The list makes every actionable item reviewable; the persistent count makes the list easier
to remember. Add option 2 as an explicit preference once counts and deduplication agree.
Defer the agent-view toggle, native goto extension and clickable custom toast until usage
shows they are needed. Routed inline answers are a separate increment after the read-only
list; local project-board answers remain available in the meantime.

Verification for implementation: fixtures with two projects, duplicate IDs across hosts,
replicated decisions, missing/multiple trackers, stale/failed sources and no live UAC;
waiting-vs-ledger classification; jump-after-popup-close and remote guidance; restart-safe
reminder deduplication and disabled/busy delivery; exact answer route, partial success and
retry. Exercise popup keyboard/mouse/resize behavior in a controlled local test session,
never by typing into the Slayer's herdr client. Run `mise run check` and the vega gate for
each implementation increment. This design-only task runs no build.

Build steps are tracked as children of vega-lhc.170 (IDs below); proposed flags, snapshot
schema and registry names must be finalized in those tasks rather than treated as existing
interfaces.

- vega-lhc.170.2: Publish fresh per-project attention snapshots across hosts.
- vega-lhc.170.3: Open a global read-only attention popup with exact pane routing.
- vega-lhc.170.4: Expose cached global attention counts in herdr tab-bar configuration.
- vega-lhc.170.5: Answer decisions from the global attention popup with validated routes.
- vega-lhc.170.6: Add opt-in coalesced human attention reminders.
