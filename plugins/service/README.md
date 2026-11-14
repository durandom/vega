# vega service

Host-local metadata for listening TCP services. `register --port <port> --title
"<title>"` records title, optional description and loopback HTTP(S) URL;
`ls` lists live registrations and `rm --port <port>` removes metadata.

This dedicated plugin owns service registration, independent of terminal lifecycle
or config deployment. Its Rust entry point embeds the self-contained Python 3
backend in [helper.py](../../config/herdr-app/mods/vega-services/helper.py), also
used by the Services panel on each scanned machine. The CLI does not need an
installed panel. It requires `ps` and `ss` (Linux) or `lsof` (macOS).

Entries are atomic JSON files keyed by port under `$XDG_STATE_HOME/vega/services`
(default `~/.local/state/vega/services`). A lock serializes mutations and pruning.
PID plus process start identity prevents metadata surviving process exit or PID
reuse. Registration must match a live listener; listing and panel Refresh prune
expired entries. JSON schema 1 has `services` entries with `port`, `pid`, `start`,
`title`, `description`, and `url`; the default list is compact TOON. Errors are
structured with usage exit 2 or runtime exit 1.

The panel keeps its project/space attribution and explicit SSH forwarding.
Metadata is matched by PID and port, never port alone. Recognized runtime
executables/package paths are hidden until **Show agent ports** is selected;
registered services remain visible. No deployment, service start, forwarding or
browser action happens during registration.

Agent guide: `vega skills service`.
