# vega uber

The Uber-UAC: one session over all projects and hosts that relays the Slayer's answers to every UAC.

Commands: `vega uber claim`, `vega uber release`, `vega uber status`; for a UAC, `vega uber ask`; on the
Uber host, `vega uber answer`, `vega uber audit`. The Claude Code capture hook is the separate binary
`vega_uber_capture`, not a `vega uber` command.

Design: [uber-uac.md](../../docs/design/uber-uac.md).

Agent guide: `vega skills uber`
