---
name: vega
description: Talks to and coordinates with other coding agents on this machine, and loads the guides of the vega agent tools. Use when asked to ask, tell, inform, delegate to, hand over to, or check with another agent, session or pane; when a message ends with a [vega-mux] footer; or when the user mentions vega.
---

# vega

The installed `vega` binary holds the version-matched guides, one per plugin. Load the
one you need before the first vega command:

- Called with an argument NAME (e.g. `/vega mux`): run `vega skills NAME`.
- A message with a `[vega-mux]` footer: run `vega skills mux`.
- A `[vega-post]` pointer: run `vega skills post`.
- Otherwise: run `vega skills` for the index, then `vega skills NAME` for the plugin that fits.

To reach a project's coordinator on any machine: `vega post send --uac <project> "<text>"`.
Which agents and machines are reachable is what `vega post` and `vega uac` (the registry) say,
not `herdr machine list` (only the machines this one saved).

If `vega` is not on PATH or loading a guide fails, report the exact error and stop.
Do not guess commands. Other vega errors are structured: follow their hint (e.g. `usage`,
`agent_not_idle`) and go on.
