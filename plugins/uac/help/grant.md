Usage: vega uac grant <inbox-only-host>:<pane> <host>:<pane> --expires <duration>
       vega uac revoke <inbox-only-host>:<pane> <host>:<pane>
       vega uac grants

Mail from an inbox-only host (deimos, puck) lands in quarantine, a notice for the
Slayer; a grant lets one pair through. Agents create one only after the Slayer
said so. Run on the receiving vegad host, with its HOME/XDG_STATE_HOME. One literal pair
per grant, no wildcards. --expires is required: positive integer + s/m/h/d.
Pane IDs are alphanumeric, e.g. w3V:p1 or w1C:pB; an hc terminal is <host>:t<N>,
e.g. puck:t1 (the sender claims a bare tN).
Grant replaces that pair's expiry (rerun to renew); revoke is idempotent. Listing includes expired
and revoked records with Unix timestamps, also shown in the uac home view.
Stored atomically in $XDG_STATE_HOME/vega/mail-grants (default ~/.local/state).
The sender host is broker-authenticated; its sender pane is that host's own
claim. Granted replies are peer mail from an inbox-only host, never Slayer
approval. No broker permission changes. Unmatched requests remain human notes.

Examples:
  vega uac grant deimos:w15W:p2 proteus:w3V:p1 --expires 2h
  vega uac grant puck:t1 proteus:t6p --expires 7d
  vega uac revoke deimos:w15W:p2 proteus:w3V:p1
  vega uac grants
