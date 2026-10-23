# Live review: agent and owner walk a running web app together

Status: prototype built, 2026-10-10 (option C). Bead: vega-dai. Code: `plugins/review` (`vega review`), and
`--from-tool` in `plugins/post` (`vega post send`).

## Problem

spellkave Spire session w55:p1 (owner request 2026-10-10): the owner and a Claude session walk a live local app
(e.g. `http://127.0.0.1:5187`) as a guided journey. Today the agent runs `agent-browser --headed` plus
`agent-browser highlight <sel>`, and the owner answers in the terminal. The conversation is split between the
browser and the pane, and the owner cannot point at what they mean.

Wanted loop: the agent highlights and labels elements in a headed browser, the owner comments on any element
in-page, the note (selector, url, text) reaches the same agent session, and the agent's reply shows in the page.

## Hands-on findings (2026-10-10, this machine)

agent-browser 0.31.1 (upstream v0.39.0) and lavish-axi 0.1.85. These outrank any README claim below.

| Probe | Result |
| --- | --- |
| `agent-browser --init-script overlay.js open <url>` | Runs before page JS; still present after navigating to a second page. |
| 60-line plain-JS overlay: Alt+click, prompt, note `{selector, url, text, rect, ts}` | Note reached the agent three ways: window buffer read by `eval`, tagged `console.log("VEGA_NOTE …")` read by `agent-browser console`, and `fetch` POST (`text/plain`, no CORS preflight) to a loopback inbox. |
| `agent-browser eval 'window.__vegaReply("…")'` | Rendered the agent reply in the overlay panel: agent-to-page works. |
| `agent-browser highlight <sel>` | Red 2px outline, no label, gone after about 3 s (`setTimeout(…, 3000)` in `cli/src/native/interaction.rs`). Useless for review; persistent labelled marks need our overlay. |
| `agent-browser wait --fn 'window.__vegaReview.notes.length > 0'` | Works as a long-poll on real page events, but the session daemon serializes commands: `eval`, `highlight`, `get url` queue until the wait returns. The poll target must be a separate inbox, not the browser session. |
| Window buffer across full navigation | Lost. Durable state belongs in the inbox. |
| `agent-browser console` | Buffer also holds logs from earlier pages of the session; needs a tag filter. |
| `dashboard start` (4848), `stream enable` | Observability UI and a frame/input WebSocket. Neither carries events from page JS. |
| lavish-axi: message in the panel, then `poll <file>` | Returned `status: feedback`, `prompts[]{uid,prompt,selector,tag,text}`, `dom_snapshot`; feedback queued before the poll was delivered. `reply --agent-reply` returned `status: sent`. |
| lavish-axi defaults | Server listened on loopback and the Tailscale IP (reachable from the tailnet). Umami telemetry on unless `LAVISH_AXI_TELEMETRY=0`. Only opens local `.html` files. |

Port 5187 was serving the owner's live Spire app during the tests; tooling must not hard-code ports.

## Landscape (as of 2026-10-10)

