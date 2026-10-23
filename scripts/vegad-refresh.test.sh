#!/bin/sh
# Tests scripts/vegad-refresh.sh against a fake vega-post: a stale vegad is
# restarted, a current, stopped or self-exec'ing one is left alone.
set -eu

script=$(cd "$(dirname "$0")" && pwd)/vegad-refresh.sh
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

# The fake answers `relay status --json` with the next line of $tmp/states
# (the last one repeats) and logs stop/start to $tmp/calls.
cat >"$tmp/vega-post" <<EOF
#!/bin/sh
case "\$2" in
status)
	n=\$(cat "$tmp/n" 2>/dev/null || echo 1)
	line=\$(sed -n "\${n}p" "$tmp/states")
	[ -n "\$line" ] || line=\$(tail -n 1 "$tmp/states")
	echo \$((n + 1)) >"$tmp/n"
	echo "\$line" ;;
*) echo "\$2" >>"$tmp/calls" ;;
esac
EOF
chmod +x "$tmp/vega-post"

running='{"running":true,"binary":"current","pid":"1"}'
stale='{"running":true,"binary":"stale","pid":"1"}'
stopped='{"running":false,"binary":"-"}'

fails=0
check() { # name, expected calls (space-separated), states...
	name=$1 want=$2
	shift 2
	printf '%s\n' "$@" >"$tmp/states"
	: >"$tmp/calls"
	rm -f "$tmp/n"
	HOME="$tmp" VEGAD_REFRESH_WAIT=2 sh "$script" "$tmp/vega-post" 2>/dev/null
	got=$(tr '\n' ' ' <"$tmp/calls" | sed 's/ $//')
	if [ "$got" = "$want" ]; then echo "ok   $name"; else
		echo "FAIL $name: want '$want', got '$got'"; fails=$((fails + 1))
	fi
}

check "current: nothing" "" "$running"
check "stopped: never started" "" "$stopped"
check "self-exec within the wait: nothing" "" "$stale" "$running"
check "still stale: restarted" "stop start" "$stale"
rm "$tmp/vega-post"
HOME="$tmp" sh "$script" "$tmp/vega-post" && echo "ok   missing binary exits 0" ||
	{ echo "FAIL missing binary"; fails=$((fails + 1)); }

[ "$fails" -eq 0 ]
