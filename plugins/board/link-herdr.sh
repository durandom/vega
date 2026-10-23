#!/bin/sh
# Build ./bin/vega-board and link this plugin into herdr, but only from the
# main checkout. herdr keeps one registration per plugin id pointing at a
# directory; linking from a git worktree repoints every session at that
# worktree, and the plugin vanishes ("plugin action not found") once the
# worktree is removed.
set -eu
cd "$(dirname "$0")"

top=$(git rev-parse --show-toplevel)
main=$(dirname "$(git rev-parse --path-format=absolute --git-common-dir)")
if [ "$top" != "$main" ]; then
  echo "vega board: skipping herdr link from worktree $top (main checkout: $main)"
  exit 0
fi

cargo install --quiet --locked --path . --root .
herdr plugin link "$PWD" >/dev/null
echo "vega board: linked $PWD"
