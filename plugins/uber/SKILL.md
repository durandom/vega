---
name: vega-uber
description: Claims, releases and shows the Uber-UAC, the one estate-wide session that relays the Slayer's answers to every project's UAC, sends a UAC's decision to it, and sends the Slayer's answers from their captured input. Use when starting or handing over the Uber-UAC session, checking which session holds it, as a UAC routing a slayer-decision bead to the Slayer's phone, or as the Uber-UAC answering an ask the Slayer decided or reading the Uber audit.
---

# vega uber: the Uber-UAC

```
vega uber claim     # this session becomes the Uber-UAC (one per estate, needs the broker)
vega uber release   # the Uber hands back its claim
vega uber status    # live claim (engaged or idle), superseded and gone ones (also: vega uber)
vega uber answer a7 --capture 12   # send the Slayer's answer to #a7, built from capture 12
vega uber audit [--last 10]        # one line per event: received, answered, sent, applied, refused
vega uber brief [--show]           # ask the UACs; --show renders
```

- A live claim elsewhere refuses with `uber_exists`; a gone one is taken over (`took_over:`). Racing claims:
  newest wins; the loser releases (`uber_conflict`).
- Engaged for 2 h after the Slayer's last input, until their herdr input elsewhere; idle, asks stay
  away. On `vega uber: you were idle (...)`: tell the Slayer once, then pull the brief.
- Never write the Slayer's answer yourself. `vega uber answer` builds it only from a capture of their
  input (written by a hook you cannot run): their pick in the AskUserQuestion dialog that showed `#a7` as
  `vega uber show` rendered it, or a typed prompt naming `#a7`. Without `--capture`: `no_capture`, listing
  the bound captures.
- L4 approval only from a picked option label; typed text or "Other" reaches the UAC as a comment.
- Refusals: `capture_unbound` (no tag, or older than the ask), `capture_mismatch` (options reworded),
  `capture_unverified` (not in this session's log),
  `capture_used`, `not_open` (already sent, applied or withdrawn), `not_uber`, `no_session_log`.
- A UAC refusal shows in `vega uber audit`; unless final (`answered`, `withdrawn`, `bead_closed`, `no_ask`)
  the ask reopens; the same capture may be resent.

UAC side: after recording a question as a `slayer-decision` bead, route it to the Uber-UAC:

```
vega uber ask --bead <id> --level L3|L4 --deadline <rfc3339|no-rush> [--urgent "<why>"] \
  [--question "<≤120, default bead title>"] [--context "<≤280>"] \
  --option "<label>: <detail>" --option ... --recommend <0-based> [--why "<≤120>"]
```

- L3: the Slayer picks an option. L4 (deploy, GitHub settings, broker rights, deletions,
  irreversible): only the Slayer may approve. L0-L2: decide yourself.
- `uber_absent` (no live claim, no broker, idle Uber): nothing is stored or sent; decide through the bead
  as before.
- Then park the sentinel (`vega uac park`); do not wait in your pane. The answer arrives as a
  Slayer comment `via uber` on the bead and a mail. Closing the bead withdraws the ask (vegad).
- Design: docs/design/uber-uac.md.
