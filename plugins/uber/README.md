# vega uber

The Uber-UAC: one session over all projects and hosts that relays the Slayer's answers to every UAC.

Commands: `vega uber claim`, `vega uber release`, `vega uber status`; for a UAC, `vega uber ask`; on the
Uber host, `vega uber answer`, `vega uber audit`. The Claude Code capture hook is the separate binary
`vega_uber_capture`, not a `vega uber` command.

The Uber takes asks only while it is engaged: for 2 h after the Slayer's last input to it, ending early
when the Slayer focuses a herdr pane on any host. While it is idle, `vega uber ask` returns `uber_absent`
and the UAC decides through its own pane. `vega uber status` prints `engaged: yes|no (reason)`.

Design: [uber-uac.md](../../docs/design/uber-uac.md).

Agent guide: `vega skills uber`
