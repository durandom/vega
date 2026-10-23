# vega review

Walks the owner through a running local web app. It opens a headed browser with a review overlay: the agent
highlights elements and asks, the owner points at elements or text and comments in the page, and the notes arrive
as `vega post` mail. Pointing beats describing.

Commands: `vega review start`, `highlight`, `reply`, `notes`, `stop`; for design feedback `patch` (live JS
patches that survive reload and navigation) and `shot` (element-cropped screenshots).

Design: [live-review.md](../../docs/design/live-review.md).

Agent guide: `vega skills review`
