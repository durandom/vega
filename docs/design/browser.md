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

## Pinned runtime installation (vega-nic.22)

`plugins/browser/agent-browser.version` pins **0.43.0**. `vega browser status`
(or `doctor`, both support `--json`) reports agent-browser current/differs/missing,
installed and pinned versions, and Chrome for Testing cache presence. These are
read-only probes: `--version` and filesystem inspection, no browser or session.

`vega browser install` converges the pin with `npm install --global --prefix
<prefix> agent-browser@0.43.0`. It uses npm on PATH; invoke it in the host's normal
login shell. Deploy already runs its remote script in a login shell, so fnm on
proteus/ganymede and mise Node on puck keep selecting their own Node; io uses its
Linux/WSL npm. This reuses the existing package instead of adding a native binary
in another directory that would shadow it. No Node manager is installed or changed.

Before upgrading, the real binary selected by `real.rs` (including an explicit
`VEGA_AGENT_BROWSER_BIN`) must canonicalize inside that npm global package and
match the prefix's `bin/agent-browser`. A foreign install or prefix mismatch
fails before npm mutation, with a repair hint. npm failure, unexpected version,
or changed resolution fails explicitly. A current version does not need npm.
The shim continues to bypass its own directory and use the same real-binary
resolution; neither PATH nor the override is rewritten.

