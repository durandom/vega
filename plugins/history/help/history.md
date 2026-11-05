vega history [status|read|export]

status: local host's last real PubAck time (Unix ms), sequence, pending receipts,
and last publication error. Null means unknown; no broker credentials needed.
read: compact event metadata, first 100 matches by arrival sequence.
export: full envelopes as JSON lines on stdout; count on stderr (zero is success).

read/export flags:
  --viewer-key <path>  Separate viewer seed (else VEGA_HISTORY_VIEWER_KEY or
                       first line of $XDG_CONFIG_HOME/vega/history.viewer-key).
  --project <id> --bead <id> --host <host>  Exact, conjunctive filters.
  --since <unix-ms>    Inclusive envelope timestamp, independent of arrival order.
  --after <seq>        Skip sequences <= cursor (default 0).
  --limit <n>          Positive match limit (read 100; export unlimited).
  --help              Show this reference. status accepts no flags.

Broker: VEGA_NATS_URL or $XDG_CONFIG_HOME/vega/nats.url. Uses _INBOX_viewer;
never falls back to host/admin keys. Scans a fixed stream snapshot without a
consumer; concurrent appends wait for the next scan. Any read error fails;
a failed export can leave partial JSONL followed by a structured error.

  vega history status
  vega history read --viewer-key /secure/viewer.nk --bead vega-lhc.252
  vega history export --viewer-key /secure/viewer.nk --since 1793894400000 > history.jsonl
