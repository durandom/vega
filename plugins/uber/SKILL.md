---
name: vega-uber
description: Claims, releases and shows the Uber-UAC, the one estate-wide session that relays the Slayer's answers to every project's UAC. Use when starting or handing over the Uber-UAC session, or to check which session holds it.
---

# vega uber: the Uber-UAC claim

```
vega uber claim     # this session becomes the Uber-UAC (one per estate, needs the broker)
vega uber release   # the Uber hands back its claim
vega uber status    # the live claim, superseded and gone claims (also: vega uber)
```

- A live claim elsewhere refuses with `uber_exists`; a claim whose pane or host is gone is taken over
  (`took_over:`).
- Two claims racing: the newest live claim wins. The loser gets `uber_conflict` and releases.
- Design: docs/design/uber-uac.md.
