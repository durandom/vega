# vega deploy

Rolls vega out to the other herdr machines. It shows which commit and vegad each machine runs, and builds and
installs a pushed branch (`main` by default) on a target over SSH, then applies the vega config there.

Commands: `vega deploy`, `vega deploy apply <machine>|--all [--from-branch <branch>]`.

`apply` fetches and fast-forwards the selected pushed branch, defaulting to `main`. It checks out an
existing local branch or creates one from the fetched commit; a fresh host clones that branch. A
divergent branch fails before installation. For example:

```sh
vega deploy apply ganymede --from-branch my-fix
vega deploy apply ganymede # return its checkout to main
```

The checkout stays on the selected branch. If no checkout has `main` checked out, vegad gitsync skips
main's fast-forward ([git-sync.md](../../docs/design/git-sync.md)). `vega deploy` status always compares
against pushed `main`, including after a feature branch deploy. Config overrides and `vega config apply`
still run after installation; install-cli's restart fallback and deploy's stopped-vegad start still apply.

Design: [upgrade.md](../../docs/design/upgrade.md) (deploy targets), [vegad.md](../../docs/design/vegad.md).

Agent guide: `vega skills deploy`
