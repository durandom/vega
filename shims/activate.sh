# Agent-only shim activation (P3 in docs/design/agent-tooling-principles.md).
# Sourced, not run: by zsh startup files (claude and codex tool shells are
# `zsh -c`) and by pi's shellCommandPrefix (pi's tool shell is `bash -c`).
# POSIX sh, so bash and zsh can both source it.
#
# Puts the shim dir first on PATH only when an agent env marker is set and the
# shell is not interactive: agents' tool shells never are, and a human shell
# that inherited a marker (e.g. from a terminal server an agent started) stays
# clean. Without a marker nothing changes: humans, scripts and CI see the
# original tools.
case $- in
*i*) ;;
*)
	case "${CLAUDECODE:-}${CODEX_THREAD_ID:-}${PI_CODING_AGENT:-}${AI_AGENT:-}" in
	'') ;;
	*)
		# First, not just present: macOS path_helper (login shells) moves
		# system dirs such as /opt/homebrew/bin in front.
		case "$PATH" in
		"$HOME/.local/share/vega/shims/bin":*) ;;
		*) export PATH="$HOME/.local/share/vega/shims/bin:$PATH" ;;
		esac
		;;
	esac
	;;
esac
