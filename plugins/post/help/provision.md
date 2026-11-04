Connects with the admin seed: VEGA_NATS_NKEY, else
$XDG_CONFIG_HOME/vega/nats-admin.nk (`vega post keygen --admin`).
Existing streams are checked; lists push consumers (vega never makes any).
`registry` creates vegad's KV bucket; `lanista` creates the catalog object store.
Creation needs `auth-config --create <host|registry|lanista>` temporarily.

`provision registry --ttl` is the sole update: reads the existing registry config,
preserves every other field verbatim, enables allow_msg_ttl and checks the result.
Already enabled: no-op, no update request. A missing bucket is an error; this does
not create it. --ttl accepts only the single target registry.

Operator procedure (enabling TTL is irreversible on NATS 2.14.7):
1. Render the usual auth-config with --admin and --create registry-ttl; reload.
2. Run `vega post provision registry --ttl`.
3. Render without --create registry-ttl and reload, including after an error.
The temporary grant allows ANY registry stream update, including republishing;
the broker cannot restrict it to TTL. Never leave it enabled. No host gets it.
Default admin rights cannot update, read mail, delete or purge streams.

Examples:
  vega post provision proteus ganymede
  vega post provision lanista
  vega post provision registry --ttl
