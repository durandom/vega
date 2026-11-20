# vega review

Walks the owner through a running web app. It opens a headed browser with a review overlay: the agent
highlights elements and asks, the owner points at elements or text and comments in the page, and the notes arrive
as `vega post` mail. Pointing beats describing.

Commands: `vega review start`, `highlight`, `reply`, `notes`, `stop`; for design feedback `patch` (live JS
patches that survive reload and navigation) and `shot` (element-cropped screenshots).

By default, review accepts a local dev URL and opens an isolated browser. Use
`vega review start <url> --profile global` for a page that needs an existing
Chromium login, or `--profile vpn` for VPN access. The review gets its own tab
and prints the pinned agent-browser session name. The owner signs in themselves.
`vega review stop` closes the review tab and inbox while the profile stays running.

Design: [live-review.md](../../docs/design/live-review.md) and [browser.md](../../docs/design/browser.md).

Agent guide: `vega skills review`

Headless CSP regression (requires `agent-browser` and its installed Chromium, uses an isolated session):

```sh
CARGO_BUILD_JOBS=6 nice cargo test -p vega-review strict_csp -- --ignored
```

It serves a strict CSP page and sends overlay notes through the real inbox after initial load, reload,
navigation and a cross-site round trip. It does not send mail or use the global browser profile.