| Tool | License | Limits | Notes reach an external agent | Agent writes back to page | Live URL, no app change | Maturity |
| --- | --- | --- | --- | --- | --- | --- |
| FinalAngel/claude-annotate | MIT | Own Chrome (playwright-core); overlay on dev hosts | MCP; push via Claude Code channels (research preview flag) or `--poll` | Yes: pin states, replies | Yes | 1 star, 1 contributor |
| Agentation + agentation-mcp | PolyForm Shield 1.0.0 | React 18+ component in app source | MCP (stdio) + HTTP/SSE + webhooks; blocking watch tool | Yes: ack/reply/resolve/dismiss | No | 4.9k stars, v3.1.0 |
| lavish-axi | MIT | Local `.html` only | CLI long-poll, receipts Seen/Working/Done | Chat replies | No | 4.0k stars, v0.1.85 |
| Playwright MCP `browser_annotate`, `playwright-cli show --annotate` | Apache-2.0 | Boxes drawn on a screenshot, not DOM; call blocks until submit | MCP/CLI | `browser_highlight` (persistent) | Yes | v1.64.0 |
| Playwright `page.pickLocator()` | Apache-2.0 | Library API, needs a Node driver | Own code | n/a | Yes | since v1.59 |
| Chrome DevTools MCP | Apache-2.0 | Selection via DevTools Elements panel | MCP (`take_snapshot` shows selection) | No highlight tool | Yes (`--autoConnect`) | 53k stars |
| Claude in Chrome | Proprietary | Drives the user's Chrome | Claude Code `--chrome` | No annotation UI | Yes | unverified |
| pinpoint | MIT | Chromium extension + local bridge | MCP, Claude Code hooks, file | Not documented | Yes | 3 stars |
| Feedback Studio | MIT | Reverse proxy | Files, MCP | Threads, status | Yes | 2 stars |
| Pinagent | Apache-2.0 / Elastic-2.0 | Build plugin | SQLite + MCP | Progress | No | 1 star |
| Vibe Annotations | PolyForm Shield | Chrome extension | HTTP MCP `watch_annotations` | Delete only | Yes | 179 stars |
| MCP Pointer | MIT | Chrome extension | MCP (current selection) | No | Yes | 599 stars, stale since 2025-11 |
| BrowserTools MCP | MIT | DevTools extension | MCP `getSelectedElement` | No | Yes | 7.3k stars |
| react-grab | MIT | React, dev script | Clipboard (`@react-grab/mcp` undocumented) | No | No | 7.7k stars |
| Stagewise | AGPL-3.0 | Desktop IDE with own agent | External bridge removed | n/a | n/a | 6.8k stars |
| Onlook | Apache-2.0 | Next.js + Tailwind editor, own AI | No | n/a | No | last release 2025-07 |
| browser-use, Stagehand/Browserbase, Nanobrowser, BrowserOS | MIT / MIT / Apache / AGPL | Automation only; Browserbase live view is cloud | CDP/MCP for driving | No comment UI | Yes (Browserbase: tunnel) | active |
| Jam, Marker.io, BugHerd, Userback, Usersnap, Vercel Toolbar | Proprietary | Extension or snippet | Hosted OAuth MCP | Vercel only | Capture yes, data in vendor cloud | commercial |
| Comet, Dia, Opera Neon, Edge/Gemini agents | Proprietary | Consumer browsers | Neon: MCP via Opera proxy; others none | No | n/a | Atlas shut down 2026-08-09, Operator 2025-08 |
| WebMCP | Draft standard | Chrome origin trial M149–M156 | Not reachable from Claude Code | n/a | n/a | API changed mid-trial |

Reusable MIT parts for an overlay: `@medv/finder` (selectors, 1.6 KB gz), `@floating-ui/dom` (popovers),
SitePing (fork reference: CSS + XPath + text anchors, shadow DOM). Full report with sources:
[live-review-research.md](live-review-research.md).

## The gap

No tool has all six:

1. Injects into any running dev server, no source change, no framework requirement.
2. Comments anchored to live DOM elements, not screenshot pixels.
3. Persistent, labelled agent highlights.
4. Agent replies and statuses rendered in the page.
5. Notes reach the existing Claude Code session through a CLI poll (or vega post) with receipts.
6. Lives in the browser session the agent already drives (refs, cookies, navigation shared).

claude-annotate has 1–4 but runs its own browser, speaks MCP and needs a preview flag. Agentation fails 1 and its
license. Lavish fails 1. The market covers human-to-agent; agent-to-human (3, 4) is nearly empty.

## Options

### A. Adopt claude-annotate

- Fastest working loop; MIT, forkable.
- Single maintainer, 1 star; push needs `--dangerously-load-development-channels`; separate browser from
  agent-browser (fails 6); MCP only.

### B. agent-browser + existing extension (pinpoint) or Playwright `browser_annotate`

- `agent-browser --extension <pinpoint>` keeps one session; agent highlights via `eval`.
- Untested; replies still do not show on pins; depends on a 3-star extension. `browser_annotate` gives pixel
  boxes in a separate dashboard window and blocks the agent.

