# Upstream snapshot

Forked from the installed herdr-app 0.1.15 on 2026-11-13 (read-only sources).

- `pane/close.svg`: SHA256 `532eb25ce41976594b69b3735e1e2f892f18f39288da7e420832b9ee66dc4ff6`
- `pane/main.js`: SHA256 `9bb9028df04281ad886165ec0eb9fc0a8ac6efadf39abc4d6aa0afd1370879c7`
- `pane/mod.json`: SHA256 `a10f79e972a197d0d51b37009180ac0e5518add50d601844b73cfb06539e2989`
- `pane/split-down.svg`: SHA256 `a9cbd394b0fe8bbb1881480dfb84072c2493f85e335b0b4c7de94b53d4a10d69`
- `pane/split-right.svg`: SHA256 `55b7bb600aad8463103f8848219ca8bcdc4cc620cdcb52325dde007f8275a637`
- `pane/unzoom.svg`: SHA256 `a061af514f83278f3b5f0bea6388b116ce66969897b202cd5709f4970b8750c6`
- `pane/zoom.svg`: SHA256 `e2d29d06235819c0b617029291adc8a9f4d31dc59264a50e97e55470e9ad2823`

The working spinner is replaced with the static `theme.working` dot used by
vega-sidebar. Owned asset paths use the fork name; other behavior and SVGs
remain stock. Other mod assets remain upstream references. `mod.json` records
the clone base.

To rebase, compare upstream files against these hashes, port stock changes
keeping the static dot, then run fake mod tests and `scripts/check-mods.sh`.
