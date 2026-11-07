#!/bin/sh
# install-cli.sh builds from the repo root even when started elsewhere
# (vega deploy runs it from ~ on the target).
set -eu

repo=$(cd "$(dirname "$0")/.." && pwd)
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
mkdir -p "$tmp/bin" "$tmp/home"
cat >"$tmp/bin/cargo" <<'EOF'
#!/bin/sh
pwd >"$CARGO_PWD_LOG"
EOF
chmod +x "$tmp/bin/cargo"

cd "$tmp"
# The fake cargo reports no binaries, so install-cli stops right after the build.
if CARGO_PWD_LOG="$tmp/pwd" HOME="$tmp/home" PATH="$tmp/bin:$PATH" \
	sh "$repo/scripts/install-cli.sh" >/dev/null 2>&1; then
	echo "install-cli.test: expected a stop after an empty build" >&2
	exit 1
fi
got=$(cat "$tmp/pwd")
if [ "$got" != "$repo" ]; then
	echo "install-cli.test: cargo ran in $got, want $repo" >&2
	exit 1
fi
echo "install-cli.test: ok"
