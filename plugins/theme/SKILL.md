---
name: vega-theme
description: Shows and sets the dark or light mode of every terminal tool (Claude Code, codex, pi, delta, gh-dash, openusage, herdr) from the OS appearance; headless hosts follow the control host. Use when a TUI shows the wrong colours, to switch all tools to dark or light, or to check theme drift.
---

# vega theme: one dark/light mode for every TUI

```
vega theme              # OS mode, wanted and applied mode, per tool: value, mode, drift
vega theme dark         # fix dark (also light); writes ~/.config/vega/theme.toml, applies
vega theme auto         # follow the OS (headless hosts: the control host's mode)
vega theme apply        # apply the due mode again (idempotent)
```

- vegad watches the OS appearance and applies every change itself, and types
  `/config theme=<mode>` into Claude panes once they are idle with an empty input.
  There is nothing to poll.
- Adapters change only their own key and skip tools that are not set up.
  `native` means the tool follows the terminal by itself (pi with a `light/dark` pair,
  herdr with `auto_switch`).
- herdr's `~/.config/herdr/config.toml` (sidebar, theme, keys, toasts) is rendered by
  `vega config` (unit `herdr`, see `vega skills config`), with colours, host labels and
  the sentinel icon from `[symbols]` in theme.toml. `vega theme herdr [--print|--apply]`
  is an alias for `vega config diff|print|apply herdr`.
- Per-tool names and switches live in `~/.config/vega/theme.toml`
  (`[tools.codex] light = ..., dark = ...`, `enabled = false`, `live_claude = false`).
- A headless host (no readable appearance, or `headless = true`) follows the mode the
  control host (`control`, default proteus) publishes in the registry.
