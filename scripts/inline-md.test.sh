#!/bin/sh
# Guides and long help texts live in .md files, embedded with include_str!.
# Fails on an inline `const SKILL: &str = "` and on any string literal over 15
# lines in plugins/*/src and shims/*/src, listing file:line. A literal that must
# stay (format! interpolation) gets `// inline-md: allow (<why>)` within the two
# lines above it.
set -eu

root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root"
max=15

files=$(find plugins/*/src shims/*/src -name '*.rs' | sort)
# shellcheck disable=SC2086
found=$(
	grep -n 'const SKILL: &str = "' $files | sed 's/^\([^:]*:[0-9]*\):.*/\1: inline SKILL guide/' || true
	awk -v max="$max" '
	FNR == 1 { st = 0; p1 = p2 = "" }
	{
		line = $0; n = length(line); i = 1
		while (i <= n) {
			c = substr(line, i, 1)
			if (st == 0) {
				if (substr(line, i, 2) == "//") break
				if (substr(line, i, 4) == "\x27\\\"\x27") { i += 4; continue }
				if (substr(line, i, 3) == "\x27\"\x27") { i += 3; continue }
				if (c == "\"") { st = 1; start = FNR; allow = (p1 p2 ~ /inline-md: allow/); i++; continue }
				if (c == "r" && (i == 1 || substr(line, i - 1, 1) !~ /[A-Za-z0-9_]/) &&
				    match(substr(line, i + 1), /^#*"/)) {
					hashes = RLENGTH - 1; st = 2; start = FNR; allow = (p1 p2 ~ /inline-md: allow/); i += RLENGTH + 1; continue
				}
			} else if (st == 1) {
				if (c == "\\") { i += 2; continue }
				if (c == "\"") { st = 0; if (!allow && FNR - start + 1 > max) print FILENAME ":" start ": " FNR - start + 1 " lines" }
			} else if (st == 2) {
				close_ = "\""; for (h = 0; h < hashes; h++) close_ = close_ "#"
				if (substr(line, i, hashes + 1) == close_) {
					st = 0; i += hashes + 1
					if (!allow && FNR - start + 1 > max) print FILENAME ":" start ": " FNR - start + 1 " lines"
					continue
				}
			}
			i++
		}
		p2 = p1; p1 = $0
	}' $files
)

if [ -n "$found" ]; then
	echo "$found" >&2
	echo "inline-md: $(echo "$found" | wc -l | tr -d ' ') found; move to plugins/<name>/SKILL.md or help/<cmd>.md, embed with include_str! (shape: plugins/bd/src/main.rs)" >&2
	exit 1
fi
echo "inline-md: ok"
