---
name: vega-deploy
description: Shows which vega commit each herdr machine runs and builds and installs a pushed branch (main by default) on them over SSH. Use when asked to deploy, update or roll out vega to other machines, or to check which version a machine runs.
---

# vega deploy: install vega on other machines

`vega deploy` shows every herdr machine (`herdr machine list`) with its architecture, deployed commit, vega version and its vegad (`vega post relay`): `current`, `stale` (runs an older binary) or `stopped`. It changes nothing.

`vega deploy apply <machine>|--all` builds on the target over SSH: fetch and fast-forward of the pushed `main` from GitHub (or the branch selected with `--from-branch`), then `scripts/install-cli.sh`, then it replaces the target's `~/.config/vega/config/` (local config overrides) with this host's (column `overrides`: the file count; a file deleted here is deleted there), then `vega config apply` (column `config`: its summary, e.g. `written 2, unchanged 7, managed 2, skipped 0, rejected 0, failed 0`; units chezmoi still manages stay untouched; never fails the deploy). A running vegad execs the new binary by itself; install-cli restarts one that still runs the old binary after 15 s; a stopped vegad is started (`vega post relay start`). Push first; uncommitted or unpushed code is never deployed. It adds no herdr profile back to this machine: replies from other machines go through `vega post`. A new machine still needs `vega post keygen` and `vega post relay install` once (see `vega skills post`).

`--from-branch <branch>` checks out the selected branch, creating it locally if needed, or clones it on a fresh host. Divergent branches fail before install. The target stays on that branch; deploy again without the flag to return to `main`. While no checkout has main checked out, vegad gitsync skips main's fast-forward (docs/design/git-sync.md). Status always compares with pushed `main`, even after a feature deploy.
