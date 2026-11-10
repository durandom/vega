#!/bin/sh
# Remove retired command binaries after a successful CLI build/install.
# Existing herdr configuration and registrations remain host-owned.
set -eu
for plugin in board open; do
    for dir in "$HOME/.local/bin" "$HOME/.local/share/vega/shims/bin"; do
        rm -f "$dir/vega-$plugin"
    done
done
