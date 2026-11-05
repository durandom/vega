# vega history

Viewer-authenticated read and JSONL export of central VEGA_HISTORY, with exact
project/bead/host and inclusive Unix-millisecond timestamp filters. Uses stream
INFO/MSG.GET with `_INBOX_viewer`, never consumers or host/admin seed fallback.

`vega history status` reads only the current host's local PubAck evidence and
pending outbox count. Unknown evidence remains null. It needs no broker access.

See [guide](SKILL.md), [help](help/history.md) and
[history design](../../docs/design/history.md) for privileges, export semantics,
limits and the optional metadata-only OTLP collector contract.
