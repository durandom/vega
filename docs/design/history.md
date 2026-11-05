# Central event history (vega-lhc.249)

NATS is the record. Agent transcripts stay in agentsview; history includes session
identifiers when known so analysis can join them. No local history archive or new
daemon is introduced. This first stage captures spawn, mux send, acknowledged mail
publishes, brokerless tool delivery and observed agent/link/UAC lineage changes.
Land/deploy outcomes are also captured; the AXI reader/export and optional
metadata-only OTLP exporter are vega-lhc.252. OTLP is off: no exporter runs yet.

## Storage and authority

`VEGA_HISTORY` is a file-backed Limits stream on `vega.history.>`, with no age
expiry, a 10 GiB cap and `discard=new`. All host keys may **publish only** to
`vega.history.<host>.>`; they get no history read or management rights. The last
subject token is an event ID. Inbox-only hosts (including deimos) also publish
their own history, including .248 local deliveries. Their private-host mail still
uses quarantine and explicit pair grants; no additional mail rights are granted.
A full host receiving granted quarantine mail
can record its acknowledged inbox publish under its own history subject.

The 10 GiB history capacity is shared, with no per-host quota. Any host key can
fill it using its own subjects; `discard=new` then rejects history from every
host until an operator expands capacity or otherwise intervenes. Subject ACLs
prevent cross-host publication, not this shared-capacity denial of service.

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
Receipts publish in ID filename order, so stream sequence is arrival order, not
event or causal order; use timestamps and lineage chain IDs when reconstructing it.

- `spawn`: captured after the start attempt in `plugins/mux/src/main.rs`, including
  full brief, parent, kind/tier/model, cwd, requested branch/base, target host,
  session when available and whether startup succeeded. A dialog or failed start
  is recorded as an attempt, not falsely reported as a running agent. Resume is
  part of that attempt, not a second spawn. No environment values are recorded.
- `mux.send`: captured after successful post/paste, including full text, target
  and transport. A mux send through post also has a `mail.sent` event; these are
  distinct operation kinds, not two deliveries.
- `mail.sent`: every acknowledged publish through post's shared mail helper
  (send/done, ask, tool, bounce, Uber, granted quarantine and PagerDuty) includes subject,
  complete headers and full body. The original inbox archive remains intact.
  Its ID derives from host, subject and the mail's Nats-Msg-Id.
- `mail.local`: confirmed brokerless tool delivery includes full source text.
  The actual one-line paste may be truncated by the existing delivery policy.
  The inbox-only local-send path captures confirmed local delivery too.
- `lineage.start/change/end`: successful herdr snapshots feeding registry
  reconciliation send agent/link/UAC values to the worker before live KV removal.
  `end` retains the full last value. Failed herdr snapshots abort reconciliation;
  they never mean that every agent ended. Timestamps are observation times, not
  exact process start/exit times. Changes between reconciles may be missed.
  Inbox-only hosts do not reconcile registry entries, so this stage has no
  snapshot-based lineage events for them; their spawn/mux/local-mail capture runs.

- `land.phase` and `land`: each `vega uac land` attempt/resume records phase
  durations, source branch/head, integrated commit, bead, check and gate evidence,
  install result, unresolved conflict paths and changelog renumberings. A stable
  repository/bead `operation` links resumes; `attempt` links phases to their outcome.
  Durations cover work in this invocation, not time waiting between resumes.
  Phases that reuse completed work may have near-zero durations. A failing check
  or gate phase is `not_passed`, with its error and actual exit code when known;
  prior gate evidence remains separately available. Phase receipts are buffered
  until the attempt returns; a crash can lose that attempt's phase events.
  Preflight failures emit a `land` outcome with identifiers known at that point.
  Read-only `--status`, help and rejected command syntax emit no events.
- `deploy`: each `vega deploy apply` outcome includes the requested branch/machines,
  total duration and per-machine duration, SSH success/failure, full checked-out
  commit when known, install/config success, relay/override/config summaries and
  raw `unit,event` evidence rows from both install and config. The commit is
  captured before install so install failures retain it; checkout failures leave
  it null. Missing unit evidence is an empty list, not a claim of unit success.
  Optional config failures remain nonfatal to deployment and are recorded as such.
  Status/help emit nothing; apply preflight errors emit a failed outcome.

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
An unset worker also logs rejected capture. A parseable receipt rejected during
publication (for example by a payload limit or changed ACL) remains pending and
stops that drain pass; later receipts can wait behind it until the operator fixes
the broker configuration. Such publish failures are logged and retried, not
quarantined as corrupt data.

Unreadable, malformed or invalid-envelope receipts are renamed with a unique
`.corrupt-<id>` suffix and their location is logged once. Their bytes are preserved
for operator inspection; they no longer count as pending receipts or block later
publications. If permissions prevent quarantine, the worker skips that receipt,
logs its path once and retries the rename on later passes. Quarantined evidence
is never automatically deleted; operators should inspect it and repair the cause.
