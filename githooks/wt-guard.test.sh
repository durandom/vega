#!/bin/sh
# Tests githooks/wt-guard against real git worktrees in a temp dir.
# Needs git >= 2.54 (config-based hooks); skips on older git.
set -eu

hook=$(cd "$(dirname "$0")" && pwd)/wt-guard
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

# Isolate from the user's git config (global hooksPath, other hooks).
export GIT_CONFIG_GLOBAL="$tmp/gitconfig" GIT_CONFIG_NOSYSTEM=1
unset VEGA_WT_GUARD
git config --global user.name test
git config --global user.email test@example.com
git config --global init.defaultBranch main
git config --global hook.vega-wt-guard.event post-checkout
git config --global hook.vega-wt-guard.command "$hook"

if ! git hook list post-checkout 2>/dev/null | grep -q vega-wt-guard; then
	echo "skip: git $(git --version) has no config-based hooks"
	exit 0
fi

fails=0
check() { # name, expect (hint|quiet), stderr file
	if grep -q '^hint: this worktree was made without wt' "$3"; then got=hint; else got=quiet; fi
	if [ "$got" = "$2" ]; then echo "ok   $1"; else
		echo "FAIL $1: want $2, got $got"; sed 's/^/     /' "$3"; fails=$((fails + 1))
	fi
}

repo() { # dir, wt.toml content ("" = none)
	git init -q "$1"
	if [ -n "$2" ]; then mkdir -p "$1/.config" && printf '%s\n' "$2" >"$1/.config/wt.toml"; fi
	git -C "$1" add -A && git -C "$1" commit -q --allow-empty -m init
}

repo "$tmp/r" '[post-start]
deps = "true"
pre-start = "true"'
cd "$tmp/r"

# stdout and exit code match a run with the hook off; stderr gets the hint.
git worktree add -b a ../a >"$tmp/out" 2>"$tmp/err"; code=$?
VEGA_WT_GUARD=0 git worktree add -b a0 ../a0 >"$tmp/out0" 2>/dev/null
check "worktree add -b in a wt repo" hint "$tmp/err"
[ "$code" = 0 ] && cmp -s "$tmp/out" "$tmp/out0" && echo "ok   stdout and exit code unchanged" ||
	{ echo "FAIL stdout/exit changed (exit $code)"; fails=$((fails + 1)); }
grep -q "wt -C $(cd ../a && pwd -P) hook post-start && wt -C $(cd ../a && pwd -P) hook pre-start" "$tmp/err" &&
	grep -q 'wt switch --create a ' "$tmp/err" && echo "ok   hint names the wt commands" ||
	{ echo "FAIL hint text:"; cat "$tmp/err"; fails=$((fails + 1)); }

git branch -q b
git worktree add ../b b 2>"$tmp/err" >/dev/null
check "worktree add of an existing branch" hint "$tmp/err"

git worktree add ../b 2>"$tmp/err" >/dev/null && code=0 || code=$?
[ "$code" != 0 ] && echo "ok   a failing worktree add keeps its exit code ($code)" ||
	{ echo "FAIL failing add exited 0"; fails=$((fails + 1)); }

VEGA_WT_GUARD=0 git worktree add -b c ../c 2>"$tmp/err" >/dev/null
check "VEGA_WT_GUARD=0" quiet "$tmp/err"

# A process named wt (stand-in for worktrunk) as an ancestor.
# A symlink keeps the name in ps; `; true` stops sh from exec'ing git.
ln -s /bin/sh "$tmp/wt"
"$tmp/wt" -c 'git worktree add -b d ../d; true' 2>"$tmp/err" >/dev/null
check "made by wt" quiet "$tmp/err"

git clone -q "$tmp/r" "$tmp/clone" 2>"$tmp/err"
check "clone" quiet "$tmp/err"

git checkout -q -b e 2>"$tmp/err"
check "plain branch checkout" quiet "$tmp/err"

repo "$tmp/plain" ""
git -C "$tmp/plain" worktree add -q -b x ../plain-x 2>"$tmp/err"
check "repo without wt.toml" quiet "$tmp/err"

repo "$tmp/nostart" '[pre-remove]
x = "true"'
git -C "$tmp/nostart" worktree add -q -b x ../nostart-x 2>"$tmp/err"
check "wt.toml without start hooks" quiet "$tmp/err"

[ "$fails" = 0 ] || { echo "$fails failed"; exit 1; }
