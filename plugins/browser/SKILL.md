---
name: vega-browser
description: Browses web pages with agent-browser through named sessions, on shared Chrome for Testing for headed development, on the global or VPN Chromium for login sessions, or in an isolated browser for headless tests. Lists and closes sessions by owner. Use when an agent needs a browser, when agent-browser refused a call for a missing session name, to access corporate pages through puck's VPN, or to see which agent owns which browser.
---

# vega browser

Name every session with `--session <project>-<task>` or `AGENT_BROWSER_SESSION`.
Use letters, numbers, hyphens and underscores.
Load `agent-browser skills get core` before driving pages. See `vega browser --help`.

## Headed development

```sh
vega browser dev --session <name> open <url>
agent-browser --session <name> snapshot -i
agent-browser --session <name> close
```

Ordinary `--headed` launches and default headed `vega review start <local-url>`
use shared Chrome for Testing. Worktrees share their Git project's context and
window. Reuse the printed review name from that project. Use `vega review stop`
to close the review tab and inbox.

Enable **Allow in Incognito** for **Vega Dev Tab Groups** in the managed CfT
`chrome://extensions` page. Set `VEGA_BROWSER_MARK=0` before opening for unmarked tabs.
Use a fresh name and `--headed false` for isolated tests. Explicit endpoints and
existing legacy bindings stay on the upstream route.

## Share a live dev project

```sh
vega browser dev share myproject --to testhost --origin http://127.0.0.1:8790 --ttl 30m
vega browser dev share status
vega browser dev share --stop myproject
```

Repeat `--origin` for trusted app origins. On the destination, export the
printed `VEGA_BROWSER_SHARE` capability and use the vega shim, e.g.
`agent-browser snapshot -i` and `agent-browser click @e1`. Keep the capability
private. Default TTL is eight hours.

## Login profiles

Each session uses only tabs it created. Closing the last tab opens a blank
replacement. Closing the session releases all its tabs.

- Global Chromium on 9223 retains logins. Use
  `vega browser ab --session <name> tab new <url>`.
- VPN Chromium uses puck's SOCKS tunnel and a pinned corp CA. Run
  `vega browser vpn start`, then
  `vega browser ab --profile vpn --session <name> tab new <url>`.
  `vega browser vpn status` reports CA trust and makes one corp HTTPS probe.
- `ab --session <name> close` disconnects. Only the user signs in.
  Never read or copy credentials, cookies or tokens.

For bot blockers, `vega browser launch <url> --session <name>` starts throwaway
Chromium. Use `vega browser cleanup --session <name>` afterward.
`vega browser status` checks pinned agent-browser and CfT dependencies.

## Session cleanup

`vega browser ls --json` lists sessions and owners. GC acts on confirmed dead
local owners. Use `vega browser gc --dry-run` to inspect legacy entries. For
one reviewed ownerless entry, run
`vega browser disconnect <session> --identity <identity>`. Close disconnects
an attached daemon and stops a browser the daemon launched.
