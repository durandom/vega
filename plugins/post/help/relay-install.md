Installs a login starter: launchd user agent on macOS, systemd user service on Linux.
It invokes `vega-post relay start` once; vegad detaches, keeps its lock and self-upgrades.
No herdr-app window or herdr binary is required. It does not start or restart hc cores.

Also writes/links the herdr startup hook when herdr is installed (TUI stays supported).
Pins broker, host, herdr socket and Dolt env in ~/.local/state/vega/vegad.env.
Always pins the resolved VEGA_POST_HOST, preserving an existing vegad.env host unless
explicitly overridden. When a broker is configured, warns if the registry cannot verify
that host (first install and unavailable registry included). Check this warning before
continuing to use the relay. If the hostname differs from the registry host (for example
io-wsl versus io), install with VEGA_POST_HOST=io; hc machine names must equal that host.
The login job preserves HOME, XDG directories and a PATH without agent shims.
Toolbox installations invoke the starter via toolbox run in the installing container.
Older supervising vega.post-relay units are removed; the new login job is vega.vegad-start
(macOS) or vega-vegad-start.service (Linux). Requires a logged-in user service manager.

Examples:
  vega post relay install
  VEGA_POST_HOST=io vega post relay install
  vega post relay status --json
  vega config infra

Use `vega post relay uninstall` to remove the login job and hook and stop vegad.
`relay stop` stops vegad until another explicit start, UI startup or login.
