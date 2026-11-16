# vega-browser

Status: implemented (phase 1, bead vega-nic.18), 2026-11-16. Phase 2 (panels, colours) is
vega-nic.19. Replaces the global skill `browser`
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

## Visibility and colour (phase 2, bead vega-nic.18 follow-up)

- **Services panel (right):** a "Browsers" group from `vega browser ls --json`: session, colour
  dot, owner agent and project, headed/headless, age, state. Unowned and stale rows show too;
  closing a row needs a confirm and only offers what `gc`/`close` allow.
- **Sidebar (left):** a small browser badge on the owning agent row, in the session's colour.
- **Colour:** each session gets a colour, by default its owner project's colour from the theme
  (fallback: a stable hash of the session name). A headed browser shows it in the window itself,
  so the window on screen can be matched to the row. Candidates, to be verified before building:
  an init script that sets a coloured frame and a `[session]` title prefix (works in any browser,
  also on the global Chromium, where a per-session profile is not possible); for browsers vega
  launches with their own `--user-data-dir`, the profile's theme colour as well. Headless
  sessions have no window; they show the colour only in the panels.

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
