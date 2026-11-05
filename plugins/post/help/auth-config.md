--viewer U...: a read-only user for a UI such as NUI (seed generated where the UI runs; only the public key here).
It may list streams and read stream info, messages (STREAM.MSG.GET, DIRECT.GET) and consumer lists; it may not
publish, ack, fetch, create consumers, delete, purge, update or create. The UI connection must use inbox prefix
_INBOX_viewer and browse without consumers (NUI edge 46bef72 or later, not NUI v0.9.3).

The admin user may list consumers and read stream info of all hosts, and create streams only for --create hosts.
Keep --create empty in steady state: a create right on a missing stream lets the admin make it a mirror of
another host's stream that republishes all mail to the admin.

Adding a host: config with --create <host>, reload, `vega post provision <host>`, config without --create, reload.

--quarantine <host> (inbox-only hosts only): may publish vega.quarantine.*.*.<host>, request/reply, no stream; the
target host's vegad notes it for the human unless a live receiver-local operator pair grant delivers peer inbox mail. Every other host serves
vega.quarantine.<self>.> for that.

--registry-read <host>=<key> (inbox-only hosts only; repeat): one last-by-subject direct get of that registry key
($JS.API.DIRECT.GET.KV_VEGA_REGISTRY.$KV.VEGA_REGISTRY.<key>); no stream info (it would list the keys), put,
delete or watch. With <control>.theme there, the host's vegad follows its control host's dark/light mode.

The lanista catalog: every host (inbox-only ones too) may direct-get OBJ_VEGA_LANISTA, no consumer; --lanista-writer
<host> (the builder) may also put it. The bucket: --create lanista, then `vega post provision lanista`.

Examples:
  vega post auth-config proteus=UABC... ganymede=UDEF... --admin UXYZ...
  vega post auth-config --file hosts.txt --admin UXYZ... --create ganymede
  vega post auth-config --file hosts.txt --admin UXYZ... --inbox-only deimos --quarantine deimos \
      --registry-read deimos=proteus.theme
  (hosts.txt: host=U... lines, # comments)

One-time registry TTL migration: `--create registry-ttl` grants the admin only
`STREAM.UPDATE.KV_VEGA_REGISTRY`. Run `vega post provision registry --ttl`, then
regenerate without this grant and reload even on failure. UPDATE can enable
republishing; the broker cannot enforce TTL-only changes. Never leave it enabled.

History: full hosts may publish only vega.history.<self>.>; inbox-only hosts cannot.
The viewer can read VEGA_HISTORY via stream info/message get, never create consumers.
`--create history` temporarily permits creating VEGA_HISTORY (`provision history`).
`--create archive` temporarily permits updating listed inboxes (`provision <host> --archive`)
to no age expiry and 10 GiB. Remove both grants and reload immediately afterwards.
Reserve history's additional 10 GiB plus inboxes and other streams before provisioning.
