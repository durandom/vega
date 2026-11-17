---
name: vega-service
description: Registers host-local service titles, descriptions and URLs for the Services panel. Use when exposing a development service to agents or naming a listening port.
---

# vega service

Run from the owning project checkout on the service's host; this never starts services or tunnels.

- `vega service register --port <port> --title "<title>" [--description "<text>"] [--url <url>] [--pid <pid>] [--also-forward <port>]`
  replaces that port's metadata. PID defaults to its unique listener owner; use
  `--pid` when multiple processes own the port. An explicit PID must own the listener.
- `vega service` or `vega service ls` lists live registrations; `ls --json`
  includes descriptions and process identity. `rm --port <port>` removes only
  metadata and succeeds when already absent. All commands accept `--json` and `--help`.
- State is local to each host in `$XDG_STATE_HOME/vega/services/`, default
  `~/.local/state/vega/services/`. Dead, zombie or reused PIDs are pruned on list
  and panel Refresh; restarting a service requires registering again.
- URLs must be HTTP(S) loopback URLs with the registered port. Paths, queries
  and fragments survive SSH forwarding. Omit URL to use protocol detection.
- Refresh the Services panel to read each machine's metadata. Titles and
  descriptions replace process-only labels. Registrations persist canonical project
  and cwd; only that project's scope shows them. Re-register old entries from the
  owning checkout to persist ownership. Legacy PID/cwd attribution still works;
  unresolved owners appear in one labelled global group.
- Repeat `--also-forward <port>` for dependent loopback ports (for example 5320
  plus 8367). One Forward/Stop controls the entire bundle. Registered services
  always use matching local port numbers, preserving URLs baked into web pages.
  A local port conflict fails the whole action; stop its owner and retry. Extra
  ports may belong to different processes; the primary PID owns registration lifetime.
- Agent runtimes are hidden by default; **Show agent ports** reveals them.
  Registered ports remain visible within their project. Browser/agent-browser
  listeners are excluded from Services (see Browsers). Forward uses private-host SSH `-L` to
  loopback only; Stop or panel close ends its tunnel.

Spire on ganymede (after installing the CLI there):
`vega service register --port 5187 --pid "$(cat ~/spellkave-evidence/spire/spire.pid)" --title Spire --description "Spellkave evidence browser" --url http://127.0.0.1:5187`
