The relay types one pointer for all pending mail while the target is idle, done or working (queued input), for peer and parent mail alike. A non-empty input box holds it behind the draft guard; blocked or unknown targets wait, and a pointer never enters a dialog.
send checks the target in the registry first: a pane its host no longer lists fails with pane_gone and a hint naming the project's UAC, unless hc reports the verified machine/t7 terminal live before its registry reconciliation; a host without a fresh registry entry gets the mail with a warning. A pane that closes with mail unread: its vegad bounces it once to each sender.
hc machine names must equal registry hosts. machine/t7 (also hc:machine/t7) needs no --host; the host must exist in the registry. Unknown hosts or unavailable checks fail without sending, and --host cannot remap a qualified address. Check names with vega config infra --fleet; fix with hc machine rename <old> <host>.
Who is reachable: vega post and the registry (`vega uac`), not `herdr machine list` (only the machines this one saved).

Examples:
  vega post send w3V:p2 "ping"
  vega post send ganymede/t7 "ping"
  vega post send --uac vega "<text>"                # the project's UAC, on whichever host
  vega post send --uac . "<text>"                   # the UAC of this checkout's project
  vega post send --uber --kind uber.withdraw '{"id":"<id>","reason":"obsolete"}'   # the live Uber-UAC
  vega post send --now w3V:p2 "stop: wrong branch"   # urgent: pointer at once
  vega post send --host ganymede w3V:p2 "ping"
  vega post send w3V:p1 --re <thread>/<hop>@<published-ms> "pong"   # reply command from `vega post inbox`
