# vega review

Walks the owner through a running web app. It opens a headed browser with a review overlay: the agent
highlights elements and asks, the owner points at elements or text and comments in the page, and the notes arrive
as `vega post` mail. Pointing beats describing.

Commands: `vega review start`, `highlight`, `reply`, `notes`, `stop`; for design feedback `patch` (live JS
patches that survive reload and navigation) and `shot` (element-cropped screenshots).

Headed local reviews use shared Chrome for Testing and the Git project’s browser context.
Use `--headless` for an isolated scripted test. Use
`vega review start <url> --profile global` for a page that needs an existing
Chromium login, or `--profile vpn` for VPN access. The review gets its own tab
and prints the agent-browser session name. Use it with `agent-browser --session <name>`
from the same project. The owner signs in themselves.
`vega review stop` closes the review tab and inbox while the profile stays running.

Design: [live-review.md](../../docs/design/live-review.md) and [browser.md](../../docs/design/browser.md).

Agent guide: `vega skills review`

Headless CSP regression (requires `agent-browser` and its installed Chromium, uses an isolated session):

```sh
CARGO_BUILD_JOBS=6 nice cargo test -p vega-review strict_csp -- --ignored
```

It serves a strict CSP page and sends overlay notes through the real inbox after initial load, reload,
navigation and a cross-site round trip. It does not send mail or use the global browser profile.

The opt-in live regression uses a throwaway Chromium profile, a local fixture
page, and the pinned agent-browser version. Build `vega-browser` and
`vega-review`, then run from the repository root:

```sh
nice -n 10 python3 plugins/review/tests/tdeu_live.py --agent-browser /absolute/path/to/agent-browser
```
