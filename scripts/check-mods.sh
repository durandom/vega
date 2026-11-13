#!/bin/sh
# herdr-app generates these files even when a check fails.
set -u
root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
mod=
cleanup() {
    if [ -n "$mod" ]; then
        rm -f -- "$mod/gpui-kit.d.ts" "$mod/jsconfig.json"
    fi
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
status=0
for mod in "$root"/config/herdr-app/mods/*; do
    [ -f "$mod/mod.json" ] || continue
    herdr-app check "$mod" || status=$?
    cleanup || exit 1
done
exit "$status"
