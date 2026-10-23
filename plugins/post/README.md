# vega post

Agent mail through a NATS broker, also across machines. A message is stored in the broker, not typed into the
target pane; vegad, the relay on each host, types a one-line pointer into the pane and the target pulls its mail.
This keeps long messages out of busy panes and lets agents on different machines talk.

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
