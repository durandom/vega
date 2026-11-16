#!/bin/sh
# Migration (docs/design/browser.md): cleanup.sh [session] [port] forwards
# to `vega browser cleanup [--session s] [--port p]`.
session=${1:-}
port=${2:-}
set --
if [ -n "$session" ]; then set -- --session "$session"; fi
if [ -n "$port" ]; then set -- "$@" --port "$port"; fi
exec vega browser cleanup "$@"
