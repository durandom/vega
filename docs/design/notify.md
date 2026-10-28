# vega notify: a louder signal when a UAC waits on the Slayer

Status: design, 2026-10-28; vega-lhc.226. Supersedes vega-lhc.71 (operator signal) and vega-lhc.110 (moshi
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
vega notify                      # open asks of every host: id, level, age, project, pane, line, next step
vega notify resolve [<id> | --key <k> | --pane <pane>]
vega notify ack <id>             # the Slayer saw it: stop escalation and reminder, keep it open
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

## Who delivers

| Class | Channels | Host |
| --- | --- | --- |
| seat | herdr toast, macOS Notification Centre | the **seat**: the host with the newest `human_at` (uber-uac.md engaged window) |
| remote | ntfy, Pushover, Moshi | the **notifier**: `notifier` in notify.toml, default `proteus`; same value everywhere |

Each vegad reads open asks from the registry; only the seat runs seat channels and only the notifier runs
remote ones. No failover, as for the PagerDuty poller: a notifier that is down sends nothing, and `vega
notify` prints `notifier proteus offline` when its beat is stale. In local mode (no broker) the origin host is
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
notifier = "proteus"
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
| action | toast + macos | after 15 m: phone if the seat is idle, else toast + macos again | 1 h after raise, phone |
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

## Build beads (proposed, not filed)

1. **notify CLI + ask state.** `plugins/notify` (`vega notify` raise, list, resolve, ack, test), ask as a
   note (`kind: ask`), key dedupe, resolve by command, pane close and bead close, guide via `--skill`. Done
   when unit tests cover raise/re-raise/level-up/resolve/ack and list across two fake hosts, and `mise run
   check` exits 0.
2. **Delivery loop + toast/macos.** vegad thread: seat and notifier roles, `sent.json` / `<host>.notify.sent`,
   suppression (focus, `seat_idle`, draft), toast and macos channels. Done when fake-clock fixtures cover seat
   change, restart without resend, two hosts without duplicates, suppression, local mode, and `mise run check`
   exits 0.
3. **Routing, escalation, quiet hours.** notify.toml parsing, levels, escalate/remind once, per-project
   remind, quiet hours with the PagerDuty-high exception, Uber-engaged reroute. Done when fake-clock tests
   cover every row of Timing, quiet-hour deferral and the exception, ack, and `mise run check` exits 0.
4. **ntfy channel.** Client with token from `vega secrets`, priorities, `vega notify test ntfy`. Done when a
   fake ntfy server test checks request shape and that no token appears in argv/logs, a live test on titan
   reaches the Slayer's iPhone, and `mise run check` exits 0. Prereq (Slayer/titan): ntfy server with
   `upstream-base-url`, auth, the `ntfy` secret.
5. **Resolve on typed prompt.** The capture hook resolves the open ask of its pane. Done when hook tests
   show a typed prompt resolves and a vegad pointer or task notice does not, and `mise run check` exits 0.
6. **UAC rule.** uac-rules.md and the uac guide carry the rule above; vegad raises PagerDuty-high asks.
   Done when the guide test checks the rule text, pagerduty tests cover the raise, and `mise run check`
   exits 0. Then close vega-lhc.71 and vega-lhc.110.
7. Later: **pushover** and **moshi** channels, each with a fake-server test and `mise run check`.

## Open questions

1. Defaults: escalate after 15 m, remind 1 h (orrery 2 h), urgent remind 15 m, quiet 22:30-07:30. OK?
2. Does a UAC-raised `urgent` also break quiet hours, or only PagerDuty high (as written)?
3. At the desk but not reacting: escalate to the phone anyway, or re-toast only (as written)?
4. Phone ack for ntfy: none in v1 (reading the push is not an ack). Add an ntfy action button later? It
   would need an HTTP endpoint on vegad, which does not exist.
5. Notifier failover: none (as PagerDuty). Acceptable when proteus is down?
