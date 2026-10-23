# vega open

A herdr plugin for humans: select a file path in a pane, press the bound key, and open it, the workspace or the git
diff in VS Code, MarkEdit or Finder. A selected pane id jumps to that pane, also on another machine. herdr runs it
from this directory (`herdr-plugin.toml`, `link-herdr.sh`).

Commands: the herdr action `vega.open.selection`; `vega open resolve <text>` to check a path.

Design: [repo-layout.md](../../docs/design/repo-layout.md) (plugin layout).

Agent guide: `vega skills open`
