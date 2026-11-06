#!/bin/sh
# check-stamp.sh write|fresh: remembers the clean tree `mise run check` passed
# on, so `mise run install` skips a check that already passed (vega-919z).
# A dirty tree never gets a stamp; `fresh` fails unless HEAD's clean tree has one.
set -eu
root=$(cd "$(dirname "$0")/.." && pwd)
stamp="${CARGO_TARGET_DIR:-$root/target}/.check-passed"
tree() {
	[ -z "$(git -C "$root" status --porcelain)" ] && git -C "$root" rev-parse 'HEAD^{tree}'
}
case ${1:-} in
write)
	if t=$(tree); then
		mkdir -p "$(dirname "$stamp")"
		echo "$t" >"$stamp"
	else
		rm -f "$stamp"
	fi
	;;
fresh)
	t=$(tree) && [ -f "$stamp" ] && [ "$(cat "$stamp")" = "$t" ]
	;;
*)
	echo "usage: check-stamp.sh write|fresh" >&2
	exit 2
	;;
esac
