Installs vegad's pinned env and the herdr TUI startup hook, and starts vegad. No login job:
the herdr-app vega mod starts vegad on the app host and, via `hc exec`, on connected hc machines.
A login job (vega.vegad-start plist, vega-vegad-start.service) from an earlier install is removed.

Also writes/links the herdr startup hook when herdr is installed (TUI stays supported).
Pins broker, host, herdr socket and Dolt env in ~/.local/state/vega/vegad.env; the mod's
remote start reads it, so the host pin applies there.
Always pins the resolved VEGA_POST_HOST, preserving an existing vegad.env host unless
explicitly overridden. When a broker is configured, warns if the registry cannot verify
that host (first install and unavailable registry included; an inbox-only host skips the check by design). Check this warning before
continuing to use the relay. If the hostname differs from the registry host (for example
io-wsl versus io), install with VEGA_POST_HOST=io; hc machine names must equal that host.
Toolbox installations make herdr's hook run the starter via toolbox run in the installing container.
Older supervising vega.post-relay units are removed too. It does not start or restart hc cores.

Examples:
  vega post relay install
  VEGA_POST_HOST=io vega post relay install
  vega post relay status --json
  vega config infra

Use `vega post relay uninstall` to remove the hook, any old login job and the env, and stop vegad.
`relay stop` stops vegad until the next start by herdr, the app mod or the board.
