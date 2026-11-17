# vega-browser

Status: implemented (phase 1, bead vega-nic.18; phase 2 panels/colours, vega-nic.19), 2026-11-17.
Replaces the global skill `browser`
(`~/.agents/skills/browser`, symlinked as `~/.claude/skills/browser`).

## Problem

Several headless "Chrome for Testing" processes run at once, and nobody can tell which session
owns which. agent-browser keeps the session name only in its daemon's environment
(`AGENT_BROWSER_SESSION`), not in Chrome's argv. The `browser` skill asks agents to use `scripts/ab`,
but nothing enforces it, so agents call plain `agent-browser`. Some leftovers are not agent-browser
at all: rally-coach lanes run headless Playwright, and a killed run leaves its Chromium behind.

## Survey (2026-11-16, all UACs)

| Project | How it starts a browser | Flags | Plain call without a session |
| --- | --- | --- | --- |
| vega | `vega review start` runs `agent-browser --session review-<pane>`: headed, one per pane, injected overlay | `--session` | never |
| rally-coach | `vega review` + `agent-browser --session wt-<journey>`; lanes use headless Playwright 1.60 (bundled Chromium) | `--session` | never |
| brainz | `~/.claude/skills/browser/scripts/{ab,chromium.sh}` from huk.py, vodafone.py, chatgpt_invoices.py, scripts/web/chromium.sh; scripts/einkauf/cdp.py talks raw CDP to 9223 | `--session` via ab | never |
| spellkave | plain agent-browser; verify-play-live (`--config`), tile-clip.ts (env `AGENT_BROWSER_SESSION`), bin/cdp-share (own Chromium + CDP); headed sentinels on Chrome for Testing (e.g. `classic-live`, which the owner watches) | `--session`, `--cdp`, `--headed`, `--config` | `apps/play/scripts/no-scroll.sh` |
| parallax | own ungoogled Chromium (CDP 9333, per-site profiles under `~/.local/share/parallax/chromium`), then `agent-browser connect <port>` | `--session`, `--restore`, `--proxy`, `--json`, `--headed`, `--download-path` | never |

So every flow names its session; the flags and browsers vary per project.

## Decisions

1. **One rule: name your session.** A call carries a session name, either `--session <name>` or a
   non-empty `AGENT_BROWSER_SESSION`. A named call passes through unchanged, with every flag and
   subcommand (`--cdp`, `connect`, `--headed`, `--config`, `--executable-path` …). A call without a
   name is refused with a hint. The shim does not police browsers or profiles; projects pick their
   own (Chrome for Testing, ungoogled Chromium, the global Chromium).
2. **Refuse, never redirect.** Redirecting to the global Chromium would silently change the browser
   and the logins.
3. **The shim records the owner.** For every named call it upserts
   `$XDG_STATE_HOME/vega/browser/sessions.json`:
   `{session: {pane, host, project, cwd, first_seen, last_seen}}`. `pane` is the calling agent's pane
   (as `vega mux` resolves it), or null outside a pane.
