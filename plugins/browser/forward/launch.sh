#!/bin/sh
# Migration (docs/design/browser.md): launch.sh <url> [session] [port]
# forwards to `vega browser launch <url> [--session s] [--port p]`.
url=${1:?usage: launch.sh <url> [session] [port]}
session=${2:-}
port=${3:-}
if [ -n "$session" ]; then set -- "$url" --session "$session"; else set -- "$url"; fi
if [ -n "$port" ]; then set -- "$@" --port "$port"; fi
exec vega browser launch "$@"
