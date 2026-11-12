Resumes a lost agent in place, or reopens a closed spawn.

In place: after a reboot hc restores a kept terminal with a login shell. When the
target's terminal is live, at an idle shell, and its record names a session, reopen
types `cd <cwd> && <recorded env> <launch argv> <resume>` into it: same terminal,
address, labels and parent. vegad offers this once per boot (see below).

Closed: respawns in its recorded directory and a new workspace, with the same name,
kind, parent and headed/headless mode. The directory must still exist. Close keeps
the spawn record and refreshes the session id before closing the pane.

Resume: claude --resume <id>, codex resume <id>, pi --session <id> (pi: the session
file its launch started). Other kinds: unsupported_kind. No saved session:
no_session; missing directory: cwd_gone; a running name: agent_running. Older
spawns have no record; `vega uac claim` records the UAC's own terminal. This
machine only. Extra harness args are retained; explicit --env values are not
stored or replayed.

After a boot vegad finds lost agents once and, per ~/.config/vega/resume.toml
`mode = "offer" | "auto" | "off"` (default offer), leaves one note with this
command or resumes them itself.

Examples:
  vega mux reopen fix-login
  vega mux reopen t1b t2c
