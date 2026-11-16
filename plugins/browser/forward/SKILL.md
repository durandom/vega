---
name: browser
description: Forwarding stub while hard-coded paths migrate. Use when an old script or guide still refers to the browser skill; the real guide now loads with vega skills browser.
---

# browser (moved into a vega plugin)

The browser skill moved into the vega plugin `vega-browser`. Load its guide
with `vega skills browser` before the first browser command. The old script
paths forward to it:

- `scripts/ab` → `vega browser ab`
- `scripts/chromium.sh` → `vega browser chromium`
- `scripts/launch.sh` → `vega browser launch`
- `scripts/cleanup.sh` → `vega browser cleanup`

This stub and the forwarding scripts exist only for the migration
(docs/design/browser.md); they are removed once the remaining hard-coded
callers switched. If `vega` is not on PATH, report that and stop; do not
guess.