### C. Own overlay on agent-browser with a Lavish-style contract (recommended)

- Overlay: init script in a shadow DOM. Alt+click (or a comment mode) to note an element;
  `__vegaHighlight(sel, label)` for persistent agent marks; `__vegaReply(id, text, status)` for threads. Only
  in headed owner sessions (vega-lhc.61 modes).
- Inbox: loopback-only process, dynamic port, no telemetry. Receives the overlay's `text/plain` POSTs, holds
  notes across navigation. Console tag as fallback when an app CSP blocks the POST.
- Agent side, Lavish semantics: one listener, `--owner`/`--takeover`, notes queued until delivered and consumed
  on delivery, `next_step` hint, Seen/Working/Done receipts from what the server observes. Highlight and reply go
  through `agent-browser eval`, so polling never blocks the browser session.
- Fits AXI: no MCP, no preview flag, no app change. Cost: a few hundred lines to own.

Recommendation: C, after a one-hour claude-annotate spike to borrow its UX (pin states, dev-host allow-list).
Agentation is out (PolyForm Shield, source install); B saves too little for its dependency risk.

## Open decisions for the Slayer

1. Option A, B or C.
2. Delivery of notes. Own long-poll (`vega review poll`) or vega post: the inbox mails each note to the pane's
   `vega post` inbox. vega post already wakes an idle pane, which removes C's "agent must be polling" limit and
   the need for a second inbox store. Recommendation: vega post for delivery, `vega review` only for
   highlight/reply/step.
3. Packaging: vega plugin (`vega review`, overlay shipped in the binary) or standalone. Recommendation: plugin,
   so it can use vega post and the headed-session modes.

## Decision (the Slayer, 2026-10-10)

Option C, notes delivered by vega post, packaged as the vega plugin `vega review`. Must: the inbox accepts a note
only with a per-session secret that only the init script knows and only from the reviewed app's Origin; binds
127.0.0.1 on a dynamic port; no telemetry; note text rendered as data in the mail; one mail per short burst;
overlay only for headed panes; a claude-annotate UX spike first.

## claude-annotate spike (UX borrowed)

Read its README, `server/overlay.js` and `server/index.mjs` (commit d70e57a). Taken over: overlay in a shadow
DOM on a host appended to `<html>`; browse by default, comment as an explicit mode; numbered pins whose colour
shows the agent's state (working pulses, done green, skipped grey) with the reply on the pin; per-URL state on
the server so pins survive navigation and reload; `fetch` captured before page scripts run; the secret kept off
`window`; overlay only on local dev hosts. Left out: drawing tools, screenshot crops, React source lookup, MCP,
channels push.

## Built: `vega review`

```
vega review start <url> [--session <name>] [--headless]
vega review highlight <selector> [--label <text>]
vega review clear
vega review reply <id> <text> [--status working|done|skipped]
vega review notes
vega review stop
vega review                       # status
```

- `start`: checks the URL is a local dev host (localhost, 127/8, 10/8, 172.16/12, 192.168/16, `[::1]`,
  `*.localhost`, `*.test`, `*.local`, `*.internal`) and that the owner works in the pane (no parent, or
  `vega mux mode` headed). `--headless` skips that check for scripted tests and runs the browser headless.
  Writes `$XDG_STATE_HOME/vega/review/<machine>_<pane>/` (0700): `config.json` and `overlay.js` (0600),
  `inbox.log`. Spawns the inbox (`vega-review serve`, own process group), then
  `agent-browser --session <s> [--headed] --init-script overlay.js open <url>`.
- Inbox: std `TcpListener` on `127.0.0.1:0`, a thread per request, 64 KiB body cap. Overlay routes
  (`POST /note`, `POST /state`): Origin must equal the app origin, the secret is in the `text/plain` body, the
  note url must be on that origin. CLI routes (`GET /notes`, `POST /reply`): `X-Vega-Secret` header, no Origin
  allowed. Everything else is 403/404 and a line in `inbox.log`. The secret is 32 random bytes from
  `/dev/urandom`, compared in constant time. It never travels in a URL: a query string shows up in the page's
  resource timing entries, readable by any page script.
