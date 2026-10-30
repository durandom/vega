vega notify — blocking asks for the operator

vega notify info|action|urgent "<project>: <what> -> <pane>"
            [--bead <id>] [--cmd "<cmd>"] [--key <key>] [--pane <pane>] [--project <key>]
vega notify [list]                   Open asks from every available host
vega notify resolve [<host>=<id> | --key <key> | --pane <pane>]
vega notify ack <host>=<id>          Seen; keep open, stop future escalation/reminder
vega notify test <channel>          Probe local toast or macos delivery

Defaults: pane = caller, project = checkout, key = <project>/<pane>.
One sentence, at most 120 characters. Never include secrets.
vegad delivers through seat channels; no config uses toast only. Phone routing lands separately.

Examples:
  vega notify action "orrery: please test apply -> w43:p1" --bead orrery-1
  vega notify resolve --key github.com/durandom/orrery/w43:p1
  vega notify ack proteus=1800000000000
