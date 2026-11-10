---
name: vega-project
description: Reads and edits personal project display names, labels and saved sidebar filters. Use when configuring a project or focusing the sidebar by label or machine.
---

`vega project config [<project-id>] [--name <name>] [--label <label>]... [--unlabel <label>]... [--pin|--unpin] [--json]`.
Use the full normalized `vega.project-id` terminal label as the key. Empty name clears the override.
No ID lists all projects and views in one read; a missing file means empty preferences.
`vega project view [<name>] [--label <label>]... [--machine <machine>]... [--delete] [--json]`.
Filter flags replace a saved view; labels match any selected label, machines any selected machine,
and both dimensions must match. `all` is reserved. `local` means the app host.
Storage is `$XDG_CONFIG_HOME/vega/projects.toml` (normally `~/.config/vega/projects.toml`),
local to the app host; the registry and project repository are untouched.
