vega notify [list] [--help]

List open asks from every available host, ignoring read/clear watermarks.
Fields: qualified id, level, age, project, pane, line and next step (command or bead).
If the registry is unavailable, print a warning and this host's asks only.

Examples:
  vega notify
  vega notify list