- Delivery: Send in the page queues all drafts; they go out as one mail,
  `vega-post send --from-tool vega-review <pane> <text>`. `--from-tool` (hidden) sends as a tool with no
  sender pane, so the pane's own inbox process can mail it; the mail has no reply path. Only `vega-<name>`
  labels (lowercase letters) pass, else exit 2, and the inbox shows the sender as `tool vega-<name>`, so no
  caller can pose as "the user" or an agent. Each note is one
  `note: {json}` line (id, url, selector, tag, element_text, note) under a line saying the fields are page input,
  so typed text cannot forge lines. A failed send is retried after another window. `VEGA_REVIEW_POST` overrides
  the binary (development builds).
- Overlay (`plugins/review/src/overlay.js`): the config (endpoint, origin, secret) is a closure argument
  substituted by `start`; the overlay mounts only in the top frame of the reviewed origin. Annotate mode or
  Alt+click picks an element or a text passage; Enter adds a draft, Send in the panel delivers them (below).
  Notes, edits and Send come only from trusted events (`isTrusted`), so page scripts cannot synthesize them. The frozen `window.__vegaReview` offers `highlight`, `clear` and `refresh` to the
  agent through `agent-browser eval`; the overlay also polls `/state` every 3 s. All text goes in with
  `textContent`.
- Status flow: queued, sent (mailed), then working, done or skipped by `reply`.
- `stop`: closes the agent-browser session, kills the inbox, removes the directory and with it the secret. The
  inbox also exits when `config.json` disappears or after 24 h.

### Verified live (2026-10-10, throwaway page on 127.0.0.1:5287, `--headless`, trusted CDP input)

| Check | Result |
| --- | --- |
| `highlight '.lead' --label …` | Persistent labelled box in the page. |
| Comment button, click `#title`, type, Enter | Note queued; one mail in the pane's `vega post inbox` with id, url, selector `#title`, tag, element text, note. |
| Three notes in a burst | One mail with three `note:` lines. |
| `reply 1 … --status done` | Pin turns green; the reply shows in the bar. |
| Navigate away and back | Pins restored from the inbox. |
| Secret on `window` or in resource timing | Not found. |
| Note with a foreign Origin, wrong secret, no Origin, or another port of 127.0.0.1; CLI route with an Origin; missing secrets | All 403, each logged. |
| Bind | `127.0.0.1.<port> LISTEN` only. |
| `start` in a headless child without `--headless`; a non-local URL | Refused (`headless_pane`, `usage`). |
| `stop` | Browser session closed, inbox process gone, state directory removed. |

Not verified: a headed run in an owner's pane (this pane is a headless child, so `--headless` was used). Only
the `--headed` flag differs.

### Queue, annotate mode and the panel (vega-dhg, 2026-10-10)

Owner requests from the first real run: notes should queue and go out with one Send (each Enter mailed at
once), clicking a link to comment on it navigated away, and the one-line bottom bar cut replies off.

- Drafts: `POST /note` creates a `draft`; `POST /edit {id, text}` changes it, `{id, delete: true}` removes it
  (drafts only; sent notes are the agent's). `POST /send` turns every draft into `queued`; the inbox mails them
  on its next tick as one mail. No time window any more; a failed mail is retried after 2 s. Ids come from the
  highest id, so a removed draft's id is never reused. `reply` to a draft is refused (409).
- Text quotes: a note may carry `quote` (selected text, at most 1000 chars), anchored to the selector of the
  element holding the selection; the mail line carries it.
- Replies are a thread: each `vega review reply` appends (at most 50 per note) and sets the status. `/state`
  returns all notes of the session, so the panel lists every page's notes; pins show only this page's.
- Annotate mode (button or Alt+A, Esc ends): hover outline on any element; while on, the overlay stops pointer
  events (`pointerdown/up`, `mousedown/up`, touch, `click`, `dblclick`, `auxclick`, `contextmenu`) at the window
  in the capture phase, before any page listener, and cancels the default of `click` and the like, so links,
  buttons and app handlers stay still. Press and release keep their default, so the owner can still select
  text; a selection on `mouseup` opens the editor with the quote, and the click that ends it is ignored.
  Alt+click picks an element outside annotate mode, as before.
- Panel (replaces the bottom bar): draggable by its head, collapsible, position and collapse state in the
  page origin's `localStorage` (`vega-review-panel`; a fresh agent-browser profile starts at bottom right). It
  holds a short help text, every note with status, quote, full text and the agent's full replies (scrollable,
  never cut), Edit/Remove on drafts and `Send n note(s)`. It is kept inside the window when it grows, expands or
  the window shrinks. Polling re-renders only when `/state` changed and never while a draft is being edited.

