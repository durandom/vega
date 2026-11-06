#!/bin/sh
# Build ./bin/vega-open and link this plugin into herdr, but only from the
# main checkout. herdr keeps one registration per plugin id pointing at a
# directory; linking from a git worktree repoints every session at that
# worktree, and the plugin vanishes ("plugin action not found") once the
# worktree is removed.
set -eu
cd "$(dirname "$0")"

top=$(git rev-parse --show-toplevel)
main=$(dirname "$(git rev-parse --path-format=absolute --git-common-dir)")
if [ "$top" != "$main" ]; then
  echo "vega open: skipping herdr link from worktree $top (main checkout: $main)"
  exit 0
fi

# The binary scripts/install-cli.sh just built (VEGA_INSTALL_EXTRA), not a second build.
mkdir -p bin
install -m 755 "$HOME/.local/bin/vega-open" bin/.vega-open.new
mv -f bin/.vega-open.new bin/vega-open
herdr plugin link "$PWD" >/dev/null
echo "vega open: linked $PWD"
