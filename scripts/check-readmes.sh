#!/bin/sh
# Directory inventories use a linked first table cell: [`name`](path/).
# Check only those indexes, not arbitrary prose or historical command examples.
set -eu

root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root"
errors=0
report() {
	echo "readmes: $*" >&2
	errors=$((errors + 1))
}

check_index() {
	index=$1
	kind=$2
	prefix=$3
	# Extract first-cell Markdown link destinations from inventory rows.
	rows=$(awk -F '|' '
		/^\|/ && $2 ~ /\]\([^)]+\)/ {
			cell = $2
			sub(/^.*\]\(/, "", cell)
			sub(/\).*$/, "", cell)
			print cell
		}' "$index")
	for dir in "$kind"/*/; do
		[ -d "$dir" ] || continue
		name=${dir%/}
		name=${name##*/}
		target=$prefix$name/
		count=$(printf '%s\n' "$rows" | grep -Fxc "$target" || true)
		[ "$count" -eq 1 ] || report "$index: expected one row linking $target, found $count"
	done
	for target in $rows; do
		case "$target" in
			"$prefix"*)
				[ -d "$(dirname "$index")/$target" ] || report "$index: row links missing directory $target"
				;;
		esac
	done
}

for dir in plugins/*/ shims/*/; do
	[ -d "$dir" ] || continue
	[ -f "${dir}README.md" ] || report "missing: ${dir}README.md"
done
for dir in plugins/*/; do
	[ -d "$dir" ] || continue
	[ -f "${dir}SKILL.md" ] || report "missing: ${dir}SKILL.md"
done
check_index README.md plugins plugins/
check_index README.md shims shims/
check_index shims/README.md shims ''

if [ "$errors" -gt 0 ]; then
	echo "readmes: $errors error(s); update the linked inventory rows and README.md/SKILL.md files" >&2
	exit 1
fi
echo "readmes: ok"
