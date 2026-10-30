vega notify ack <host>=<id> [--help]

Record that the operator saw an ask. Keep it open; future delivery must stop
escalation and reminders. Already acked or resolved is a successful no-op.
Remote ids require the origin vegad to be running with notify support installed.

Examples:
  vega notify ack proteus=1800000000000
  vega notify ack ganymede=1800000000001