Verified live (headed, throwaway page on a free port, trusted CDP input): Alt+A turned annotate on; clicking a
link and a button opened the editor with `#go` / `#btn` and neither navigated, counted the click nor reached
the page's `mousedown` listener; a drag selection opened the editor with the quote; three drafts, one removed
and one edited through the panel, produced no mail; one Send produced one mail with both notes, the quote on
the text note; two replies showed in full as a thread; a dragged and collapsed panel kept its place and state
after reload; an expanded panel saved low in the window was pulled back inside; after Esc the button counted
clicks again; Alt+click outside annotate mode still opened the editor.

### Page notes (vega-enb, 2026-10-10)

Owner request: write to the agent without picking anything first. The panel has a text box under the list;
Enter or `Add page note` creates a draft with the page's url and an empty selector (`/note` accepts it; tag and
element text stay empty). It is queued and sent with the element drafts in the same mail, as
`note: {"id":…,"note":…,"on":"page","url":…}` without the element fields, and `vega review reply` threads
under it like any note. It gets no pin (nothing to anchor it to); the panel lists it as `this page`, or
`<path> · page` from another page. `vega review notes` shows an empty selector.

### Viewport follows the window (vega-qlq, 2026-10-10)

The first owner run showed a 1728x1080 page in a 1312x734 window: no resize, no scrolling to the end. Cause: the
agent had run `agent-browser set viewport 1728 1080 2` on the review session. agent-browser 0.31.1 never sets a
device metrics override by itself (`cli/src/native/browser.rs` `set_viewport` is reached only from
`set viewport`, `set device` and a launch `viewport` option; new tabs re-apply the last one), so a fresh headed
session already tracks its window. There is no "unset" command, but `set viewport 0 0 0` sends
`Emulation.setDeviceMetricsOverride` with zeros, which CDP treats as no override, on the daemon's own CDP session
(a second CDP client cannot clear it: overrides are per session); later tabs get zeros too.

- Headed `start` and every `highlight` read `innerWidth/innerHeight/outerWidth/outerHeight`; a page wider or
  narrower than its window (more than 16 px) or taller than it counts as pinned, and `vega review` runs
  `set viewport 0 0 0` and prints `viewport: was fixed at WxH; reset, ...`. `--headless` sessions keep any
  override (tests set sizes on purpose).
