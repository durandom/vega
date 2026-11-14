# Upstream snapshot

Forked from the installed herdr-app 0.1.15 on 2026-11-13 (read-only sources).

- `tabs/main.js`: SHA256 `d1368ee4c1dd588adca3c20e015f8bf41c7e99e4b9591c910cc4aba14c90fe26`
- `tabs/mod.json`: SHA256 `bd2a09edf56933dd318e75ce134b2c48aed56010b825f3c936f92a995e686c4d`
- `tabs/win-close.svg`: SHA256 `bc46f314ea68818459856618e10f694e53cccd947b3069286f1988927e8a1fac`
- `tabs/win-max.svg`: SHA256 `d8f2df2b1c3b3416ab765be0ad0b6f453f4c89e0c01ac01e4e10bd12e01b109a`
- `tabs/win-min.svg`: SHA256 `c2f8d7d1e117f99b73970816877a926becb3da102cd706fd0f0ce1780259cd52`
- `tabs/win-restore.svg`: SHA256 `69ccde33c78d83ce39972ef129e158dd09fb19d3002bd709047801802fbad51d`

The working spinner is replaced with the static `theme.working` dot used by
vega-sidebar. Owned asset paths use the fork name; other behavior and SVGs
remain stock. Other mod assets remain upstream references. `mod.json` records
the clone base.

To rebase, compare upstream files against these hashes, port stock changes
keeping the static dot, then run fake mod tests and `scripts/check-mods.sh`.

Services adds an always-enabled right-stack toggle, following the Apache-2.0
ship-it-mind/herdr-ports pattern. See `../vega-services/UPSTREAM.md` for attribution.
