Reopens a closed spawn in its recorded directory and a new workspace, with the
same name, kind, parent and headed/headless mode. Both --worktree and plain --cwd
spawns are supported; the directory must still exist. Close keeps the spawn
record and refreshes the session id from herdr before closing the pane.

Supported: claude --resume <id>, codex resume <id>. Other kinds return a structured
unsupported_kind error. No saved session: no_session; missing directory: cwd_gone;
a running name: agent_running. Older spawns have no record. This machine only.
Extra harness args are retained; explicit --env values are not stored or replayed.

This resumes the conversation without sending a new brief. `spawn --resume <pane>`
is different: it delivers an undelivered brief to an existing open pane.

Examples:
  vega mux reopen fix-login
  vega mux reopen w4C:p1
