# direnv helper from vega (plugins/secrets/direnv/vega.sh, installed to
# ~/.config/direnv/lib/vega.sh). In an .envrc: `use vega <tool>` exports the
# vega secrets for <tool>, and only those. The values travel through direnv's
# own dump file, never through stdout or argv.
use_vega() {
	local tool=${1:-}
	if [[ -z $tool ]]; then
		log_error "use vega: name the tool, e.g. use vega spellkave"
		return 1
	fi
	if ! has vega; then
		log_error "use vega $tool: vega is not installed; secrets not loaded"
		return 0
	fi
	direnv_load vega secrets run --for "$tool" -- "$direnv" dump
}
