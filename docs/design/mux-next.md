# vega mux next: focus the longest-waiting agent, on any host

Bead vega-lhc.145. Replaced the dotfiles script `~/bin/herdr-next-done`, formerly behind prefix+d.
The command remains available; its key bindings were removed in vega-lhc.160.

## Problem

prefix+d ran `herdr-next-done`: the oldest `done` agent of `herdr agent list`, then `herdr agent focus`.
The Slayer reported that it never finds one and never worked across machines. Diagnosis (herdr
0.9.5 source and server log, notes on the bead):

- herdr's `done` is `idle` and not *seen* on the server. Seen is set by any focus, by any tab or
  workspace switch (every pane of the shown tab), by a completion in the foreground client's tab, by
  the client regaining terminal focus, by `working`/`blocked`, and by a server restart (seen is not
  persisted). Reads never set it; vega never sets it. Passing through a tab clears every pane in it.
- A `[[keys.command]]` runs on the server of the machine the client has selected. With another machine
  selected, the script sees only that machine's agents.
- No herdr API switches a client to another machine. Only input inside the client does that (sidebar,
  prefix+w, next/previous-agent keys, the navigator, a toast click). `herdr --machine M agent focus P`
  focuses P on M's server; the client stays where it is (as `vega_herdr::pane` notes).

## Shape

**Ledger (vegad, per host).** vegad already streams `pane.agent_status_changed` (plugins/post/src/
agents.rs). It adds `pane.focused`, and keeps `waiting`: pane -> {status, since}.

- Enters: the status becomes `done` (a completion the Slayer did not watch) or `blocked`. `since` =
  first entry (Unix ms). `done` -> `blocked` -> `done` keeps the first `since`.
- Leaves: `working`, `idle` after `blocked` (answered), the pane is focused (`pane.focused` for that
  pane), the pane is gone.
- herdr's later `done` -> `idle` (a tab visit, a restart) is ignored: only a focus of that pane counts as
  seen. That is the independence from herdr's flag.
- Saved to `$XDG_STATE_HOME/vega/waiting.json`, so a vegad restart keeps the queue. On a snapshot,
  panes no longer `done`/`blocked`/`idle` leave; a `done`/`blocked` pane not yet recorded enters with
  `since` = now.
- Poll mode (no stream): the same rules on status changes between polls, without `pane.focused`
  (a focus there clears only by `working`, or by `vega mux next` itself).

**Registry.** `<host>.agent.<pane>` gains `waiting: {status, since}` (absent when not waiting). vegad
reconciles every 15 s; vegad also reconciles at once when the ledger changes, so the queue trails
by about a second, not 15.

**`vega mux next [--status done,blocked]`** (default `done`):

1. This host: its ledger file, panes herdr still lists. Other hosts: the registry (fresh hosts only);
   without it, this host only, said on stderr.
2. Pick the waiting agent with the oldest `since` whose status is in `--status`.
3. Local: `pane.focus` over the socket (as `vega open`). vegad sees `pane.focused` and drops it.
4. Another host: map the host to its herdr machine (label or ssh target equal to the host token),
   `herdr --machine M agent focus P` (focuses it there; that host's vegad drops it), and show a toast
   on the local client: `next: <name> on <host>, switch with prefix+w`. The client cannot be switched
   for the Slayer.
5. None: toast `no done agent` (or `no blocked or done agent`), exit 0.

Output (AXI): one line `focused <pane> on <host> (<status> for <age>)`, or `none`. Errors on stderr with
a hint; exit 0 when nothing waits.

## Keys

Vega leaves prefix+d and prefix+shift+d to herdr's defaults: prefix+d is unbound and
prefix+shift+d is `close_workspace` (verified with `herdr --default-config`). Neither has a
vega override in config/herdr/config.toml.

The Slayer prefers herdr's goto overlay (prefix+g, then d), which lists done agents across
machines and switches the client to the selected machine and pane. `vega mux next` cannot
switch the client to another machine: remote focus only changes that machine's server state
and requires a separate client switch with prefix+w. That limitation is why both vega key
bindings were removed. `vega mux next` and `vega mux next --status blocked,done` remain commands.

## Not doing

- No herdr agent view (`agent.view.set`): it also reorders the Slayer's sidebar.
- No herdr patch for remote client switching (related upstream ask: vega-8m7, host-scoped last_pane).

## Tests

- Ledger transitions as a pure function (enter, keep `since`, leave on focus/working/answered/gone,
  ignore herdr's `done` -> `idle`).
- Pick: oldest across hosts, status filter, stale hosts skipped, local fallback.
- Host -> machine mapping (label, ssh target, `user@` stripped).
- `vega mux next` with a fake herdr: local focus, remote focus plus toast, none.
