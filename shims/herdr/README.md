# herdr shim

Resolves the real `herdr` (`HERDR_BIN_PATH`, which herdr exports into its panes, then `PATH`) and replaces itself with
it via `exec`: argv, stdio, TTY, signals and exit code are herdr's own. It adds nothing yet; it is the hook point for
agent-only additions to herdr.
