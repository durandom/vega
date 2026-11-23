vega browser: named sessions for shared headed development, Chromium profiles and isolated tests

Usage: vega browser <command>

Commands:
  vpn start|stop|status|trust [--json]
                              Dedicated headed VPN Chromium via SSH SOCKS to puck.
                              Requires autossh and BatchMode SSH access. Status makes
                              one bounded corp HTTPS probe; stop preserves the profile.
  ab --profile vpn --session <name> <args>
                              Use session-owned tabs in the running VPN profile (start it first).
  install                     Converge the npm global pin or print the required mise update.
                              Install pinned Chrome for Testing once the agent-browser pin is met.
  status [--json]             Read-only current/differs/missing and Chrome cache status.
  doctor [--json]             Same dependency status as status.
  ab --session <name> <args>  agent-browser on the global Chromium (127.0.0.1:9223).
                              Lists and operates on tabs created by this session.
                              Refuses connect/--cdp/--auto-connect/--provider/
                              --executable-path; requires a session name.
  chromium [url]              Attach to or start the global Chromium (idempotent);
                              the url opens only when it starts.
  dev --session <name> <args>  Use shared headed Chrome for Testing for this
                              Git project. Use open <url> to start and close
                              to release the session.
  dev state register <origin> Register a live dev project's HTTP(S) origin.
  dev state save|restore      Save or restore this Git project's dev state.
  dev share <project> --to <ssh-host> --origin <http(s)-origin> [--ttl 8h]
                              Share new grouped tabs in a live project context.
                              Repeat --origin for trusted app origins. TTL accepts
                              seconds, minutes or hours, up to 24h. Run the
                              printed command with the remote vega shim.
  dev share status            List destinations and remaining share lifetimes.
  dev share --stop <project>  Revoke the share and close its tabs.
  launch <url> [--session s] [--port p]
                              Throwaway ungoogled Chromium (temp profile, CDP on
                              127.0.0.1) for bot blockers. Defaults: session
                              `chromium`, port 9222; never the global port.
  cleanup [--session s] [--port p]
                              Close the fallback browser, verify the port closed,
                              delete the temp profile. Refuses the global port and
                              profile before touching anything.
  ls [--json]                 Sessions merged from agent-browser's list, the owner
                              records and the browser processes. States: live,
                              stale (owner pane gone or no call for >1 day),
                              unowned (no record; Playwright leftovers), closed.
                              Browsers: pid, kind, headed, age; the global one
                              marked `global`.
  gc                          Close sessions with a confirmed dead local owner.
  gc --dry-run                Read-only legacy socket inventory with owner status
                              and identity for ownerless entries. Idle age alone
                              gives no evidence of owner liveness.
  disconnect <session> --identity <identity>
                              Close one reviewed ownerless legacy daemon using
                              its current identity from gc --dry-run. Attached
                              browsers stay running.
  close <session>             Close a named session on explicit request.

