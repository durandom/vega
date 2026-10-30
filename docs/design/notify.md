# vega notify: a louder signal when a UAC waits on the Slayer

Status: agreed, 2026-10-28 (Slayer answers below); vega-lhc.226. Supersedes vega-lhc.71 (operator signal) and vega-lhc.110 (moshi
phone path); relates to vega-lhc.170.6 (attention reminders) and vega-lhc.65 (approval view). Builds on
[vegad](vegad.md) (Notifications, registry), [Uber-UAC](uber-uac.md) (engaged window, `human_at`),
[secrets](secrets.md) and [upgrade](upgrade.md) (moshi-hook).

## Problem

Work stalls because a UAC waits on the Slayer (a go, a test on a device, a login, `! <cmd>`) and the ask is
only text at the end of a pane reply. 2026-10-28: the spellkave UAC (ganymede w5S:p1) waited ~7.5 h for a go
on orrery step 4.1, and egress 9443 has blocked PR #4573 since 12:03; the orrery UAC (w43:p1) waited 1-2 h on
a multinode apply and again on a classifier block that needed `! <cmd>`. The Slayer learns by looking at the
pane. Decision beads have a board; inline asks have nothing.

## Rules

- One open **ask** per blocking question; it fires on raise, escalates once, reminds once, then stays quiet
  on the board until resolved.
- `vega notify` only tells. It answers nothing, has no reply path into a pane and no approve action. Agent
  permission gates stay in the pane (uac-role.md hard limits); a phone tap never types.
- No ask for FYI, progress, peer mail or acks. Those stay in the pane or mail.
- One host delivers each channel class (below): no duplicate pushes from several vegads.

## CLI

```
vega notify <info|action|urgent> "<line>" [--bead <id>] [--cmd "<cmd>"] [--key <k>]
            [--pane <pane>] [--project <key>]
vega notify [list]               # open asks of every host: id, level, age, project, pane, line, next step
vega notify resolve [<host>=<id> | --key <k> | --pane <pane>]
vega notify ack <host>=<id>             # the Slayer saw it: stop escalation and reminder, keep it open
vega notify test <channel>       # one test message through a channel
```

- `--pane` defaults to the caller's pane, `--project` to the checkout's project key, `--key` to
  `<project>/<pane>` (one ask per pane). Raising an open key again replaces the line and keeps the clock;
  a higher level escalates at once, a lower one changes nothing.
- The line is one sentence, at most 120 chars, in the shape `<project>: <what> -> <pane>`:
  `orrery: go for apply io (preview 7f2) -> w43:p1`, `orrery: please run ! vega bd init -> w43:p1`.
  `--cmd` is shown verbatim (terminal controls stripped). Never a secret: the UAC rule says so.
- Output (AXI): `ask a7 (action) open; toast now, ntfy at 13:42 unless resolved`. Re-raise:
  `ask a7 updated`. Resolve with nothing open: `no open ask for <key>` and exit 0.
- vegad raises asks too: PagerDuty high-urgency incidents ([pagerduty.md](pagerduty.md), `--source
  pagerduty`); later perhaps headed panes in `asking` past N minutes (vega-lhc.170.6 may reuse the loop).

## State, dedupe and resolve

An ask is a note ([vegad.md](vegad.md), Notifications) with `kind: ask`, `level`, `key`, `cmd`, `bead` and
`cause: ask:<key>`. That reuses the store, the registry reconcile (`<host>.note.<id>`), the board Inbox and
the UAC badge. The origin host owns the ask; its vegad sets `resolved`.

Resolve, whichever comes first:

- `vega notify resolve` (the UAC, once it got its answer and continues);
- a Slayer prompt typed into the ask's pane: the Claude `UserPromptSubmit` capture hook ([uber-uac.md](uber-uac.md)
  §4) already tells typed prompts from vegad pointers and task notices; panes of other harnesses resolve
  explicitly;
- the pane closes; the bead named by `--bead` closes.

Delivery state (which channel sent what, ack, reminder done) lives with the deliverer, in
`$XDG_STATE_HOME/vega/notify/sent.json`, published as `<host>.notify.sent`, so a restart or a new deliverer
does not resend. Inbox-only hosts (deimos) have no registry: their asks reach only the local toast.

## CLI/state build (.226.1)

The CLI persists asks without sending. Delivery, channel probes, notifier health and timing output
land in .226.2-.4; until then raise prints `delivery pending` and `test <channel>` reports
`channel_unavailable` (exit 1). No channel is silently treated as delivered.

