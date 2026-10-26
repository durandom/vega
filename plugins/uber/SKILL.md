---
name: vega-uber
description: Claims, releases and shows the Uber-UAC, the one estate-wide session that relays the Slayer's answers to every project's UAC, and sends a UAC's decision to it. Use when starting or handing over the Uber-UAC session, checking which session holds it, or, as a UAC, routing a slayer-decision bead to the Slayer's phone.
---

# vega uber: the Uber-UAC

```
vega uber claim     # this session becomes the Uber-UAC (one per estate, needs the broker)
vega uber release   # the Uber hands back its claim
vega uber status    # the live claim, superseded and gone claims (also: vega uber)
```

- A live claim elsewhere refuses with `uber_exists`; a claim whose pane or host is gone is taken over
  (`took_over:`).
- Two claims racing: the newest live claim wins. The loser gets `uber_conflict` and releases.

UAC side: after recording a question as a `slayer-decision` bead, route it to the Uber-UAC:

```
vega uber ask --bead <id> --level L3|L4 --deadline <rfc3339|no-rush> [--urgent "<why>"] \
  [--question "<≤120, default bead title>"] [--context "<≤280>"] \
  --option "<label>: <detail>" --option ... --recommend <0-based> [--why "<≤120>"]
```

- L3: the Slayer picks an option. L4 (deploy, GitHub settings, broker rights, deletions,
  irreversible): only the Slayer may approve. L0-L2: decide yourself.
- `uber_absent` (no live claim, no broker): nothing is stored or sent; decide through the bead as before.
- Then park the waiting sentinel (`vega uac park`); do not wait in your pane. The answer arrives as a
  Slayer comment `via uber` on the bead and a mail. Closing the bead withdraws the ask (vegad).
- Design: docs/design/uber-uac.md.