`ls --json` (schema 1): {schema, sessions: [{session, state, colour, pane,
host, project, cwd, age, idle, headed, pid}], browsers: [{pid, kind, headed,
age, port, user_data_dir, global}], error}. `pane` is null outside a pane;
`headed`/`pid` come from the browser whose `--user-data-dir` ends in
`<session>-profile` (agent-browser's own headed launches), else null. `colour`
is the session's colour (below).

Shared headed development:

  vega browser dev --session ui open http://localhost:3000
  agent-browser --session ui snapshot -i
  agent-browser --session ui tab new http://localhost:3000/settings
  agent-browser --session ui tab list
  agent-browser --session ui tab t1
  agent-browser --session ui tab close t2
  agent-browser --session ui close

The shim routes ordinary `--headed` launches to dev and remembers their
names. Explicit CDP/launch settings and existing legacy bindings stay on
the upstream route. Default headed local reviews use dev as well. Git worktrees of one project share cookies and a window. Each
session can select and close only its own tabs. Use a fresh name for an
isolated headless browser. The managed profile is
`${XDG_DATA_HOME:-~/.local/share}/vega/browser/dev-cft`.

Dev sessions use the pinned agent-browser runtime for page commands such
as `open`, `snapshot`, `click`, `eval`, `get` and `screenshot`. Browser
and endpoint overrides are rejected. After closing a tab in Chrome or
restarting CfT, a stale binding reports an error. Release it with the
session's `close` command before reusing the name.

Save and restore state from the project's Git checkout:

  vega browser dev state register http://localhost:3000
  vega browser dev state save
  vega browser dev state restore

Register each origin in the live project before saving. Registrations survive
navigation and tab closure while the context lives. Save before closing the last
session. Restore with the project closed, then open a named dev session. Snapshots
contain host-only cookies for registered hosts and first-party localStorage for
registered origins. Domain and partitioned cookies on those hosts make save fail.
Other browser storage is outside v1. Snapshots are owner-only files under
`${XDG_STATE_HOME:-~/.local/state}/vega/browser/dev-state`.

Every named agent-browser call (shim, ab, launch) upserts the owner record
`$XDG_STATE_HOME/vega/browser/sessions.json`:
{session: {pane, host, project, cwd, first_seen, last_seen}}. Explicitly
headed calls (--headed flag or truthy AGENT_BROWSER_HEADED) carry the
session's window mark: `--init-script <marks dir>/<session>.js`, a coloured
frame, a session tag and a `[session]` title prefix (aria-hidden). In 0.43,
registrations persist across later calls and cover `tab new` immediately.
`launch` never registers a mark. `VEGA_BROWSER_MARK=0` prevents registration;
set it before first use, or use a fresh session for unmarked work. The colour is the
owner project's `[symbols.project_colors]` colour in ~/.config/vega/theme.toml,
else a stable hash of the session name.

Environment: CDP_PORT (9223), CDP_PROFILE (~/.local/share/vega-chromium),
CDP_CHROMIUM (/Applications/Chromium.app/Contents/MacOS/Chromium),
VEGA_AGENT_BROWSER_BIN (the real agent-browser binary).

Design: docs/design/browser.md
Agent guide: vega skills browser

VPN defaults: SSH host puck, SOCKS 127.0.0.1:1080, CDP 127.0.0.1:9224,
profile ~/.local/share/vega-chromium-vpn. Optional config:
$XDG_CONFIG_HOME/vega/browser-vpn.json (default ~/.config/vega/browser-vpn.json):
{"host":"puck","socks_port":1080,"cdp_port":9224}
Start automatically imports the pinned corp root into this profile before launch;
`vpn trust` performs only that step. Requires a stopped profile for a new import.
Already trusted: no writes or SSH fetch. Status includes ca_sha256 and ca_trusted
(false = absent; null = unreadable/unsupported/conflicting metadata).
Optional ca_sha256: exactly 64 hex digits (case insensitive); default:
4678DB0942D0B9E732122507BD6CDC08D128AB832DA274AD7D46F8C3576BC925.
The public cert is read from the SSH host's System.keychain and checked before
any import. A mismatch fails; existing distrust/constraints or unknown store
versions fail without overwrite. Review those in the VPN certificate manager.
No macOS keychain writes or browser policies.
Optional profile must be an absolute dedicated vega-chromium-vpn* directory.
Stop before changing config. Full-profile SOCKS/remote DNS, no direct fallback;
no PAC or domain list. Only the user logs in. VPN uses the pinned Chrome for
Testing from vega browser install unless CDP_CHROMIUM selects another binary.
VPN status reports browser_version from the running browser.
Examples: vega browser vpn start; vega browser vpn status --json;
vega browser ab --profile vpn --session corp-vpn-task tab new https://gitlab.cee.redhat.com
vega browser vpn stop

For mise-managed agent-browser, install reports current or prints the mise command
to update its source configuration. For npm globals, use your normal login shell.
An older binary must resolve to that npm prefix's agent-browser package; a
foreign binary or mismatched VEGA_AGENT_BROWSER_BIN is refused, never shadowed.
The pinned Chrome for Testing build is installed under ~/.agent-browser/browsers.
Existing sessions continue using their already running daemon.
`vega config apply browser.agent-browser` runs the same installer; deploy's
config apply includes it. Examples: `vega browser status`, `vega browser install`.

0.43 session names allow letters/numbers, hyphens and underscores (no dots).
Tabs have stable ids (`t1`, `t2`, ...); use `tab` to list them. `--pin-tab` is
sticky and retains the bound tab when another tab becomes active; `--no-pin-tab`
clears it. Vega does not add either flag. `addinitscript` remains unsupported.