- IDs are `<host>=<millisecond-id>` to disambiguate hosts. Explicit ID resolve/ack uses the existing
  `vega post cmd` transport (`notify` handler) so only the origin writes its note. `--key` and `--pane`
  select asks on the local host; bare resolve selects the caller's default project/pane key.
- `acked` (Unix seconds) is stored on the origin note, independent of read/clear watermarks. The
  delivery loop must honor it when maintaining its per-deliverer receipts in `sent.json`.
- A same-level re-raise replaces line, command, bead and target metadata, retaining ID/clock and ack;
  a higher level does the same and records the new level for delivery; a lower level changes nothing.
  Dedupe is origin-local: two hosts can have the same key, with independent qualified IDs.
- Open asks are exempt from note retention limits: they stay on the board until resolved. Resolved
  asks follow ordinary note pruning. Read/clear watermarks do not hide open asks from the board
  or `vega notify list` (reading can still clear the unread badge).
- Origin vegad resolves pane/bead closure on the metadata clock, including local-only mode. Unknown
  pane status or a failed bead query keeps the ask open. Bead queries use a verified owner-local
  project checkout and configured bd binary; without one, explicit resolve still works.
- The typed-prompt hook is .226.5; UAC rule installation and PagerDuty asks are .226.6.

## Who delivers

| Class | Channels | Host |
| --- | --- | --- |
| seat | herdr toast, macOS Notification Centre | the **seat**: the host with the newest `human_at` (uber-uac.md engaged window) |
| remote | ntfy, Pushover, Moshi | the **notifier**: `notifier` in notify.toml, default `ganymede` (runs 24x7); same value everywhere |

Each vegad reads open asks from the registry; only the seat runs seat channels and only the notifier runs
remote ones. No failover, as for the PagerDuty poller: a notifier that is down sends nothing, and `vega
notify` prints `notifier ganymede offline` when its beat is stale. In local mode (no broker) the origin host is
both. This also lifts vegad.md's "not macOS Notification Center" limit: one host shows each ask.

## Channels

```rust
trait Channel {
    fn class(&self) -> Class;                          // Seat | Remote
    fn send(&self, ask: &Ask, why: Why) -> Result<Receipt>; // Why: Raise | Escalate | Remind
    fn acked(&self, r: &Receipt) -> Option<bool> { None }   // polled; Pushover only
    fn resolve(&self, r: &Receipt) -> Result<()> { Ok(()) } // withdraw where supported
}
```

- **toast** (exists): `vega_herdr::notes` already shows it (`--sound request`). Results `busy`,
  `no_foreground_client` count as not delivered and fall through to escalation.
- **macos**: `osascript -e 'display notification …'` on the seat when it is macOS; no click action. Linux
  seat: `notify-send` if present, else skipped.
- **ntfy** (first phone channel, Slayer 2026-10-28): self-hosted on titan, reached over the tailnet. POST
  to `<url>/<topic>` with `Authorization: Bearer <token>`, title = the line, body = cmd/bead, priority 3
  (action) or 5 (urgent), tag per project. iOS: Apple allows push only through APNs, so the titan server
  sets `upstream-base-url: https://ntfy.sh`. For each message it sends ntfy.sh a `poll_request` carrying the
  message ID and a SHA-256 hash of the topic URL. ntfy.sh wakes the iOS app via APNs, and the app fetches
  the message from titan. **Leaves the tailnet:** the topic hash, message ID and timing (to ntfy.sh and
  Apple), not the title, body or command. The phone needs Tailscale up to fetch; without it the push shows
  no content. To verify in the build: what the iOS app shows when the fetch fails, and how priority 5 shows on
  iOS (Focus / time-sensitive).
- **pushover** (later): `priority=2` (emergency) for urgent repeats until acked (`retry`, `expire`); the
  receipt API gives `acked`, which becomes `vega notify ack`. Title and body go to Pushover and Apple: use only
  for projects whose text may leave the tailnet (`[channels.pushover] projects = [...]`).
- **moshi** (later): through moshi-hook, already optional infra ([upgrade.md](upgrade.md)). Its API and what
  it sends off-host: I don't know; read its docs before building.

Credentials come from `vega secrets` (`Store::get("ntfy", "vega-notify")`), passed in a header, never argv or
logs. The secrets `ntfy` and `pushover` are the Slayer's to create (`for = ["vega-notify"]`, hosts = the
notifier).

