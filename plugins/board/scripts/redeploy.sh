#!/usr/bin/env bash
# Close every beads pane so they respawn on the latest release build.
# Run after `cargo build --release` while iterating on the binary.
set -uo pipefail
DIR="$(cd "$(dirname "${BASH_SOURCE[0]:-$0}")" && pwd)"
# shellcheck source=lib.sh
. "$DIR/lib.sh"

# The marker is a substring match, so this catches dock and board alike.
# No mapfile: herdr runs macOS's /bin/bash 3.2. Pane ids hold no spaces.
n=0
for pid in $(beads_panes_by_title vega-board); do
  "$HERDR_BIN" pane close "$pid" >/dev/null 2>&1
  n=$((n + 1))
done
echo "closed $n board pane(s)"
