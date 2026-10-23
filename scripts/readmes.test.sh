#!/bin/sh
# Check the real repository and exercise inventory failures in an isolated fixture.
set -eu
root=$(cd "$(dirname "$0")/.." && pwd)
sh "$root/scripts/check-readmes.sh"

fixture=$(mktemp -d)
trap 'rm -rf "$fixture"' EXIT HUP INT TERM
mkdir -p "$fixture/scripts" "$fixture/plugins/demo" "$fixture/shims/tool"
cp "$root/scripts/check-readmes.sh" "$fixture/scripts/"
cat > "$fixture/README.md" <<'ROWS'
| Plugin | Purpose |
| --- | --- |
| [`demo`](plugins/demo/) | Demo |
| Shim | Purpose |
| --- | --- |
| [`tool`](shims/tool/) | Tool |
ROWS
cat > "$fixture/shims/README.md" <<'ROWS'
| Shim | Purpose |
| --- | --- |
| [`tool`](tool/) | Tool |
ROWS
touch "$fixture/plugins/demo/README.md" "$fixture/plugins/demo/SKILL.md" "$fixture/shims/tool/README.md"
check() { sh "$fixture/scripts/check-readmes.sh"; }
expect_failure() {
	if check > "$fixture/output" 2>&1; then
		echo "readmes regression: unexpectedly passed: $1" >&2
		exit 1
	fi
	if ! grep -F "$1" "$fixture/output" > /dev/null; then
		cat "$fixture/output" >&2
		echo "readmes regression: missing diagnostic: $1" >&2
		exit 1
	fi
}
check > /dev/null
for path in plugins/demo/README.md plugins/demo/SKILL.md shims/tool/README.md; do
	rm "$fixture/$path"
	expect_failure "missing: $path"
	touch "$fixture/$path"
done
for index in README.md shims/README.md; do
	cp "$fixture/$index" "$fixture/saved"
	# Remove each inventory row independently, including both root inventories.
	for target in $(awk -F '[()]' '/^\|.*\]\(/ { print $2 }' "$fixture/saved"); do
		awk -v target="($target)" 'index($0, target) == 0' "$fixture/saved" > "$fixture/$index"
		expect_failure "$index: expected one row linking $target, found 0"
	done
	cp "$fixture/saved" "$fixture/$index"
	case "$index" in README.md) targets="plugins/ghost/ shims/ghost/" ;; *) targets=ghost/ ;; esac
	for target in $targets; do
		printf '| [`ghost`](%s) | Missing |\n' "$target" >> "$fixture/$index"
		expect_failure "$index: row links missing directory $target"
		cp "$fixture/saved" "$fixture/$index"
	done
done
printf '| [`demo`](plugins/demo/) | Duplicate |\n' >> "$fixture/README.md"
expect_failure 'README.md: expected one row linking plugins/demo/, found 2'
echo 'readmes regression: ok'
