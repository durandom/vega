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

`vega browser install` detects an active `npm:agent-browser` from mise, including
a mise shim on PATH. On proteus and ganymede, mise reads this tool from
`~/.config/mise/conf.d/agent-tools.toml`. A matching version skips the package
install and still checks Chrome. A mismatch fails with a pinned `mise use --path`
command targeting the configuration that supplied the active version.

For npm globals, `vega browser install` converges the pin with `npm install --global --prefix
<prefix> agent-browser@0.43.0`. It uses npm on PATH; invoke it in the host's normal
login shell. Deploy already runs its remote script in a login shell, so mise
Node on puck and Linux/WSL npm on io keep selecting their own Node. This reuses
the existing package instead of adding a native binary
in another directory that would shadow it. No Node manager is installed or changed.

Before upgrading, the real binary selected by `real.rs` (including an explicit
`VEGA_AGENT_BROWSER_BIN`) must canonicalize inside that npm global package and
match the prefix's `bin/agent-browser`. A foreign install or prefix mismatch
fails before npm mutation, with a repair hint. npm failure, unexpected version,
or changed resolution fails explicitly. A current version does not need npm.
The shim continues to bypass its own directory and use the same real-binary
resolution; neither PATH nor the override is rewritten.

The browser installer pins Chrome for Testing to the stable build recorded in
`plugins/browser/src/chrome.rs`. It downloads Google's platform archive into a
staging directory in `~/.agent-browser/browsers` and publishes the complete
build as `chrome-<version>`. Older cached builds stay available. Global
Chromium on 9223 keeps its own binary selection.

Config unit **`browser.agent-browser`** plans through `vega-browser status --json`
and applies through `vega-browser install`, verifying convergence again. It has
no rendered file and no local source override; dependency failures are failed
units, never a successful no-op. `vega config diff` shows pending work. Default
`vega config apply`, hence `vega deploy apply --all`, includes it. Deploy retains
its existing behavior of reporting config failure without failing the binary
rollout; the UAC must check the config summary and browser status on each host.

Rollout: install the vega build, then config apply in each host's normal login
shell. Proteus/ganymede use their configured mise tool; puck reuses mise Node
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
WebRTC UDP and remove implicit loopback proxy bypass. No `--host-resolver-rules`:
Chromium shows an "unsupported flag" warning bar for it, and SOCKS5 already
resolves page names on puck; a leaked local prefetch cannot resolve corp names.
The status probe uses `--insecure` (reachability only; the corp host uses an
internal CA). This is browser routing, not an OS VPN.
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
VPN defaults to the exact pinned Chrome for Testing build. CDP_CHROMIUM
overrides the binary; global CDP_PORT/CDP_PROFILE stay separate. Status reads
`browser_version` from the owned running browser's DevTools endpoint.
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

## VPN profile root trust (vega-nt71)

Before launch, `vpn start` ensures the configured CA is trusted; `vpn trust`
performs just this step. The default `ca_sha256` pin is
`4678DB0942D0B9E732122507BD6CDC08D128AB832DA274AD7D46F8C3576BC925`;
configuration accepts exactly 64 hexadecimal digits, case insensitive. Fetch is
read-only SSH BatchMode to the configured host (default puck), running
`security find-certificate -a -p -c 'Internal Root CA' /Library/Keychains/System.keychain`.
Select only the PEM certificate whose DER SHA-256 matches the pin; absence or
mismatch fails before creating a profile/store or launching anything.

Mechanism: Chromium's native `Default/ServerCertificate` SQLite store, only
while the profile is stopped. Chromium 146's profile service passes
`Profile::GetPath()` to this database. Schema v1 has `meta` version and
last_compatible_version both 1, and `certificates(sha256hash_hex TEXT PRIMARY
KEY, der_cert BLOB NOT NULL, trust_settings BLOB NOT NULL)`. Hashes are lowercase
DER SHA-256. Trust metadata `0a020803` is protobuf CertificateMetadata.trust
(field 1), CertificateTrust.trust_type (field 1), TRUSTED (3). The user's
manually imported entry confirmed this format through a read-only query.