## Config: `~/.config/vega/notify.toml`

```toml
notifier = "ganymede"
quiet = "22:30-07:30"              # the notifier's local time
seat_idle = "5m"                   # seat input newer than this: the Slayer is at the desk

[levels.info]
channels = ["toast"]
[levels.action]
channels = ["toast", "macos"]
escalate = { after = "15m", channels = ["ntfy"] }
remind = "1h"
[levels.urgent]
channels = ["toast", "macos", "ntfy"]
remind = "15m"

[channels.ntfy]
url = "https://ntfy.<tailnet>"
topic = "vega"
secret = "ntfy"

[projects."github.com/durandom/orrery"]
remind = "2h"                      # spellkave keeps 1h
```

No file: toast only, as today. A channel named in a level but not configured is a `vega notify` warning
and an `vega onboard` config finding, not an error. The file is shared via the vega config (vega-lhc.107);
every host reads the same values.

## Timing

| Level | Raise | Escalate | One reminder |
| --- | --- | --- | --- |
| info | toast | never | never |
| action | toast + macos | after 15 m: phone, also when the Slayer sits at the seat | 1 h after raise, phone |
| urgent | toast + macos + phone | none | 15 m after raise, phone |

- After the reminder: nothing more; the ask stays on the board and in `vega notify`. Ack stops both steps.
- **Quiet hours** hold every phone send until they end, then send the escalation once (not the backlog of
  reminders). Exception: a PagerDuty high-urgency incident (`--source pagerduty`, `urgency: high`) pages
  during quiet hours.
- **Suppression:** no seat send while the ask's pane is focused on the seat and seat input is younger than
  `seat_idle`, or `delivery::draft` sees a draft in that pane: the Slayer is reading or typing there. The
  clock keeps running; the next tick re-checks.
- **Uber engaged:** while the Uber claim is engaged ([uber-uac.md](uber-uac.md) §2), the phone step goes to
  the Uber as an ask (`vega uber ask`) instead of ntfy: the Slayer is already in Claude on the phone. Not
  engaged: ntfy.
- vegad ticks every 30 s; at-least-once like PagerDuty: a crash between send and checkpoint may send twice.

## UAC rule (lands with the CLI)

Added to [uac-rules.md](uac-rules.md) and the uac guide when `vega notify` ships, not before:

> When you block on the Slayer (a go, an approval, a device or host test, a login, `! <cmd>`), run
> `vega notify action "<project>: <what> -> <pane>" [--bead <id>] [--cmd "<cmd>"]` and end your turn.
> `urgent` only for production or data at risk. `vega notify resolve` when you continue. Not for FYI,
> progress or mail. Never put secrets in the line or `--cmd`.

## Build beads

Filed 2026-10-28 under vega-lhc.226; each bead's acceptance includes `mise run check`.

| Bead | Scope | After |
| --- | --- | --- |
| vega-lhc.226.1 | CLI and ask state (`plugins/notify`, ask as note, dedupe, resolve, ack) | - |
| vega-lhc.226.2 | vegad delivery loop: seat/notifier, `sent.json`, suppression, toast, macOS | .1 |
| vega-lhc.226.3 | Routing, escalation, one reminder, quiet hours, Uber reroute | .2 |
| vega-lhc.226.4 | ntfy channel; prereq: ntfy on titan with `upstream-base-url`, `ntfy` secret | .2 |
| vega-lhc.226.5 | Resolve on a typed prompt (capture hook) | .1 |
| vega-lhc.226.6 | UAC rule in uac-rules.md and the uac guide, PagerDuty-high asks; closes .71, .110 | .3, .4 |
| vega-lhc.226.7 | Pushover channel (P3) | .3 |
| vega-lhc.226.8 | Moshi channel; first answers the off-host question (P3) | .3 |

## Decisions (Slayer, 2026-10-28)

1. Defaults as written: escalate after 15 m, remind 1 h (orrery 2 h), urgent remind 15 m, quiet 22:30-07:30.
2. Only a PagerDuty high-urgency incident breaks quiet hours; a UAC-raised `urgent` waits.
3. At the desk but not reacting: the phone escalation still fires after 15 m.
4. No ntfy ack in v1; reading the push is not an ack (an action button would need an HTTP endpoint on vegad).
5. Notifier default `ganymede` (24x7), no failover.

## Open questions

- Moshi: its API and what it sends off-host are unknown; answer in the moshi adapter bead before building.
