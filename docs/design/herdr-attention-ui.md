# Herdr attention UI research (retired)

The TUI popup/dashboard proposal is retired by decision vega-0h4c.11. The old
implementation and research are preserved at git tag `herdr-tui-final`.

The current attention surface is the herdr-app inbox, sidebar and project window;
see [herdr-app-ui.md](herdr-app-ui.md). Shared decision, waiting, note and badge
state remains in `crates/vega-herdr`, `plugins/post` and the registry. The legacy
herdr transport remains available for active panes on puck; it does not install
TUI plugins or own their config.
