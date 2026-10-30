vega notify resolve [<host>=<id> | --key <key> | --pane <pane>] [--help]

No selector: current checkout/pane's default key. Key and pane selectors are local;
qualified ids update the origin through its vegad. No open match is success (exit 0).

Examples:
  vega notify resolve
  vega notify resolve --pane w43:p1
  vega notify resolve proteus=1800000000000