This internal format is guarded, not an upstream compatibility guarantee.
Check versions and column shape; fail closed for unknown formats without
migrating or razing. A transaction inserts only the missing pinned certificate,
preserving all other entries. Existing matching DER and canonical trust are a
read-only no-op, including when Chromium is running, without SSH. Conflicting
DER, explicit distrust, constraints or unknown metadata require manual review;
never overwrite them. A missing CA requires no process using the profile and
no SingletonLock; symlinks within the certificate path are refused. Vega's
lifecycle lock serializes its own commands; avoid launching Chromium manually
against this directory during trust import. Status reads the database read-only:
`ca_trusted` is true for the canonical pinned entry, false for absence, null
for unreadable/unsupported/conflicting metadata.

Chosen over macOS/NSS stores (not this per-profile store), policies (broader
scope), fragile certificate-manager UI automation, and certificate-error bypass
flags (which do not install a CA). No public CDP CA-import API was identified.
Full TLS verification remains enabled; no keychain writes, policy changes,
global profile changes or running-profile writes.

Source references (Chromium 146.0.7680.0):
- [profile factory](https://chromium.googlesource.com/chromium/src/+/146.0.7680.0/chrome/browser/net/server_certificate_database_service_factory.cc)
- [store and schema](https://chromium.googlesource.com/chromium/src/+/146.0.7680.0/components/server_certificate_database/server_certificate_database.cc)
- [trust protobuf](https://chromium.googlesource.com/chromium/src/+/146.0.7680.0/components/server_certificate_database/server_certificate_database.proto)
- [verifier additional trust](https://chromium.googlesource.com/chromium/src/+/146.0.7680.0/net/cert/cert_verify_proc_builtin.cc)

## Review in a browser profile (vega-tdeu)

`vega review start <url> --profile global|vpn` uses the normal browser plugin
attach path. Review invokes `vega-browser ab` so the profile checks, global
launch, owner record, and headed marking stay in one implementation. For VPN
it first invokes the idempotent `vega-browser vpn start`. Headed local reviews use the managed dev allocator by default. Explicit
headless reviews launch an isolated agent-browser.

Global and VPN Chromium share their default login context. Agent-browser 0.43
discovers all page targets on a browser-level CDP connection and can select
the active or first page when attaching or after a close. A daemon name alone
does not filter that discovery.

For `ab --profile global|vpn`, Vega records only the target IDs returned by
`Target.createTarget` for the named session, together with the browser websocket
generation. Tab IDs are local to the session. Vega serializes commands for a
name, filters tab listing and selection through that registry, and allocates a
blank page if all owned tabs are gone. Closing the active tab selects an owned
successor first, allocating a blank page when closing the final tab. The runtime and review's CSP client
receive a loopback websocket bound to one owned target. Each connection filters
discovery, attachment, activation and page commands at the CDP boundary.
Closing the session releases its remaining tabs. Plain named shim calls reuse
the recorded binding.

A shared review starts on an owned blank page with a unique fragment, attaches
its CSP bypass there, then opens the app through the same named session. Stop
releases the session's targets and leaves Chromium and the VPN tunnel running.

## Shared headed development browser (decision, 2026-11-20)

Proposed implementation for vega-tyye. The earlier sections describe the deployed browser and review commands. The reported inventory of 26 sessions, including 19 without an owner, motivated consolidating visible windows. A session count measures agent-browser daemons, so it can exceed the number of browsers even after migration.

| Request | Browser and lifetime |
| --- | --- |
| Real-internet work, including brainz and explicit global review | Headed Chromium.app on 127.0.0.1:9223 with the existing ~/.local/share/vega-chromium login profile. Preserve the existing ab contract. |
| Explicit VPN work or review | Existing headed VPN Chromium on 9224 by default. Its configured endpoint, SOCKS tunnel, CA pin and stopped-profile import remain the responsibility of vpn.rs and vpn/trust.rs. |
| Headed development, including default local review | One host-local CfT process. Each project gets an ephemeral CDP browser context and one window. Each named session owns its tabs. |
| Headless tests and lanes | Keep the caller's isolated process, launch settings and teardown. |

### Launch, ownership and attachment (vega-tyye.2)

Use the installed CfT executable and a dedicated profile under XDG data at vega/browser/dev-cft. Launch headed with loopback debugging on port 0 and read DevToolsActivePort from that profile. This avoids fixed-port collisions, including configured VPN ports. Serialize launch and allocation. Record the process identity, canonical profile and browser WebSocket generation, and revalidate them before reuse. A stale record or occupied profile is grounds to rediscover the owned instance or fail closed. Never discover dev by scanning ports.

Resolve the worktree through Vega's canonical project resolver. Key contexts by project ID and display the project's label. The basename that owner.rs currently records can collide across repositories. On first use call Target.createBrowserContext, then Target.createTarget with that context and newWindow=true. Allocate further tabs in that window and verify context and window before returning a binding. Record generation, project, context, window, session, pane and target. Concurrent opens for one project must converge. Closing one session releases only its targets. Dispose a project context when its last session releases it and no user tabs remain. A browser restart invalidates every old binding.

Use a named dev entry point, `vega browser dev --session <name> …`, and route ordinary headed dev launches through it. Preserve the upstream command output. Agent-browser 0.43.0 exposes browser-level --cdp and a sticky --pin-tab. Review's current first pinned open allocates a tab, and an explicit tab new can rebind it. Neither operation selects a caller-specified context. On proteus a fresh real daemon given the project's page WebSocket returned about:blank from get url instead of the unique about:blank#project-probe, despite exit 0. A raw page WebSocket is therefore unsuitable as the binding contract.

Implement explicit context and initial target selection in the agent-browser attachment path (an upstream change or a pinned adapter). Verify TargetInfo.browserContextId before the first navigation or init script. Bind subsequent page operations to that ID, scope discovery and tab creation to the session's project, and scope selection and close to its owned tabs. Refuse endpoint overrides, automatic reconnection to another target, and operations that escape the binding. A missing target has a structured stale-session error. This is an implementation prerequisite for switching review.

### Project and session colours (vega-tyye.3)

On proteus, headed CfT 152.0.7977.64 loaded a scratch MV3 extension with --load-extension. Its worker appeared in CDP. In a fresh profile the extension reported incognito access disabled and saw only the ordinary blank tab. With Developer mode and Allow in Incognito enabled in the disposable CfT profile, the default spanning extension could enumerate the project tab but grouping failed with “No tab with id” or “No window with id”. With manifest incognito set to split, a packaged extension page opened via Target.createTarget inside the project context saw the incognito project tab and chrome.tabs.group returned a group ID for its window. The fixture used a temporary profile, OS-assigned port and blank pages. It stopped its own browser after each probe. [Chrome incognito modes](https://developer.chrome.com/docs/extensions/reference/manifest/incognito)

Load the packaged extension only in managed dev CfT. Give it a stable extension ID. Before allocating a marked tab, check incognito access in the regular context. When access is missing, use the private browser CDP connection to open the bundled extension's details in the regular context, enable Developer mode for the unpacked extension's reload and toggle Allow in Incognito, close the settings tab and verify the grant with the extension API. If automation fails, report the manual setup steps. The scoped CDP proxy blocks navigation to Chrome's settings.

Use a packaged control page in the verified project context. The wrapper supplies the project label, session and theme colour by typed CDP function arguments. The control page creates and groups the session's blank tab in its own window using the Chrome tabs APIs. This also works in contexts created before incognito access was granted, where Chrome may not start a split-mode extension worker. Correlate the tab ID with the CDP page target using a fresh one-use blank-page fragment and verify the context and window before exposing it. Allocation has a deadline and closes the temporary control tab and any failed nonce tab.

Title each session group with project and session, and map mark.rs project colours deterministically to Chrome's group palette. Preserve the opt-out and existing frame/title marks on paths that use them. A tab or group ID is meaningful only within its browser generation. [Chrome tab groups](https://developer.chrome.com/docs/extensions/reference/api/tabGroups)

On first use, open `chrome://extensions` in the regular managed Chrome for Testing window, enable Developer mode, then open **Vega Dev Tab Groups** details and enable **Allow in Incognito**. Retry `vega browser dev --session <name> open <url>`. An already-running managed browser from before the extension was packaged needs to be restarted. Use `VEGA_BROWSER_MARK=0` when opening tabs to skip grouping and page marks. Existing groups remain until their tabs are closed or manually ungrouped.

The extension needs only the `tabGroups` manifest permission and has no host permissions. Chrome owns the groups. Each allocation reconciles its session's verified tabs and leaves groups containing other tabs alone. Run the headed fixture with `nice -n 10 node plugins/browser/tests/dev-groups.mjs target/debug/vega-browser` after building the browser plugin. It tests the grant in a pre-existing context, persistence across restart, and cleanup after a stalled allocation using only a throwaway profile.

### Explicit dev state (vega-tyye.4)

Provide `vega browser dev state save` and `vega browser dev state restore` for the resolved project. Restrict both to a positively identified managed CfT generation and its registered non-default dev context. Reject profile, URL and endpoint overrides before connecting. Never export or import from Chromium 9223/9224, configured VPN endpoints, headless sessions or the default CfT context. Start, close, GC and review never trigger a snapshot.

Keep the filesystem and snapshot authority in the CLI. From the project checkout, use `vega browser dev state register <origin>` once per HTTP(S) origin in the live project, then `vega browser dev state save` before closing the last session. Registrations belong to the project context and survive navigation and tab closure during its lifetime. Restore reinstates the registrations.

Use context-qualified CDP cookie operations and first-party localStorage for registered origins. V1 accepts host-only cookies for registered hosts and preserves supported cookie attributes, including source scheme and port. Domain and partitioned cookies touching the registered scope make save fail. A temporary page in the managed context uses a synthetic, intercepted first-party document to access localStorage without contacting the app. Report unsupported cookie attributes or inaccessible origins before replacing a good snapshot. IndexedDB, sessionStorage, caches and extension storage are outside v1. Keep snapshot values out of output, logs and extension storage.

Store one file per encoded canonical project ID under ${XDG_STATE_HOME:-~/.local/state}/vega/browser/dev-state, directory 0700. Create a 0600 temporary file in that directory, validate ownership and refuse symlinks, then rename atomically. Validate version, project and origin scope before replay. Restore into a fresh unpublished project context before opening the app. Refuse replacement while a project is live. On failure dispose the staged context so retries start clean. Acceptance uses synthetic loopback fixtures and a CfT restart.

### Review, guidance and legacy cleanup (vega-tyye.5)

Route default local headed review through the dev allocator after target binding works. Preserve its unique blank target, pre-navigation CSP setup, origin-scoped overlay, pane inbox, native viewport, printed session and tab-only stop. Explicit global/VPN review keeps its existing path. Keep explicit headless review isolated. Update browser and review runtime guides, shim hints and help together, including how a subsequent plain named command reuses its binding. Treat existing explicitly configured external endpoints as external sessions; never silently adopt them into dev.

Keep ls compatible with existing owner records. Distinguish a confirmed dead local owner from a missing record, remote owner or unknown liveness. Offer a dry-run inventory and explicit per-session disconnect for reviewed legacy ownerless entries. Age alone cannot prove that a process is abandoned. Recheck identity and liveness immediately before acting. Ordinary GC handles proven dead local owners only. Demonstrate cleanup with fixtures before any operator-directed cleanup on a live machine.

`vega browser gc --dry-run` reads the legacy daemon socket directory and shows
owner classifications. Copy the current identity of one reviewed ownerless
entry to `vega browser disconnect <session> --identity <identity>`. It sends
close over the verified Unix socket. The upstream daemon disconnects from an
attached browser or shuts down a browser it launched. The dry run leaves stale
daemon files and owner records intact.

### Remote dev shares

`vega browser dev share <project> --to <host> --origin <origin>` requires a
live managed dev project and the tab-group extension's incognito grant. The
supervisor allocates fresh grouped tabs in its recorded window and context.
Existing local tabs stay private. Each command checks the target, context and
window against Chrome. Remote tab IDs and accessibility refs belong to the
share. The worker revalidates the browser generation and never scans ports.

The display machine starts a dedicated OpenSSH master and requests a dynamic
reverse TCP listener on remote IPv4 loopback. Its local destination is a
one-use Unix socket in a private directory. The supervisor verifies the
actual remote bind using `ss` or `lsof` before publishing the URL. The remote
account needs public-key SSH access, remote forwarding, and one of those
listener inspection tools. A 256-bit capability authorizes one command per
WebSocket connection. Pending handshakes have a two-second total deadline and
share a bounded pool. Expiry follows the earlier of wall and monotonic deadlines,
including time spent asleep. Browser-originated handshakes are rejected. Stop, loss
of the browser generation, SSH exit, guard loss and TTL revoke the listener
and connected clients. The supervisor then closes the share's verified tabs.
The remote SSH master session uses a stdin-backed command so loss of the local
supervisor also ends the tunnel.

The remote vega shim consumes `VEGA_BROWSER_SHARE` before upstream endpoint
discovery. It submits a `Vega.command` request. The broker implements a finite
set of operations, including navigation, accessibility snapshots, mouse/form
actions, key input, scrolling and screenshots. It uses private target
attachments and fixed isolated-world functions for element actions. All user
strings enter Chrome as typed arguments. Caller-supplied CDP methods, scripts,
session IDs, file inputs, storage APIs, dev state operations and download
settings are outside the protocol. Only the remote shim writes screenshot
files. The local agent-browser adapter remains private.

Each origin is an exact HTTP(S) origin. Startup excludes the managed debugger,
reserved ports 9223/9224 and adapter ports in the local binding registries. A persistent Fetch session checks requests and
redirects, bypasses service workers and disables cache. It rejects attachment
and non-HTML document responses. Before delivering HTML, the broker fulfills
the response with an additional sandboxed CSP restricting connections to
approved hosts and ports, including their WebSocket counterparts. The policy
also excludes frames, new workers, popups and downloads. Chrome's unmodified
debugger independently rejects page-originated WebSockets. The implementation
bounds intercepted HTML bodies and ends the share on guard failure.

The allowlist design trusts the selected applications and their backends.
Chrome's request interception applies to the share's page targets. It does
not mediate server-side requests by an approved app, WebRTC traffic, or the
independent background activity of workers already registered by local tabs.
Speculation-rules prerender targets have not been verified against the page
interceptor. The supervisor does not auto-attach to such targets.
Applications that need iframes, new workers, or document downloads cannot use
those features in shared tabs. Share actions support CSS selectors and
snapshot refs; keyboard shortcuts that invoke browser UI are excluded.

Run the two disposable fixtures with Node 22+:

```sh
nice -n 10 node plugins/browser/tests/share-origins.mjs "<CfT executable>"
nice -n 10 node plugins/browser/tests/dev-groups.mjs target/debug/vega-browser --share
```

Build the browser plugin and the agent-browser shim first for the second
fixture. It starts its own loopback sshd with temporary keys, two projects,
an approved app and a denied listener. It exercises the real reverse forward
and verifies group/context isolation, denied requests, WebSocket restrictions,
stop, TTL and loss of the disposable browser.
