# Upstream snapshots

Forked from the installed herdr-app 0.1.15 on 2026-11-13 (read-only sources):

- `mods/tabs/main.js`: SHA256 `d1368ee4c1dd588adca3c20e015f8bf41c7e99e4b9591c910cc4aba14c90fe26`
- `mods/changes/main.js`: SHA256 `887b1ac632eef05b593da30dc7d1db92950b10174b2d7f9797de13dc5a894b21`
- Four `tabs/win-*.svg` icons and `changes/changes.svg` copied unchanged.

`tabs.js` keeps stock pill styling, scrolling, press/release gestures and window
controls. Its adapter uses profile layout groups, direct slot terminals and
`vega window` mutations instead of machine-space tabs. The passive title and
pill-level double-click rename avoid a competing label click target during drag.
Stable IDs retain the drag source across renders; full profile positions include
optional groups absent from the layout. The Config button replaces the stock
Files/Changes toggles; panel persistence is not needed for this dialog. `main.js`
owns input, machine choice and persistence. Space menus and native tab status
dots are omitted because composition groups have neither native tabs nor panes.

`diff.js` extracts changes' line numbering, tab expansion, row rendering and
virtualized, horizontally scrollable viewport. It has no git commit controls.
`window show` supplies a unified diff of the canonical applied repo profile and
its effective local overlay; this is the same base that Promote validates.

To rebase, compare the snapshots by hash, port stock render/gesture changes to
`tabs.js` and `diff.js`, then run fake mod tests and `scripts/check-mods.sh`.
