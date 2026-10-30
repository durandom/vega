# vega review

Walks the owner through a running local web app. It opens a headed browser with a review overlay: the agent
highlights elements and asks, the owner points at elements or text and comments in the page, and the notes arrive
as `vega post` mail. Pointing beats describing.

Commands: `vega review start`, `highlight`, `reply`, `notes`, `stop`; for design feedback `patch` (live JS
patches that survive reload and navigation) and `shot` (element-cropped screenshots).

Design: [live-review.md](../../docs/design/live-review.md).

Agent guide: `vega skills review`

Headless CSP regression (requires `agent-browser` and its installed Chromium, uses an isolated session):

```sh
CARGO_BUILD_JOBS=6 nice cargo test -p vega-review strict_csp -- --ignored
```

It serves a strict CSP page and sends overlay notes through the real inbox after initial load, reload,
navigation and a cross-site round trip. It does not send mail or use the global browser profile.
