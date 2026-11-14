# Sources and licenses

Read-only research clones, inspected 2026-11-14:

- https://github.com/ship-it-mind/herdr-ports at
  `5bf9c3baada5b88d83ea08eec5d3972536a8be48` — Apache-2.0,
  Copyright 2026 Ahmed Sultan. Preferred reference for the side-stack Services
  button and machine inventory/exec pattern. Existing vega-tabs remains our
  stock Herdr fork; this addition adapts the always-enabled panel pattern.
- https://github.com/antoine-sh/herdr-ports at
  `cb2e41d17598d1d173e89c48461c3f1b87ad7678` — MIT, Copyright 2026 Antoine.
  Read for forward toggles and close cleanup; no code copied.
- https://github.com/Numbered-com/herdr-ports at
  `ac0583d9fdf9941f9b1ca281857e858e395f21c8` — no license found.
- https://github.com/lucidstack/herdr-services at
  `1d9f061d7aae0170e598ad478240bd9620b3f034` — no license found.
  These last two are ideas only: ancestry before most-specific cwd. No code copied.

The Services implementation is original. It uses manual refresh, Unix TCP
snapshots, project labels, explicitly owned SSH control sockets and no automatic
forwarding, detached hc terminals, UI timers, input widgets or animations.
It deliberately does not copy the upstream process-wide pkill cleanup or
survive-close tunnel behavior. HTTP links are defaults, not protocol detection.
