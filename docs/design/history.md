# Central event history (vega-lhc.249)

NATS is the record. Agent transcripts stay in agentsview; history includes session
identifiers when known so analysis can join them. No local history archive is
introduced. This first stage captures spawn, mux send, acknowledged mail
publishes, brokerless tool delivery and observed agent/link/UAC lineage changes.
Land/deploy events are tracked in vega-lhc.251; the AXI reader/export and optional
metadata-only OTLP exporter are vega-lhc.252. Capacity gauges are exported by vegad when
the shared OTLP configuration is enabled; metadata log export is described by .252. Both
exporters are off by default.

## Capacity gauges

vegad polls `VEGA_INBOX_<host>` and `VEGA_HISTORY` stream info every five minutes and exports
the stream's stored bytes, message count and configured `max_bytes` as OTLP metrics. The inbox
stream info right is part of the host's normal permissions. Generated host grants do not include
VEGA_HISTORY stream info, so only the inbox gauges are available by default. If an operator grants
VEGA_HISTORY info separately, vegad exports those gauges too; it never reads event messages. Such
a broker grant requires coordination with orrery and the Slayer. These gauges are optional and
off by default. Capacity metrics use the shared
`$XDG_CONFIG_HOME/vega/otlp.toml` (normally `~/.config/vega/otlp.toml`):

```toml
enabled = true
endpoint = "http://otel.example:4318"
# timeout_ms = 5000
```

The endpoint is an OTLP/HTTP base URL. The metrics request appends `/v1/metrics`; stream identity
and host are metric attributes. A missing file or `enabled = false` disables exporters. A denied
or missing VEGA_HISTORY stream produces no history gauges and is logged once; the own inbox gauges
continue independently. The .252 history metadata exporter uses this shared config too.

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
is added. The reader uses the viewer key, not an ordinary host key.

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
  prior gate evidence remains separately available. Install evidence distinguishes
  not run, failed with a measured exit, uncertain, measured success and UAC
  attestation; unmeasured states never imply exit 0. Phase receipts are buffered
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

## Analysis reader and export (vega-lhc.252)

`vega history` shows local publication evidence and reader discovery.
`vega history status` needs no broker connection, viewer key or admin seed. It
reads only this host's outbox and atomic `publish.state`: last real PubAck receipt
time in Unix milliseconds, stream sequence, pending receipt count, and last
publication error. Missing evidence is null, never inferred from pending files.
A retry rejected by the unique-subject compare-and-set removes an already-stored
receipt without inventing an ack sequence/time. Broker errors retain the last ack;
a later successful publication clears the publication error. This is local evidence,
not proof of current broker health or remote-host activity. Status I/O failure is
diagnostic and cannot prevent acknowledged receipt removal. Corrupt status fails
the status command explicitly; it is not reported as zero pending or a fresh ack.

The analysis commands use a separately supplied viewer seed with `_INBOX_viewer`:

```sh
vega history read --viewer-key /secure/viewer.nk --project <project-id>
vega history export --viewer-key /secure/viewer.nk --bead vega-lhc.252 > history.jsonl
vega history read --viewer-key /secure/viewer.nk --host proteus --since 1793894400000
```

The key path can also be set by `VEGA_HISTORY_VIEWER_KEY` or the first line of
`$XDG_CONFIG_HOME/vega/history.viewer-key` (`~/.config/vega/history.viewer-key`).
Broker URL uses `VEGA_NATS_URL` or the ordinary `vega/nats.url`. There is no fallback
to the ordinary host seed or admin seed. Existing viewer grants suffice: only
stream INFO and MSG.GET, no consumer creation, updates, deletes or ack privileges.
Host and admin principals cannot read the history through this reader.

`read` lists sequence, timestamp, recording host, kind and event ID, first 100
matches by ascending arrival sequence. It reports the total matching count and
snapshot end sequence. `--limit <positive-count>` raises or lowers the displayed
limit. `export` streams full schema-1 JSON envelopes as JSON lines, unlimited by
default; its emitted/total count, last emitted sequence and snapshot end go to
stderr, including an explicit zero on an empty result. Resume a limited export
with `--after` its last emitted sequence, not the snapshot end. No matching events is exit 0. Export contains the original bodies;
it is an explicit analysis operation, separate from metadata telemetry below.

Both commands accept exact conjunctive `--project`, `--bead`, `--host` filters and
inclusive `--since <Unix-milliseconds>` on envelope timestamps. `--after <seq>`
skips arrival sequences through a supplied cursor. Each invocation scans the
stream through its initial INFO last sequence; concurrent appends wait for a later
scan. It scans that whole snapshot to compute the matching total, even with a
small output limit. Large histories can take time; memory holds only the listed
metadata or one exported event. A failed fetch, missing sequence, invalid JSON or
unsupported schema exits nonzero, never an empty success. A failed export may
already have written JSONL and then prints the usual structured error on stdout;
consumers must check exit status and discard partial output before analysis.
Snapshot sequence is a cursor, not a count of timestamp-ordered events.

## Optional metadata-only OTLP logs: orrery collector contract

The history exporter and capacity gauges use the shared `$XDG_CONFIG_HOME/vega/otlp.toml`
(`~/.config/vega/otlp.toml`); capacity gauges are described above.
A missing file or `enabled = false` disables export.
To opt in, configure an OTLP/HTTP **base** endpoint and restart vegad:

```toml
enabled = true
endpoint = "http://collector.example:4318"
timeout_ms = 5000
```

The history sink posts JSON protobuf `ExportLogsServiceRequest` to `/v1/logs`.
It starts only with explicit enabled configuration; an endpoint alone does not
enable it. Invalid configuration disables the sink with a diagnostic. No viewer
or admin key is involved. It observes only this host's newly acknowledged history
publications, including inbox-only hosts. It does not read/backfill central
history. A dedicated thread sends from a bounded 64-record channel; full queues,
HTTP errors and process crashes may lose telemetry. HTTP calls use the configured
timeout, disable redirects and never block history publication or live mail.
Central VEGA_HISTORY remains the record. OTLP has no durable retry archive and
cannot provide delivery completeness; an ack/unlink crash window may duplicate
or omit metadata, so collectors may deduplicate with `vega.event.id`.

The orrery collector should route logs by resource `service.name = vega.history`
(and recording `host.name`) to the history metadata route. This is the complete
allowlist; no arbitrary event data, header or attribute copying is permitted:

| Location | Field | Type/value |
| --- | --- | --- |
| Resource | `service.name` | string, constant `vega.history` |
| Resource | `host.name` | string, recording host |
| Scope | `name` | string, constant `vega.history` |
| LogRecord | `eventName`, `body.stringValue` | constant known kind string |
| LogRecord | `severityNumber`, `severityText` | INFO (9), `INFO` |
| LogRecord | `timeUnixNano` | envelope `ts` milliseconds × 1,000,000; decimal string |
| Attributes | `vega.schema` | int64 1 (JSON `intValue` decimal string) |
| Attributes | `vega.event.id` | string, stable event ID |
| Attributes | `vega.project`, `vega.bead`, `vega.pane`, `vega.session` | strings, omitted when unknown |

Known kinds are `spawn`, `mux.send`, `mail.sent`, `mail.local`,
`lineage.start`, `lineage.change`, `lineage.end`, `land.phase`, `land` and `deploy`. Unknown kinds
or schemas are skipped instead of copying arbitrary text. The OTLP body is only
the constant kind, never an event body. Mail bodies/headers, briefs, mux text,
lineage values, cwd paths, transcripts, raw JSON and arbitrary `data` fields are
excluded. Diagnostics do not include collector responses or event content.
