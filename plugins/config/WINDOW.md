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
The status POC uses three direct terminal blocks, with no spaces or sidebar.
New homes skip welcome; existing shared state is preserved. No terminal cleanup
is automatic. A partial launch can be retried without replacing successful slots.
