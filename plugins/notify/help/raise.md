vega notify info|action|urgent "<project>: <what> -> <pane>"
  --bead <id>       Resolve when this bead closes
  --cmd "<cmd>"    Next step, shown verbatim with terminal controls stripped
  --key <key>       Dedupe key (default <project>/<pane>)
  --pane <pane>     Target pane (default caller)
  --project <key>   Project identity (default checkout)
  --help            Show this help

One sentence, at most 120 characters. Never include secrets.
Re-raise replaces the line and retains the original clock/ack. Lower levels do nothing.
Urgent is for production or data at risk. notify.toml controls seat/phone routing and timers;
action escalates at 15m and reminds once at 1h, urgent reminds at 15m. Quiet: 22:30–07:30.
No config uses toast only. A stale notifier beat prints an offline notice.

Examples:
  vega notify action "orrery: go for apply -> w43:p1" --cmd "apply io"
  vega notify urgent "orrery: production down -> w43:p1" --bead orrery-1
