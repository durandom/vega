#!/bin/sh
# Installs the vega CLI, its portable plugins, the agent-only shims and the
# global skill link. Used by `mise run install` here and by `vega deploy` on
# other machines, so it must not need mise, VS Code or a GUI.
set -eu

root=$(cd "$(dirname "$0")/.." && pwd)

# Build in this checkout's own target dir. With a CARGO_TARGET_DIR shared by
# several worktrees, `cargo install` can find its build "fresh" and copy the
# binary another worktree built last (seen 2026-10-07: an older vega-post got
# installed).
export CARGO_TARGET_DIR="$root/target"

cargo install --locked --path "$root/vega" --root "$HOME/.local"
cargo install --locked --path "$root/plugins/mux" --root "$HOME/.local"
cargo install --locked --path "$root/plugins/post" --root "$HOME/.local"
cargo install --locked --path "$root/plugins/deploy" --root "$HOME/.local"
cargo install --locked --path "$root/plugins/skills" --root "$HOME/.local"
# vega-skill was renamed vega-skills and `vega skill` is gone; a stale binary
# would answer `vega skill` with an old guide. Drop it with its cargo install record.
if [ -e "$HOME/.local/bin/vega-skill" ]; then
	cargo uninstall --root "$HOME/.local" vega-skill >/dev/null 2>&1 || rm -f "$HOME/.local/bin/vega-skill"
fi
cargo install --locked --path "$root/plugins/orchestration" --root "$HOME/.local"
cargo install --locked --path "$root/plugins/uac" --root "$HOME/.local"
cargo install --locked --path "$root/plugins/ci" --root "$HOME/.local"
cargo install --locked --path "$root/plugins/theme" --root "$HOME/.local"
cargo install --locked --path "$root/plugins/uber" --root "$HOME/.local"
cargo install --locked --path "$root/plugins/config" --root "$HOME/.local"
cargo install --locked --path "$root/plugins/onboard" --root "$HOME/.local"
cargo install --locked --path "$root/plugins/lanista" --root "$HOME/.local"
cargo install --locked --path "$root/plugins/capacity" --root "$HOME/.local"
cargo install --locked --path "$root/plugins/janitor" --root "$HOME/.local"
cargo install --locked --path "$root/plugins/review" --root "$HOME/.local"
cargo install --locked --path "$root/plugins/gate" --root "$HOME/.local"
cargo install --locked --path "$root/plugins/bd" --root "$HOME/.local"
cargo install --locked --path "$root/plugins/secrets" --root "$HOME/.local"
# direnv: `use vega <tool>` in an .envrc loads that tool's secrets (docs/design/secrets.md).
direnv_lib="${XDG_CONFIG_HOME:-$HOME/.config}/direnv/lib"
mkdir -p "$direnv_lib"
install -m 644 "$root/plugins/secrets/direnv/vega.sh" "$direnv_lib/vega.sh"
# Shims must not shadow tools for humans (P3); only agent sessions put this dir on PATH.
cargo install --locked --path "$root/shims/herdr" --root "$HOME/.local/share/vega/shims"
cargo install --locked --path "$root/shims/gh" --root "$HOME/.local/share/vega/shims"
cargo install --locked --path "$root/shims/tea" --root "$HOME/.local/share/vega/shims"
# git push gate (docs/design/gate.md); every other git command is a plain exec.
cargo install --locked --path "$root/shims/git" --root "$HOME/.local/share/vega/shims"
# Per-worktree BEADS_ACTOR (docs/design/beads-sync.md, Claim identity).
cargo install --locked --path "$root/shims/bd" --root "$HOME/.local/share/vega/shims"

# Agent hooks (docs/design/config.md, units claude.hooks, codex.hooks, pi.prefix):
# the ask-parent PreToolUse hook (a spawned agent asks its parent, not the user
# in its pane) for Claude and codex, and the P3 shim activation: Claude sources
# activate.sh through a SessionStart hook and CLAUDE_ENV_FILE (it overwrites PATH
# from its shell snapshot), pi through its shellCommandPrefix; codex runs `zsh -c`,
# so zsh startup files cover it (chezmoi-managed, see P3). Key-merged: other keys
# and hooks stay, the old file goes to <file>.vega-bak. After a hook change every
# new codex session stops at "Hooks need review" until the user trusts it once.
# Only the hooks: the personal units (instructions, styles, settings keys) are
# applied by `vega deploy` or an explicit `vega config apply`, never here.
install -m 644 "$root/shims/activate.sh" "$HOME/.local/share/vega/shims/activate.sh"
"$HOME/.local/bin/vega-config" apply hooks ||
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
