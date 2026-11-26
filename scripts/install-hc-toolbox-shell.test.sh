#!/bin/sh
# Tests scripts/install-hc-toolbox-shell.sh against a fake HOME and fake toolbox.
set -eu

script=$(cd "$(dirname "$0")" && pwd)/install-hc-toolbox-shell.sh
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
sh=$(command -v sh)
mkdir "$tmp/bin" "$tmp/home"
run() { env -i PATH="$tmp/bin:/usr/bin:/bin" HOME="$tmp/home" HC_CONTAINERENV="$tmp/none" "$@" "$sh" "$script"; }
fail() { echo "FAIL: $*"; exit 1; }

case $(run) in skip:*toolbox*) ;; *) fail "no toolbox must skip" ;; esac
printf '#!%s\n' "$sh" >"$tmp/bin/toolbox"
chmod +x "$tmp/bin/toolbox"
case $(run) in skip:*name*) ;; *) fail "no name must skip" ;; esac
[ ! -e "$tmp/home/.config" ] || fail "skip wrote config"

run HC_TOOLBOX=box >/dev/null
bin=$tmp/home/.local/bin/hc-toolbox-shell
cfg=$tmp/home/.config/herdr-core/config.toml
[ -x "$bin" ] || fail "wrapper not executable"
grep -q 'toolbox run -c "box" ' "$bin" || fail "name not baked in"
[ "$(cat "$cfg")" = "shell = [\"$bin\"]" ] || fail "config: $(cat "$cfg")"
run HC_TOOLBOX=box >/dev/null
[ "$(wc -l <"$cfg")" -eq 1 ] || fail "rerun duplicated the shell line"

echo 'shell = ["nu"]' >"$cfg"
case $(run HC_TOOLBOX=box) in skip:*another*) ;; *) fail "foreign shell must be kept" ;; esac
[ "$(cat "$cfg")" = 'shell = ["nu"]' ] || fail "foreign shell overwritten"
echo "ok: install-hc-toolbox-shell"
