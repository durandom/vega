vega browser: named agent-browser sessions — the global Chromium, throwaway fallbacks, owner records

Usage: vega browser <command>

Commands:
  ab --session <name> <args>  agent-browser on the global Chromium (127.0.0.1:9223).
                              Refuses connect/--cdp/--auto-connect/--provider/
                              --executable-path; requires a session name.
  chromium [url]              Attach to or start the global Chromium (idempotent);
                              the url opens only when it starts.
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
  gc                          Close the sessions of dead owner panes, prune records
                              of sessions that no longer exist. Never closes a
                              live owner's session or an unrecorded one; prints
                              everything it closes.
  close <session>             Close one session on explicit request.

`ls --json` (schema 1): {schema, sessions: [{session, state, pane, host,
project, cwd, age, idle, headed, pid}], browsers: [{pid, kind, headed, age,
port, user_data_dir, global}], error}. `pane` is null outside a pane;
`headed`/`pid` are null when agent-browser keeps the mapping private.

Every named agent-browser call (shim, ab, launch) upserts the owner record
`$XDG_STATE_HOME/vega/browser/sessions.json`:
{session: {pane, host, project, cwd, first_seen, last_seen}}.

Environment: CDP_PORT (9223), CDP_PROFILE (~/.local/share/vega-chromium),
CDP_CHROMIUM (/Applications/Chromium.app/Contents/MacOS/Chromium),
VEGA_AGENT_BROWSER_BIN (the real agent-browser binary).

Design: docs/design/browser.md
Agent guide: vega skills browser
