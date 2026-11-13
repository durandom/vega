---
name: vega-window
description: Opens or raises independent herdr-app profile windows and starts missing terminals. Use when asked to open the status window or another applied profile.
---

# vega window

`vega window` lists profiles and whether they are applied.
`vega config apply herdr-app.profiles` writes profile homes without opening windows.
`vega window status` starts missing status terminals and opens or raises its window.
Profiles live in `config/herdr-app/profiles/<name>.toml`; local overrides use
`~/.config/vega/config/herdr-app/profiles/`. Each home is `~/.config/herdr-app-<name>`
(or under XDG_CONFIG_HOME). HERDR_APP_HOME in the calling shell is ignored.
Live terminals are reused by window and slot labels; closing the window leaves
programs running. Changing argv/machine does not replace an already-live slot.
The status window has top tabs: proteus, ganymede, optional io (btop), and tokens
(token-burn tui on ganymede). Missing io btop is skipped with a warning.
Profiles use [[tab]] name and one or two [[tab.terminals]] entries; two split
horizontally. Include vega-window-tabs in mods and a profile:tabs stack in the
layout. Flat [[terminals]] profiles remain supported. Tab switching keeps the
usual Ctrl-Tab, Cmd-Shift-[ / ], Cmd-1..9 and ctrl-s n/p/1..9 keys.
Tabs are layout groups, without shared core spaces or a sidebar.
New homes skip welcome; existing shared state is preserved. No terminal cleanup
is automatic. A partial launch can be retried without replacing successful slots.
Manage grouped tabs in the top bar: + (or leader-c) chooses a machine for a
plain shell; double-click a tab to rename, drag to reorder, × to hide without killing.
Config shows the applied repo → effective diff and offers promote/reset. Local edits persist in
`$XDG_CONFIG_HOME/vega/windows/<name>.toml` across opens and config apply.
`vega window promote <name> --repo <vega-checkout>` writes an uncommitted repo
profile (defaults to the current checkout). Review/commit, rebuild/install, apply
the new profile, then reset local edits.
`vega window show <name>` prints effective config, overlay and unified diff as JSON.
`add`, `rename`, `move`, `close`, `reset` expose the same edits; use `--help`.