- The guide tells agents never to `set viewport` or `set device` on a review session.
- `highlight` parses the overlay's JSON answer and always prints `highlight: <sel> (<label>), shown (<n> on the
  page)` or an error (`not_found`, `no_overlay`, `overlay_failed`). The "second highlight printed only the help
  line" report was the caller's `| tail -1`, which cut the confirmation off.

Verified live, headed (`AGENT_BROWSER_HEADED=true`, throwaway page on a free port, window resized via CDP
`Browser.setWindowBounds`): after `set viewport 1728 1080 2` and a 1312x734 window the page measured 1728x1080;
`highlight` reported the reset, the page measured 1312x591, and after resizing to 1000x650 it measured 1000x507.

### Hide highlights (vega-07l, 2026-10-10)

Owner request: agent highlights cover what is under them. Each highlight label has an x that hides that
highlight (a highlight without a label gets a label holding only the x). The panel lists the agent's highlights
(label, or the selector) with Show/Hide each and one `Hide all` / `Show all` toggle (`Hide all` while any is
shown); clicking an entry scrolls to its element. Hiding is local to the page view: the agent's list stays, the
agent is not told, and `highlight` on the same selector replaces the old one (new label, shown again) instead
of stacking a second box. Only trusted clicks hide or show. Labels take their text's width (`max-content`, at
most 320 px) instead of wrapping inside narrow elements.

Verified live, headed (`AGENT_BROWSER_HEADED=true`, UA without `HeadlessChrome`, throwaway page on a free port,
trusted CDP mouse input): three highlights listed; the x on one hid its box, the panel's Show brought it back;
Hide all hid all three, Show all restored them; Hide in the panel hid one; a re-highlight of that selector
replaced it, shown, still three in the list; a scripted `.click()` on the x did nothing.

### Live-patch for design feedback (vega-lhc.149, 2026-10-19)

Owner feedback from a Spire review session: patching the owner's open page gave immediate design feedback
without a code change or rebuild. The loop: read the real DOM and reuse the app's classes, write the change as
a JS file, inject it, screenshot and crop to self-check, let the owner comment in the overlay, repeat; on
approval the patch and a written spec go to the builder. It broke on reload, SPA navigation and HMR.

```
vega review patch <name>.js           # store and run; replaces a patch of the same name
vega review patch --list              # name, bytes, runs and error on the current page
vega review patch --clear [<name>]    # remove one or all
vega review shot [--selector <css>] [--out <path>]
```

- agent-browser 0.31.1 has no `addinitscript` command (only `--init-script` at launch, and
  `removeinitscript`), so the overlay, already an init script on every page of the origin, carries the
  patches. The inbox stores them (`POST /patch`, `POST /unpatch`, `GET /patches`: CLI routes, header secret,
  no Origin; at most 20 patches of 24 KiB) and adds `patches[{name, code}]` to the overlay's `/state`.
- The overlay compiles each patch with `Function` (captured before page scripts) and runs it when it is new
  or changed, and again 150 ms after any DOM change (a `MutationObserver` on the document): SPA navigation,
  re-renders and HMR updates re-apply it; a reload or full navigation fetches it with the state. Records of
  the patches' own changes are discarded (`takeRecords`), so a patch does not trigger itself. Hence the
  convention: a patch must be idempotent, marking what it adds or changes with `data-patched="<name>"` and
  skipping when the marker is there; `patch` refuses a file without `data-patched`. The name is the file
  stem (letters, digits, `-`, `_`).
- A replaced or cleared patch leaves its nodes in the page, and a changed patch would skip on its old marker,
  so `patch` of an existing name and `--clear` reload the page. A new patch runs without a reload.
- `patch` waits up to 4 s for the overlay's `patches()` report and fails with `patch_error` (the JS error) or
  `patch_not_run`; on a page without the overlay it says the patch is stored.
- `shot` hides the overlay (`__vegaReview.hide`) so highlights and the panel do not cover the design, runs
  `agent-browser screenshot [<selector>] [<path>]` (agent-browser crops to the element itself; no `sips`), and
  shows the overlay again. `--out` is made absolute: the agent-browser daemon has its own working directory.
- Limit: a CSP without `unsafe-eval` blocks `Function`; the patch then reports an `EvalError`.

Verified live (headless, throwaway page on a free port with a `pushState` re-render): a patch without a marker
was refused (exit 2); the patch ran once, kept one marked node after SPA navigation, a full navigation and a
reload; a broken patch failed with `nope is not defined` and `--list` showed it; `shot --selector .card` wrote a
200x80 crop without the overlay, `shot --selector .nope` failed `not_found`; the overlay was visible again
after; re-patching reloaded with one node; `--clear` of one, of a missing one (`not_found`), of all (0 nodes
after the reload), and with none left.

### Limits

- The secret guards against other pages and browsers, not against the reviewed app: its scripts share the page
  and could read what the owner types into the open shadow root.
- Highlights belong to a page: navigation drops them (pins come back, highlights do not).
- A strict app CSP (`connect-src`) blocks the overlay's fetch to the inbox; the bar then shows the error.
- One review per pane, and the agent-browser session must be new: the init script is registered at launch.
