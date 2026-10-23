---
name: vega-open
description: Opens a file path selected in a herdr pane, the workspace or the git diff in VS Code, MarkEdit or Finder, and jumps to a selected herdr pane id. A human tool behind a herdr keybinding menu. Use only when the user asks how to open a selection, file, diff or pane from a herdr pane; agents normally do not call it.
---

# vega open: open selections in an app (human tool)

The user presses the herdr key bound to `vega.open.selection` and picks from a menu. Do not open apps for the user unasked.

`vega open resolve <text> [--cwd <dir>]` shows which file a path such as `src/main.rs:12` resolves to and which apps fit. It has no side effects.

A selected herdr pane id jumps to that pane: `w1:p2` is looked up here first, then on the herdr machines; `io:w1:p2` names the machine. When you tell the user about a pane on another machine, write it machine-qualified (`io:w1:p2`), so selecting it carries the machine.