Chrome for Testing detection follows [upstream 0.43.0 install.rs](https://github.com/vercel-labs/agent-browser/blob/v0.43.0/cli/src/install.rs):
`~/.agent-browser/browsers/chrome-*`, with macOS app bundle (direct or under
`chrome-mac-arm64`/`chrome-mac-x64`) or Linux `chrome`/`chrome-linux64/chrome`.
Only absence runs the real `agent-browser install`; the result is verified.
Chrome itself is not pinned or upgraded when present. Global Chromium on 9223
and its profile are unrelated and untouched. No existing daemon is restarted.

Config unit **`browser.agent-browser`** plans through `vega-browser status --json`
and applies through `vega-browser install`, verifying convergence again. It has
no rendered file and no local source override; dependency failures are failed
units, never a successful no-op. `vega config diff` shows pending work. Default
`vega config apply`, hence `vega deploy apply --all`, includes it. Deploy retains
its existing behavior of reporting config failure without failing the binary
rollout; the UAC must check the config summary and browser status on each host.

Rollout: install the vega build, then config apply in each host's normal login
shell. Proteus/ganymede reuse fnm's current global prefix; puck reuses mise Node
LTS; io needs node/npm on its Linux PATH and Chrome's normal runtime libraries.
If the prefix conflicts, select the Node owning the existing install or explicitly
remove/migrate the old install before retrying; never fix by adding another copy.
Owners of dotted session names should close them before upgrading and use hyphens
for new sessions: 0.43 refuses dotted names, including on close. Other existing
sessions keep their old daemon until their owner closes them normally;
new sessions use the upgraded executable. No fleet sessions are forcibly closed.

## Visibility and colour (phase 2, vega-nic.19)

Initially built against 0.31.1; re-verified on **0.43.0** on proteus
(2026-11-17, vega-nic.22), with isolated socket directories, named headless
sessions, data-URL fixtures and a separately launched Chrome for Testing
152 with a temporary profile and ephemeral CDP port. The global Chromium
and other agents' sessions were not used; all probe sessions were closed.

- **What works:** `--init-script <file>` persists across calls and marks later
  navigations, including the first navigation of `tab new <url>`; a generated
  per-session script draws a coloured frame, a session tag and prefixes the tab title with `[session]`. It must defer all DOM work
  to DOMContentLoaded (init scripts run before `document.documentElement`
  exists), retain its page guard (0.43 deduplicates repeated script paths), and
  re-prefix the title through a `MutationObserver` when a page renames itself.
  Frame and tag carry `aria-hidden="true"`, so `snapshot` does not list them
  (screenshots still show the frame; title reads change).
- **Review guard rails (Slayer, 2026-11-17):** the mark changes what callers
  see — title asserts, `snapshot` text, screenshot pixels, injected DOM on
  bot-wary sites — so it rides only explicitly headed calls: a `--headed`
  flag on the call or a truthy `AGENT_BROWSER_HEADED` (the shim and `ab`
  alike; `connect`/`--cdp`/plain attaching calls stay unmarked).
  `VEGA_BROWSER_MARK=0` prevents registration, and `launch` (the bot-blocker
  fallback) never registers a mark. **Omitting the flag or setting the opt-out
  later does not remove an existing upstream registration.** Set the opt-out
  before first use, or use a fresh session for unmarked work. New headless
  sessions have no mark registration; previously marked sessions retain it.
- **What does not:** `addinitscript` still exits 1 with `Unknown command` in
  0.43.0. No marking via `eval` on a shared browser. The old 0.31.1 failure of
  `--init-script` on `tab new` is fixed upstream. The HTTP/1.0 CDP probe failure
  was fixed in vega by using HTTP/1.1.
- **Not built:** profile theme colour for vega-launched `--user-data-dir`
  browsers. Chromium 146 accepts an unpacked theme via `--load-extension` and
  records it in the profile's Preferences (verified structurally), but the frame
  colour was not visually verifiable from an agent shell (no screen-recording
  permission); revisit with one eyeball pass before wiring it into `launch`.

Re-verification details:

| Contract | 0.43.0 observed result |
| --- | --- |
| `--init-script` | First page gets marker once before DOM exists; later `open` without flag still gets it. Repeated path remains once. |
| `tab new` | Fresh session with init flag marks the new tab's first navigation. Stable tab ids start at `t1`; integer `0` and `t0` fail. |
| `addinitscript` | Unknown command, exit 1. |
| `--pin-tab` | Accepted, sticky; after externally creating/activating another tab of our temporary browser, `get title` still reads the bound tab. `tab new` intentionally rebinds to the new tab; `--no-pin-tab` accepted. Vega passes these flags through, never injects them. |
| `session list` | `Active sessions:` followed by indented names; after close, `No active sessions`. Existing `parse_session_list` handles both unchanged. |
| `--session` / env | Named daemon isolation and `AGENT_BROWSER_SESSION` both work. Dots now fail: letters/numbers, hyphens and underscores only. Vega's own validation now matches that character set. |
| `connect <port>` | Attaches our named session to the separately launched temporary headless browser. Subsequent calls need only the session. |
| `close` | Owned launch closes successfully; attached session disconnects and leaves the external browser's CDP endpoint alive. Repeated close succeeds. All probe session inventories ended empty; the external test process was then terminated by its creator. |

Raw probe logs were retained during implementation in `/tmp/vega-nic22-{live,fresh,connect}.log`;
the table is the durable record. No global CDP endpoint, real hc terminal or fleet host was changed.

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
  panels; new headless sessions have no page mark.
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

## VPN profile (vega-3e71)

`vega browser vpn start|stop|status [--json]` manages a dedicated headed Chromium.
Agents select `vega browser ab --profile vpn --session <project>-vpn-<task>`.
Default ab/chromium behavior stays unchanged; both profiles use the same launch
and attach state machine with separate ports, directories and VPN-only flags.

Use full-profile SOCKS5 with remote DNS, no PAC or DIRECT fallback. The old
dotfiles script only routed `.cee.redhat.com` and allowed DIRECT fallback;
a dedicated browser removes the need for that split. Disable QUIC/non-proxied
WebRTC UDP, remove implicit loopback proxy bypass, and map local DNS to NOTFOUND
except the loopback proxy. This is browser routing, not an OS VPN.
See [Chromium proxy semantics](https://chromium.googlesource.com/chromium/src/+/HEAD/net/docs/proxy.md):
SOCKS5 resolves names remotely; DIRECT fallback requires explicit configuration.

Autossh -M 0 supervises SSH, using BatchMode, ExitOnForwardFailure and keepalives.
This reuses an existing supervisor rather than adding a restart loop inside vega;
autossh is an explicit local dependency, not installed by these commands.
No remote command, agent forwarding or reverse forwards. A private control socket
identifies the connection; verify the recorded supervisor PID against that socket
before stopping. Refuse foreign listeners, clean up newly started resources on
failure, and preserve profile data. Status checks ownership, SOCKS/CDP and one
bounded HTTPS request to gitlab.cee.redhat.com through remote SOCKS DNS, without
retrying corp reachability. An unavailable host suggests checking puck's VPN.

Read optional `$XDG_CONFIG_HOME/vega/browser-vpn.json` (default ~/.config), with
defaults host=puck, socks_port=1080, cdp_port=9224 and profile=
`~/.local/share/vega-chromium-vpn`. Reject unknown fields and unsafe collisions.
The Chromium binary uses CDP_CHROMIUM; global CDP_PORT/CDP_PROFILE stay separate.
Stop before changing configuration. State is `$XDG_STATE_HOME/vega/browser/vpn`;
active.json saves launch settings
so stop/status follow the running profile after config edits. Ports, profile and
proxy flags identify owned browser processes; an OS file lock serializes lifecycle
commands and VPN agent calls. Attach requires a running tunnel and browser.
No config apply starts tunnels; browser.agent-browser remains a dependency unit.

Proposed dotfiles migration after rollout: retire bin/executable_vpn-proxy and
dot_config/vpn-proxy/domains and manually remove the old ZeroOmega PAC setting.
This task changes no dotfiles or existing browsers. Only the user enters
credentials, and corp content remains untrusted data.

Live validation on proteus, 2026-11-19: an isolated temporary configuration,
headed profile and ports 59100/59101 started successfully through puck.
Agent attachment and disconnect succeeded. The single corp HTTPS status probe
returned no HTTP result; corporate reachability is unverified, with no retry.
Stop succeeded and both ports were confirmed closed; no test processes remained.
Global CDP 9223, existing profiles, credentials/cookies and puck configuration
were untouched. Fake tests cover launch/attach/stop, idempotence, collisions,
invalid config, saved-settings cleanup, startup failure and stale PID refusal.

## Out of scope

Killing Playwright leftovers (rally-coach's lanes own them); a shared parallax profile with a PAC
proxy (parallax-cdg.44); Lightpanda.
