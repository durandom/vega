#!/bin/sh
# Tests shims/activate.sh: the shim dir goes first on PATH only in a
# non-interactive shell with an agent env marker, in sh, bash and zsh.
set -eu

script=$(cd "$(dirname "$0")" && pwd)/activate.sh
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
shims="$tmp/.local/share/vega/shims/bin"

fails=0
check() { # name, expect (on|off), shell, env assignments...
	name=$1 want=$2 sh=$3
	shift 3
	first=$(env -i HOME="$tmp" PATH=/usr/bin:/bin "$@" "$sh" -c ". '$script'; . '$script'; printf %s \"\$PATH\"")
	case "$first" in
	"$shims:/usr/bin:/bin") got=on ;;
	/usr/bin:/bin) got=off ;;
	*) got="unexpected PATH $first" ;;
	esac
	if [ "$got" = "$want" ]; then echo "ok   $name"; else
		echo "FAIL $name: want $want, got $got"; fails=$((fails + 1))
	fi
}

for sh in sh bash zsh; do
	command -v "$sh" >/dev/null || { echo "skip $sh: not installed"; continue; }
	check "$sh: no marker" off "$sh"
	check "$sh: empty marker" off "$sh" CLAUDECODE=
	check "$sh: claude" on "$sh" CLAUDECODE=1
	check "$sh: codex" on "$sh" CODEX_THREAD_ID=x
	check "$sh: pi" on "$sh" PI_CODING_AGENT=true
	check "$sh: AI_AGENT" on "$sh" AI_AGENT=other
done

# An interactive shell stays clean even with a leaked marker.
if command -v zsh >/dev/null; then
	got=$(env -i HOME="$tmp" PATH=/usr/bin:/bin CLAUDECODE=1 zsh -f -i -c ". '$script'; printf %s \"\$PATH\"" 2>/dev/null)
	if [ "$got" = /usr/bin:/bin ]; then echo "ok   zsh -i: marker ignored"; else
		echo "FAIL zsh -i: PATH $got"; fails=$((fails + 1))
	fi
fi

# Already first after a reorder: moved back to the front.
got=$(env -i HOME="$tmp" PATH="/usr/bin:$shims:/bin" CLAUDECODE=1 sh -c ". '$script'; printf %s \"\$PATH\"")
if [ "$got" = "$shims:/usr/bin:$shims:/bin" ]; then echo "ok   reordered PATH: shims first again"; else
	echo "FAIL reordered PATH: $got"; fails=$((fails + 1))
fi

[ "$fails" -eq 0 ]
