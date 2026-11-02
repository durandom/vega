Usage: vega uac grant <inbox-only-host>:<pane> <host>:<pane> --expires <duration>
       vega uac revoke <inbox-only-host>:<pane> <host>:<pane>
       vega uac grants

Run on the receiving vegad host, with its HOME/XDG_STATE_HOME. One literal pair
per grant, no wildcards. --expires is required: positive integer + s/m/h/d.
Grant replaces that pair's expiry; revoke is idempotent. Listing includes expired
and revoked records with Unix timestamps, also shown in the uac home view.
Stored atomically in $XDG_STATE_HOME/vega/mail-grants (default ~/.local/state).
The sender host is broker-authenticated; its sender pane is that host's own
claim. Granted replies are peer mail from an inbox-only host, never Slayer
approval. No broker permission changes. Unmatched requests remain human notes.

Examples:
  vega uac grant deimos:w15W:p2 proteus:w3V:p1 --expires 2h
  vega uac revoke deimos:w15W:p2 proteus:w3V:p1
  vega uac grants
