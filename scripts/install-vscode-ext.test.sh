#!/bin/sh
# Tests scripts/install-vscode-ext.sh: without `code` on PATH it prints the
# manual-install hint and exits 0; with a (fake) `code` it installs the vsix.
set -eu

script=$(cd "$(dirname "$0")" && pwd)/install-vscode-ext.sh
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
sh=$(command -v sh)
mkdir "$tmp/bin"

out=$(PATH="$tmp/bin" "$sh" "$script") || {
	echo "FAIL: exit $? without code on PATH"
	exit 1
}
[ "$out" = "skip: code not on PATH; install vscode/dist/vega-vscode.vsix by hand" ] || {
	echo "FAIL: hint without code: $out"
	exit 1
}

printf '#!%s\necho "$@" >"%s/calls"\n' "$sh" "$tmp" >"$tmp/bin/code"
chmod +x "$tmp/bin/code"
PATH="$tmp/bin" "$sh" "$script" >/dev/null
[ "$(cat "$tmp/calls")" = "--install-extension vscode/dist/vega-vscode.vsix --force" ] || {
	echo "FAIL: code called with: $(cat "$tmp/calls")"
	exit 1
}
echo "ok: install-vscode-ext"