4. **Cleanup never kills what it cannot attribute to a dead owner.** `vega browser gc` closes
   (`agent-browser --session <s> close`) only sessions whose recorded owner pane is gone. A session
   whose owner pane is alive is never touched (spellkave's `classic-live`). An unrecorded session is
   only listed, never closed; `vega browser close <session>` closes one on explicit request.
   Playwright leftovers are listed as `unowned` with their PID; vega-browser does not kill them.
5. **The skill moves into a plugin.** `plugins/browser` (binary `vega-browser`, guide via
   `vega skills browser`, stub skill as for every plugin) takes over `ab`, `chromium.sh`,
   `launch.sh` and `cleanup.sh` as subcommands. The global skill is removed from vega-skills.
6. **No permanent stubs.** brainz switches its hard-coded skill paths to the new commands
   (bead in brainz, after this lands). The old paths stay as forwarding scripts only for the
   migration and are removed after brainz has switched.

## Commands

```sh
vega browser ab --session <name> <agent-browser args>   # was scripts/ab: global Chromium on 9223
vega browser chromium                                   # was chromium.sh: start/attach the global Chromium
vega browser launch <url> [--session s] [--port p]      # was launch.sh: throwaway Chromium for bot blockers
vega browser cleanup [--session s] [--port p]           # was cleanup.sh: close it, delete the temp profile
vega browser ls                                         # sessions: name, owner pane/project, age, state
vega browser gc                                         # close sessions of dead owner panes (prints what it closes)
vega browser close <session>                            # close one session on request
```

`ls` merges `agent-browser session list`, the owner records and the browser processes (`ps`), and
marks rows `live`, `stale` (owner pane gone, or no call for more than a day) or `unowned`
(no record: older sessions, Playwright).

## Visibility and colour (phase 2, vega-nic.19)

Built (2026-11-17), after verifying every mechanism against agent-browser 0.31.1
with throwaway sessions on Chrome for Testing and the global Chromium (9223):

- **What works:** `--init-script <file>` marks the navigations of the call that
  carries it; a generated per-session script draws a coloured frame, a session
  tag and prefixes the tab title with `[session]`. It must defer all DOM work
  to DOMContentLoaded (init scripts run before `document.documentElement`
  exists), guard itself against duplicate registrations in one call, and
  re-prefix the title through a `MutationObserver` when a page renames itself.
  Frame and tag carry `aria-hidden="true"`, so `snapshot` does not list them
  (screenshots still show the frame; title reads change).
- **Review guard rails (Slayer, 2026-11-17):** the mark changes what callers
  see — title asserts, `snapshot` text, screenshot pixels, injected DOM on
  bot-wary sites — so it rides only explicitly headed calls: a `--headed`
  flag on the call or a truthy `AGENT_BROWSER_HEADED` (the shim and `ab`
  alike; `connect`/`--cdp`/plain attaching calls stay unmarked).
  `VEGA_BROWSER_MARK=0` turns it off everywhere, and `launch` (the bot-blocker
  fallback) never marks. Headless sessions show their colour in the panels
  only, as the design says.
- **What does not:** `addinitscript` (unknown command in 0.31.1 despite its own
  help); `--init-script` on `tab new <url>` (the new tab's first navigation is not
  marked); `eval` on a shared browser (it runs on the *active* tab, which can be
  the user's — never used for marking); `vega browser ab`'s HTTP/1.0 CDP probe
  (Chrome 146 closes 1.0 requests — fixed to HTTP/1.1).- **Not built:** the profile theme colour for vega-launched `--user-data-dir`
  browsers. Chromium 146 accepts an unpacked theme via `--load-extension` and
  records it in the profile's Preferences (verified structurally), but the frame
  colour was not visually verifiable from an agent shell (no screen-recording
  permission); revisit with one eyeball pass before wiring it into `launch`.

The panels:

- **Services panel (right):** a "Browsers" group from `vega browser ls --json`:
  session, colour dot, owner pane and project, headed/headless, age, state;
  unowned sessions and loose browser processes (Playwright leftovers, grouped by
  profile) show too, with no close. Sessions whose owner resolves to a project
  follow the panel's project scope; the rest always show. Closing is two-step
  (Close → "Close session?") and runs `vega browser close <session>`, nothing
  else. The read rides the panel's existing refresh (first show + manual
  Refresh; no timer) and publishes `shared("vega.browsers")` for the sidebar.
- **Sidebar (left):** up to three colour dots per owning agent row (more show
  `+n`), read from that shared state — the sidebar never reads browsers itself;
  the payload is part of the render model's input signature (vega-0h4c.103), so
  it derives once per change. Headless sessions show the same colour in the
  panels; their pages carry the mark harmlessly.
- **Colour:** `vega browser ls --json` carries a `colour` per session: the owner
  project's `[symbols.project_colors]` colour in `~/.config/vega/theme.toml`
  (keyed by the worktree directory name, like host colours; validated
  `#RRGGBB`), else a stable FNV-1a hash of the session name into a ten-colour
  palette. The mark script uses the same colour, so window, panel row and badge
  agree. `ls --json` also fills a session's `headed`/`pid` from the browser
  whose `--user-data-dir` ends in `<session>-profile` (agent-browser's own
  headed launches); helper processes share the browser's arguments, so the
  group's lowest pid is the browser.

## Shim

`shims/agent-browser` follows `shims/README.md`: active only in agent shells (`activate.sh`), so
humans, scripts started by humans and CI (GitHub runners have no vega) see the real binary. Agent
subprocesses inherit the shim, which is intended: parallax's and spellkave's scripts run from agent
shells and all name their session. The shim `exec`s the real binary by absolute path (found on
`PATH` after the shim directory), and `vega-browser` calls the real binary the same way, so neither
recurses. Exempt without a name: `--help`, `--version`, `skills …`, `session list`.

The refusal hint:

```
agent-browser: name your session: --session <name> or AGENT_BROWSER_SESSION=<name>
  (e.g. --session <project>-<task>); see `vega skills browser`
```

## Migration

1. Land the plugin, the shim and the forwarding scripts at the old skill paths; remove the global
   skill from vega-skills. `vega review` is unchanged (it already names its session).
2. Mail the UACs: spellkave fixes `apps/play/scripts/no-scroll.sh` (add `--session`); brainz
   switches its paths to `vega browser ab` / `vega browser chromium`.
3. After brainz confirms, remove the forwarding scripts.

## Out of scope

Killing Playwright leftovers (rally-coach's lanes own them); a shared parallax profile with a PAC
proxy (parallax-cdg.44); Lightpanda.
