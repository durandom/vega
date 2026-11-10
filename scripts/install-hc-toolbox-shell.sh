#!/bin/sh
# Makes new herdr-core terminals start zsh inside the toolbox with HC_* intact
# (docs/design/hc-migration.md, slice 0 finding 5). Installs the wrapper to
# ~/.local/bin and points `shell` in herdr-core's config at it.
# Also makes herdr-app's Claude menu entry run in the toolbox (programs.json).
# Toolbox name: $HC_TOOLBOX, else the container we run in. Skips (exit 0) without
# toolbox or a name; never overwrites a `shell` the user set to something else.
set -eu

src=$(cd "$(dirname "$0")/.." && pwd)/config/herdr-core/hc-toolbox-shell
bin=${HC_TOOLBOX_SHELL_BIN:-$HOME/.local/bin}/hc-toolbox-shell
cfg=${XDG_CONFIG_HOME:-$HOME/.config}/herdr-core/config.toml
containerenv=${HC_CONTAINERENV:-/run/.containerenv}

command -v toolbox >/dev/null || {
	echo "skip: toolbox not on PATH; hc terminals keep the default shell"
	exit 0
}
name=${HC_TOOLBOX:-}
if [ -z "$name" ] && [ -r "$containerenv" ]; then
	name=$(sed -n 's/^name="\(.*\)"$/\1/p' "$containerenv")
fi
[ -n "$name" ] || {
	echo "skip: no toolbox name; set HC_TOOLBOX=<container> and rerun"
	exit 0
}

mkdir -p "$(dirname "$bin")" "$(dirname "$cfg")"
sed "s/__TOOLBOX__/$name/" "$src" >"$bin.tmp"
chmod +x "$bin.tmp"
mv "$bin.tmp" "$bin"

# herdr-app starts menu agents in the host shell; run Claude in the toolbox too.
prog=${XDG_CONFIG_HOME:-$HOME/.config}/herdr-app/programs.json
if command -v python3 >/dev/null && [ -d "$(dirname "$prog")" ]; then
	python3 - "$prog" "$bin" <<'PY'
import json, os, sys
path, wrapper = sys.argv[1:]
d = json.load(open(path)) if os.path.exists(path) else {}
c = d.setdefault("programs", {}).setdefault("claude", {})
if "run" not in c:
    c["run"] = [wrapper, "claude"]
    json.dump(d, open(path, "w"), indent=2)
    print("ok: %s claude runs in the toolbox" % path)
PY
fi

want="shell = [\"$bin\"]"
if [ ! -e "$cfg" ] || ! grep -q '^shell *=' "$cfg"; then
	printf '%s\n' "$want" >>"$cfg"
	echo "ok: $cfg shell -> $bin (toolbox $name); new terminals only"
elif grep -qxF "$want" "$cfg"; then
	echo "ok: hc-toolbox-shell ($name) up to date"
else
	echo "skip: $cfg already sets another shell; point it at $bin by hand"
fi
