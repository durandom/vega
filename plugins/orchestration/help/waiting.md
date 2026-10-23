# vega orchestration waiting

    vega orchestration waiting --on Slayer --for "decision pending" --bead <id>
    vega orchestration waiting --on <pane>@<host> --for "answer needed" --thread <id>
    vega orchestration waiting --coordinator <pane>@<host> --on Slayer --for "review"

Required: --on <owner>, --for <reason>.
Optional: --bead <id>, --thread <id>, --coordinator <pane@host>; --help/-h.
Default coordinator: the recorded local parent, otherwise the caller itself.
For a remote coordinator not recorded locally, specify --coordinator explicitly.

Owners: Slayer, pane@host, or ci:<existing-watch-target>@<head>. CI references
must match this caller's existing watch receipt; no watch is created.
The caller requires its current herdr pane and harness session identity.

Wait state belongs to the coordinator host, under local state. Remote updates
use the authenticated post command channel; the coordinator checks the sender's
host/current agent and its own waiter lineage. Hosts assert their own sessions;
this is coordination evidence, not a process capability or permission grant.
Repeating the same wait returns its ID. A different active wait for the same
waiter session/bead/thread must first be cleared with evidence. Inbox reads,
idle panes, expired asks and CI status never clear explicit waits.
