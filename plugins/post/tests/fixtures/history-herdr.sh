#!/bin/sh
if [ "$1" = "$VEGA_SNAPSHOT_FAIL" ] && [ "$2" = list ]; then
    echo '{"error":{"code":"snapshot_failed","message":"fixture snapshot failure"}}'
    exit 1
fi
case "$1 $2" in
    'agent list') echo '{"result":{"agents":[]}}' ;;
    'workspace list') echo '{"result":{"workspaces":[]}}' ;;
    *) echo '{"result":{}}' ;;
esac
