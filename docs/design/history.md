# Central event history (vega-lhc.249)

NATS is the record. Agent transcripts stay in agentsview; history includes session
identifiers when known so analysis can join them. No local history archive or new
daemon is introduced. This first stage captures spawn, mux send, acknowledged mail
publishes, brokerless tool delivery and observed agent/link/UAC lineage changes.
Land/deploy events are tracked in vega-lhc.251; the AXI reader/export and optional
metadata-only OTLP exporter are vega-lhc.252. OTLP is off: no exporter runs yet.

## Storage and authority

`VEGA_HISTORY` is a file-backed Limits stream on `vega.history.>`, with no age
expiry, a 10 GiB cap and `discard=new`. Full host keys may **publish only** to
`vega.history.<host>.>`; they get no history read or management rights. The last
subject token is an event ID. Inbox-only hosts (including deimos) get no publish
right and enqueue nothing, including their .248 local deliveries. Capturing those
requires a later Slayer decision. A full host receiving granted quarantine mail
can record its acknowledged inbox publish under its own history subject.

The viewer principal may read history via stream info and `STREAM.MSG.GET`, using
`_INBOX_viewer` as its reply prefix. No consumer creation is granted: that could
create push consumers publishing past ACLs. The provisioning admin can inspect
stream info but cannot read events, delete or purge them. No wildcard update right
is added. The future reader must use the viewer key, not an ordinary host key.

Inbox streams now default to `max_age=0` and `max_bytes=10737418240` (10 GiB).
Existing streams change only with explicit provisioning below. Their existing
`discard` policy is preserved: an inbox using `discard=old` can still drop old mail
at its size cap. The permanent event stream refuses new events at its cap instead
of dropping accepted history. **Finite capacity is not a promise of lossless
capture forever.** Broker storage expansion, cap alerts/panels and backups belong
to the orrery rollout. Provision enough space for all inbox reservations, history,
registry and catalog before running the update (roughly 100 GB estate plan).

## One-time operator procedure

1. Raise broker storage capacity and enable alerts/backups through orrery.
2. Render the normal host configuration with `--admin <key> --create history
   --create archive`, retaining its existing inbox-only/viewer/grant flags; reload.
3. Run `vega post provision history` with the admin seed. Run
   `vega post provision <host>... --archive` for the existing inboxes.
4. Regenerate without both temporary `--create` values and reload, including after
   any error. Deploy updated host binaries after the stream and ACLs exist.

`--create history` grants only creation of VEGA_HISTORY; `--create archive` grants
UPDATE of the explicitly listed inbox streams. As with registry TTL migration,
the broker cannot restrict UPDATE to two fields: a temporary admin could enable
republishing. Remove the grant promptly. The provisioner preserves unknown fields
and raw JSON values, changing only max_age/max_bytes; an already migrated inbox
is a no-op. Missing inboxes are errors; create them normally first. Ordinary
`provision <host>` never silently updates an existing stream. History provisioning
checks an existing stream against its retention/subject contract and refuses an
incompatible one rather than changing or deleting it.

## Event envelope and capture points

Each JSON event has `schema:1`, a stable `id`, `ts` (Unix milliseconds), recording
`host`, `kind`, nullable `project`, `bead`, `pane`, `session`, and `data`.
Unknown identifiers are null; a task label is not assumed to be a bead ID.

- `spawn`: captured after the start attempt in `plugins/mux/src/main.rs`, including
  full brief, parent, kind/tier/model, cwd, requested branch/base, target host,
  session when available and whether startup succeeded. A dialog or failed start
  is recorded as an attempt, not falsely reported as a running agent. Resume is
  part of that attempt, not a second spawn. No environment values are recorded.
- `mux.send`: captured after successful post/paste, including full text, target
  and transport. A mux send through post also has a `mail.sent` event; these are
  distinct operation kinds, not two deliveries.
- `mail.sent`: every acknowledged publish through post's shared mail helper
  (send/done, ask, tool, bounce, Uber and granted quarantine) includes subject,
  complete headers and full body. The original inbox archive remains intact.
  Its ID derives from host, subject and the mail's Nats-Msg-Id.
- `mail.local`: confirmed brokerless tool delivery includes full source text.
  The actual one-line paste may be truncated by the existing delivery policy.
  The inbox-only local-send path calls capture too, but policy excludes it.
- `lineage.start/change/end`: successful herdr snapshots feeding registry
  reconciliation send agent/link/UAC values to the worker before live KV removal.
  `end` retains the full last value. Failed herdr snapshots abort reconciliation;
  they never mean that every agent ended. Timestamps are observation times, not
  exact process start/exit times. Changes between reconciles may be missed.

A command crash between its action and capture can miss an event; the operation
and history are not one transaction. Historical inbox content is not backfilled
into VEGA_HISTORY. Spawn/session fields depend on what herdr knows at capture.

## Bounded outbox, recovery and loop isolation

`crates/vega-herdr/src/history.rs` is the shared receipt writer. CLI capture writes
one atomic receipt to `$XDG_STATE_HOME/vega/history-outbox/<host>` (normally
`~/.local/state/vega/history-outbox/<host>`). This is a transient retry queue, **not** an
archive: acknowledged receipts are removed. It holds at most 256 events of at
most 256 KiB each (64 MiB payload budget). A nonblocking file lock serializes
producers. Oversize, full, busy or failed writes emit diagnostics and reject new
events; existing receipts are never evicted. Capture never changes command exit
status. Disk I/O occurs on CLI capture, not on the vegad live loop.

vegad starts one history thread with a bounded 64-item channel. Its live loop
only submits events/snapshots with `try_send`; full/disconnected queues log a
failure. The worker owns serialization, filesystem I/O, its broker connection,
publication retries and the mutable `lineage.state` checkpoint. Delivery and live
registry readers never consult history. The checkpoint keeps current values and
per-key event-chain identities, so daemon restarts do not replay unchanged
lineage; removed values remain in the central event. A malformed checkpoint
stops this worker with a diagnostic rather than inventing lifecycle events.

Publication waits for the JetStream ack, then removes the receipt. Every event
has a unique subject and sends `Nats-Expected-Last-Subject-Sequence: 0` plus an
expected stream and Nats-Msg-Id. If a crash loses an ack or leaves a receipt,
republication cannot append a second event even after the normal deduplication
window expires: the subject already exists. This holds while central events are
retained; an operator deleting/recreating the stream also deletes that evidence.
The worker retries broker failures independently of mail delivery. Bounded queues
cannot retain an unlimited outage; rejected capture is visible in diagnostics.
