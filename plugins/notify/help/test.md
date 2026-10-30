vega notify test <toast|macos|ntfy|pushover|moshi> [--help]

Probe a channel. Delivery adapters are not implemented in this CLI/state build:
this command reports channel_unavailable (exit 1) and sends nothing.

Examples:
  vega notify test toast
  vega notify test ntfy
