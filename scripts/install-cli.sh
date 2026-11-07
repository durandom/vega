#!/bin/sh
# Installs the vega CLI, its portable plugins, the agent-only shims and the
# global skill link. Used by `mise run install` here and by `vega deploy` on
# other machines, so it must not need mise, VS Code or a GUI.
set -eu

root=$(cd "$(dirname "$0")/.." && pwd)
# cargo build finds the workspace from the cwd; vega deploy runs this from ~.
cd "$root"

# Build in this checkout's own target dir. With a CARGO_TARGET_DIR shared by
# several worktrees, `cargo install` can find its build "fresh" and copy the
# binary another worktree built last (seen 2026-10-07: an older vega-post got
# installed).
export CARGO_TARGET_DIR="$root/target"

# One release build for every binary, then copied (vega-919z): a `cargo
# install --path` per package resolved features and linked each on its own.
# Shims go to the agent-only dir: they must not shadow tools for humans (P3);
# the git shim is the push gate (docs/design/gate.md), the bd shim sets a
# per-worktree BEADS_ACTOR (docs/design/beads-sync.md).
# VEGA_INSTALL_EXTRA adds packages (mise run install: vega-open, vega-board).
bins="$HOME/.local/bin"
shims="$HOME/.local/share/vega/shims/bin"
pkgs="vega vega-mux vega-post vega-history vega-notify vega-deploy vega-skills
vega-orchestration vega-uac vega-ci vega-theme vega-uber vega-config vega-onboard
vega-lanista vega-capacity vega-janitor vega-review vega-gate vega-bd vega-secrets-cli
herdr-shim gh-shim tea-shim git-shim bd-shim ${VEGA_INSTALL_EXTRA:-}"
args=""
for p in $pkgs; do args="$args -p $p"; done
mkdir -p "$bins" "$shims"
built=$(mktemp)
trap 'rm -f "$built" "$built.json"' EXIT
# To a file, not a pipe: sh has no pipefail, a failed build must stop here.
# shellcheck disable=SC2086 # $args is a word list
cargo build --locked --release $args --message-format=json-render-diagnostics >"$built.json"
sed -n 's/.*"package_id":"\([^"]*\)".*"executable":"\([^"]*\)".*/\1 \2/p' "$built.json" >"$built"
[ -s "$built" ] || {
	echo "install-cli: cargo build reported no binaries" >&2
	exit 1
}
while read -r id exe; do
	case $id in
	*/shims/*) dest=$shims ;;
	*) dest=$bins ;;
	esac
	name=$(basename "$exe")
	# A new inode, never an overwrite in place: a running vegad keeps its binary.
	install -m 755 "$exe" "$dest/.$name.new"
	mv -f "$dest/.$name.new" "$dest/$name"
done <"$built"
echo "install-cli: $(wc -l <"$built" | tr -d ' ') binaries installed" >&2
# vega-skill was renamed vega-skills and `vega skill` is gone; a stale binary
# would answer `vega skill` with an old guide. Drop it with its cargo install record.
if [ -e "$bins/vega-skill" ]; then
	cargo uninstall --root "$HOME/.local" vega-skill >/dev/null 2>&1 || rm -f "$bins/vega-skill"
fi
# direnv: `use vega <tool>` in an .envrc loads that tool's secrets (docs/design/secrets.md).
direnv_lib="${XDG_CONFIG_HOME:-$HOME/.config}/direnv/lib"
mkdir -p "$direnv_lib"
install -m 644 "$root/plugins/secrets/direnv/vega.sh" "$direnv_lib/vega.sh"

# Agent hooks (docs/design/config.md, units claude.hooks, codex.hooks, pi.prefix):
# the ask-parent PreToolUse hook (a spawned agent asks its parent, not the user
# in its pane) for Claude and codex, and the P3 shim activation: Claude sources
# activate.sh through a SessionStart hook and CLAUDE_ENV_FILE (it overwrites PATH
# from its shell snapshot), pi through its shellCommandPrefix; codex runs `zsh -c`,
# so zsh startup files cover it (chezmoi-managed, see P3). Claude hooks use the
# vega-hooks plugin, migrating legacy settings hooks after install verification.
# Codex hooks stay key-merged, with <file>.vega-bak. After a hook change every
# new codex session stops at "Hooks need review" until the user trusts it once.
# Only the hooks: the personal units (instructions, styles, settings keys) are
# applied by `vega deploy` or an explicit `vega config apply`, never here.
install -m 644 "$root/shims/activate.sh" "$HOME/.local/share/vega/shims/activate.sh"
VEGA_CONFIG_PHASE=install "$HOME/.local/bin/vega-config" apply hooks ||
	echo "warning: vega config apply hooks did not complete; see \`vega config hooks\`" 1>&2

# Global git hooks, config-based (git >= 2.54): they run next to core.hooksPath
# and repo hooks, for agents, scripts and humans alike. Copied, not linked: a
# worktree's path dies with the worktree. The config lives in a vega-owned file,
# included from the XDG git config: ~/.gitconfig is chezmoi-managed.
hooks="$HOME/.local/share/vega/githooks"
mkdir -p "$hooks"
install -m 755 "$root/githooks/wt-guard" "$hooks/wt-guard"
vega_gitconfig="$HOME/.config/vega/git/hooks.gitconfig"
mkdir -p "$(dirname "$vega_gitconfig")"
cat >"$vega_gitconfig" <<EOF
# Written by vega scripts/install-cli.sh; do not edit.
[hook "vega-wt-guard"]
	event = post-checkout
	command = $hooks/wt-guard
EOF
xdg_gitconfig="${XDG_CONFIG_HOME:-$HOME/.config}/git/config"
mkdir -p "$(dirname "$xdg_gitconfig")"
git config --file "$xdg_gitconfig" --get-all include.path 2>/dev/null | grep -qxF "$vega_gitconfig" ||
	git config --file "$xdg_gitconfig" --add include.path "$vega_gitconfig"
git hook list post-checkout 2>/dev/null | grep -q vega-wt-guard ||
	echo "warning: $(git --version) ignores config-based hooks; wt-guard needs git >= 2.54" >&2

# lanista (the model catalog) from git main, and its seats file from this repo,
# so `vega deploy` keeps every machine current. Never fatal: without uv it is
# skipped, and an editable install (a dev checkout) is left alone; the output
# says what `vega lanista install --replace` would do.
if command -v uv >/dev/null; then
	"$HOME/.local/bin/vega-lanista" install >&2 ||
		echo "warning: vega lanista install did not complete; see \`vega lanista\`" >&2
else
	echo "warning: uv missing; lanista not installed (see \`vega skills lanista\`)" >&2
fi

# Global skill stub; the guides themselves come from `vega skills` (P5).
mkdir -p "$HOME/.agents/skills" "$HOME/.claude/skills"
# Always link the main checkout: a worktree's path dies with the worktree.
main=$(dirname "$(git -C "$root" rev-parse --path-format=absolute --git-common-dir)")
ln -sfn "$main/skills/vega" "$HOME/.agents/skills/vega"
ln -sfn ../../.agents/skills/vega "$HOME/.claude/skills/vega"

# Install record (docs/design/upgrade.md): vegad checks this checkout's
# upstream daily and `vega upgrade` pulls it; `vega deploy` sets
# VEGA_INSTALL_BY=deploy, and such hosts never check.
state="${XDG_STATE_HOME:-$HOME/.local/state}/vega"
mkdir -p "$state"
printf '{"checkout": "%s", "commit": "%s", "by": "%s", "at": %s}\n' \
	"$main" "$(git -C "$root" rev-parse HEAD)" "${VEGA_INSTALL_BY:-local}" "$(date +%s)" \
	>"$state/install.json.tmp"
mv "$state/install.json.tmp" "$state/install.json"

# A running vegad that has not exec'd the new vega-post by itself gets
# restarted; one that is not running stays stopped.
"$root/scripts/vegad-refresh.sh" "$HOME/.local/bin/vega-post" ||
	echo "warning: vegad restart did not complete; see \`vega post relay status\`" >&2
