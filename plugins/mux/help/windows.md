This native target subset is called by a Unix coordinator over hc.
Policy/model selection and spawn orchestration run on that coordinator.
Windows supports Codex and Claude with direct native .exe argv; no shell.
Worktrees and local app/TUI lifecycle commands are unavailable.
Repository trust comes only from the target's existing trust configuration.

Examples:
  vega mux prepare --kind codex --cwd 'G:\src\repo'
  vega mux prepare --kind claude --cwd 'G:\src\repo with spaces'
