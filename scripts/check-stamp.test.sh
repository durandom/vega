#!/bin/sh
# Tests scripts/check-stamp.sh: a stamp only for a clean tree, stale after a change.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
d=$(mktemp -d)
trap 'rm -rf "$d"' EXIT
mkdir -p "$d/scripts"
cp "$here/check-stamp.sh" "$d/scripts/"
echo target/ >"$d/.gitignore"
cd "$d"
git init -q
git -c user.name=t -c user.email=t@t add scripts .gitignore
git -c user.name=t -c user.email=t@t commit -qm init
export CARGO_TARGET_DIR="$d/target"
s="sh scripts/check-stamp.sh"
fail() { echo "check-stamp.test: $1" >&2; exit 1; }
$s fresh && fail "fresh without a stamp"
$s write
$s fresh || fail "not fresh after write on a clean tree"
echo x >new
$s fresh && fail "fresh on a dirty tree"
$s write
[ -f target/.check-passed ] && fail "stamp written for a dirty tree"
git -c user.name=t -c user.email=t@t add new
git -c user.name=t -c user.email=t@t commit -qm new
$s fresh && fail "fresh for another tree"
$s bogus 2>/dev/null && fail "unknown verb accepted"
echo "check-stamp.test: ok"
