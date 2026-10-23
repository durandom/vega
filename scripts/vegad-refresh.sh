#!/bin/sh
# Restarts a running vegad that still runs a replaced binary (vega-lhc.122).
# Called by install-cli.sh after the install, so `mise run install-cli`,
# `vega upgrade` and `vega deploy` all end with a current vegad.
#
# A healthy vegad execs a replaced binary by itself within seconds, state
# handed over (docs/design/vegad.md, Lifecycle); it waits for a quiet point,
# and one that never comes kept an old vegad on proteus for a day. So wait
# for the self-exec first, and only then restart (which drops the in-memory
# state the handoff would keep). Not running: left alone, never started.
#
# Usage: vegad-refresh.sh [<vega-post>]   (default ~/.local/bin/vega-post)
# VEGAD_REFRESH_WAIT: seconds to wait for the self-exec (default 15).
set -eu

post=${1:-$HOME/.local/bin/vega-post}
wait=${VEGAD_REFRESH_WAIT:-15}
[ -x "$post" ] || exit 0

# running | stale | stopped, from `relay status --json` (compact serde JSON).
state() {
	s=$("$post" relay status --json 2>/dev/null) || {
		echo unknown
		return
	}
	case "$s" in
	*'"running":false'*) echo stopped ;;
	*'"binary":"stale"'*) echo stale ;;
	*) echo running ;;
	esac
}

i=0
while :; do
	case $(state) in
	stale) ;;
	*) exit 0 ;;
	esac
	[ "$i" -lt "$wait" ] || break
	sleep 1
	i=$((i + 1))
done

echo "vegad: still runs a replaced binary after ${wait} s; restarting it" >&2
"$post" relay stop >&2
"$post" relay start >&2
