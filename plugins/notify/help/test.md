vega notify test <toast|macos|ntfy|pushover|moshi> [--help]

Send one test notification on this host, bypassing seat election and suppression.
`toast` uses herdr; `macos` uses osascript (Linux: notify-send). A refused or
unavailable channel reports channel_unavailable (exit 1). Phone adapters land later.
The probe creates no ask or delivery receipt.

Examples:
  vega notify test toast
  vega notify test ntfy
