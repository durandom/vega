# vega post

vegad also runs the blocking-ask delivery worker: seat election, durable cross-host receipts,
focus/draft suppression and toast/macOS adapters shared with `vega notify test`.
Local mode delivers origin-local asks; inbox-only hosts use toast only. See
[notify](../../docs/design/notify.md) for the staged channel and routing build.

Agent mail through a NATS broker, also across machines. A message is stored in the broker, not typed into the
target pane; vegad, the relay on each host, types a one-line pointer into the pane and the target pulls its mail.
This keeps long messages out of busy panes and lets agents on different machines talk.

`vega post relay install` pins `vegad.env` and links the optional herdr startup hook; it writes
no login job and removes one from an earlier version (vega-0h4c.42). The herdr-app vega mod
starts vegad on its host and, through `hc exec`, on connected hc machines. `relay status`
shows a leftover login job; `vega config infra` flags it. hc machine/SSH retains core ownership.
See [Lifecycle](../../docs/design/vegad.md#lifecycle-vega-lhc54).

hc machine names must equal vega registry hosts. `vega post send ganymede/t7 "ping"`
(also `hc:ganymede/t7`) routes to host `ganymede` and terminal `t7` without `--host`.
The registry must contain that host; unknown hosts or unavailable checks fail before
enqueueing. A stale host remains known and follows the existing reachability warning
policy. If a fresh registry has not listed the terminal yet, a live hc result for
`machine/t7` allows the send immediately. A conflicting `--host` cannot remap a qualified address. Unqualified `t7
--host <host>` keeps its existing explicit-host behavior. Inbox-only hosts cannot
verify remote names and must use that explicit form from a reply command.
`vega config infra --fleet` checks names and reports `hc machine rename <old> <host>`
for mismatches. No SSH alias or remote identity query participates in routing.

Daemon herdr subprocesses and socket requests have deadlines; timeouts are logged
and pointer mail remains pending for retry. Event-stream silence triggers a fresh
snapshot after 30 seconds. See [herdr integration](../../docs/design/vegad.md#herdr-integration).

Registry removals use one-hour TTL purge markers when `KV_VEGA_REGISTRY` has
`allow_msg_ttl` enabled, falling back to plain delete with one diagnostic otherwise.
Each vegad checks a startup snapshot of its own subjects in batches of 32 per
15 seconds and converts old tombstones; live values and other hosts are preserved.
Existing host ACLs suffice. See [registry](../../docs/design/vegad.md#registry-m4).

A pointer never joins text the user is typing (the draft guard): vegad reads the input box of Claude Code,
Codex and pi and holds the pointer while it has a draft; a draft left 5 minutes is stashed and typed back
after the pointer, or, when it cannot be typed back, keeps holding with one notification. A screen with no
known input box holds the pointer until it has been unchanged for 20 s (one notification after 5 busy
minutes). hc terminals differ: a draft holds only while it changes and goes 60 s after the last
input change, with no stash or restore; transcript and footer changes do not extend the hold.
Known Codex placeholder hints count as empty, so parent and peer mail can reach working Codex agents immediately.
Both parent and peer pointers queue while an agent works; blocked or unknown targets wait.
A screen with no known input box does not hold. `vega mux send --now` pastes without the guard. See
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

`log` scans the filtered archive without acknowledging mail. It allows 10 seconds
for the first message and 5 seconds between messages; a timeout or premature end
returns `broker_error`. An empty result comes from the broker's filtered pending
count, so a stalled first read cannot be reported as zero messages.

Commands: `vega post`, `send`, `inbox`, `done`, `ask`, `log`, `notes`, `relay`.

Inbox-only same-host `send`/`done` deliver through herdr; `--uac` resolves local claims only.
Cross-host replies remain human quarantine notes unless the receiving host has a live
operator pair grant (`vega uac grant <inbox-only-host>:<pane> <host>:<pane> --expires 2h`).
Granted replies enter that peer's inbox with an inbox-only marker, never Slayer approval;
the sender pane is its host's own claim. Expiry, revocation and nonmatching pairs retain
the quarantine note. See [quarantine](../../docs/design/vegad.md#quarantine-inbox-only-replies).

Uber-UAC mail ([uber-uac.md](../../docs/design/uber-uac.md) §3): `send --uber` mails the live Uber-UAC
claim (`vega_registry::View::live_uber`) and fails with `uber_absent` when none is live. `send --kind
uber.ask|answer|applied|withdraw|brief '<json>'` sets the `Vega-Kind` header and checks the body against its
schema and byte cap (`crates/vega-uber-msg`) first: `uber_too_large`, `uber_malformed` or `uber_kind`, and
nothing is sent. vegad verifies every `uber.answer` before the addressed UAC sees it (uber-uac.md §4); `inbox` and
`log` print it withheld. Other kinds are not checked on receipt yet.
`ask --kind uber.brief` sends the Uber's brief the same way (body as is); `inbox` tells the UAC how to reply,
and vegad on the Uber host stores each reply on a recorded brief thread (`vega_uber_msg::brief`).

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
`vegad.env`; a non-empty process `VEGA_POST_HOST` wins over the file and selects
that host's relay lock and identity state. Otherwise the host matches the pinned
vegad host. See [host precedence](../../docs/concepts.md).

Central history capture and the explicit archive migration are documented in
[history](../../docs/design/history.md). All hosts, including inbox-only hosts,
publish their own history through vegad; mail restrictions remain unchanged. When OTLP is enabled,
vegad also reports its inbox stream capacity every five minutes. VEGA_HISTORY capacity requires a
separate stream-info grant, which generated host permissions do not include.

`vega history status` inspects local PubAck evidence without privileged keys.
The separate `vega history read/export` plugin uses a viewer seed for analysis.
Optional metadata-only OTLP logs are off by default; configure `vega/otlp.toml`
and restart vegad to opt in (the history design specifies the collector allowlist).
