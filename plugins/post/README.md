# vega post

Agent mail through a NATS broker, also across machines. A message is stored in the broker, not typed into the
target pane; vegad, the relay on each host, types a one-line pointer into the pane and the target pulls its mail.
This keeps long messages out of busy panes and lets agents on different machines talk.

A pointer never joins text the user is typing (the draft guard): vegad reads the input box of Claude Code,
Codex and pi and holds the pointer while it has a draft; a draft left 5 minutes is stashed and typed back
after the pointer, or, when it cannot be typed back, keeps holding with one notification. A screen with no
known input box holds the pointer until it has been unchanged for 20 s (one notification after 5 busy
minutes). `vega mux send --now` pastes without the guard. See
[Draft guard](../../docs/design/vegad.md).

Reply commands carry a timestamped `--re` token. Replies within 120 seconds count toward the eight-hop
loop limit; later replies reset to hop 1 on the same thread, so long task threads can continue. At the
limit, stop automatic replies and tell the user; resume only with a substantive reply after work.

A broker requires `VEGA_NATS_URL` or `~/.config/vega/nats.url` (first line,
under `$XDG_CONFIG_HOME` when set). Without it, post returns `broker_unreachable`
and vegad enters local mode only after a completed identity migration.
Unmigrated or failed-cutover hosts remain in identity quiesce; see
[Local mode](../../docs/design/vegad.md#local-mode-vega-lhc118).
No estate endpoint is assumed.

Commands: `vega post`, `send`, `inbox`, `done`, `ask`, `log`, `notes`, `relay`.

Design: [vegad.md](../../docs/design/vegad.md), [vega-mux-broker-spike.md](../../docs/design/vega-mux-broker-spike.md).

Agent guide: `vega skills post`

Project identity uses the flag-day v2 protocol; deployment and exact-grant
requirements are in [the registry cutover handoff](../../docs/design/project-registry-cutover.md).
Legacy integer UAC status timestamps migrate without changing their time.
Inbox-only hosts migrate local project records without registry publication.
For separate Git directories, set `core.worktree` to the actual checkout before
generating mappings from the Git directory. The runbook also describes an
opt-in startup/file-rollback rehearsal on state copies without broker writes.
A host that missed the flag day rejoins with a re-audited bundle that completed
hosts accept without re-migrating ([Late host rejoin](../../docs/design/project-registry-cutover.md#late-host-rejoin)).
`vega post identity verify --host <host>` checks only that host's planned
projection (plain `verify` is the flag-day check). Identity subcommands load
`vegad.env`, so host token, relay lock and broker match vegad's.
