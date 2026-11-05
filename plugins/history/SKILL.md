---
name: vega-history
description: Reads and exports central vega event history with a viewer key and shows local publication evidence. Use when analyzing events by project, bead, host or timestamp or checking a host's history outbox.
---

# vega history

- `vega history`: local publication evidence and discovery.
- `vega history status`: local host's last real PubAck time/sequence, pending count,
  and last error. No history read privileges or admin seed needed.
- `vega history read --viewer-key <path>`: metadata list; filters `--project`,
  `--bead`, `--host`, `--since <unix-ms>`; `--limit` defaults to 100.
- `vega history export --viewer-key <path>`: full JSONL envelopes for analysis,
  unlimited by default, same filters; `--after <seq>` skips arrival sequences.
- Configure the separate viewer seed path with VEGA_HISTORY_VIEWER_KEY or
  `$XDG_CONFIG_HOME/vega/history.viewer-key`; broker uses normal nats.url config.
  No host/admin key fallback and no consumers. `--help` gives the contract.

History can contain briefs, mail bodies and mux text. OTLP is a separate,
explicitly enabled metadata sink; see docs/design/history.md for its allowlist.
