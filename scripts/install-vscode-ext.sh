#!/bin/sh
# Installs the vega VS Code extension (vscode/dist/vega-vscode.vsix, built by
# `npm run package`). `code` is missing on hosts without the VS Code CLI (e.g.
# WSL without the Windows PATH): skip with a hint and exit 0, don't fail.
set -eu

vsix=vscode/dist/vega-vscode.vsix
if command -v code >/dev/null; then
	code --install-extension "$vsix" --force
else
	echo "skip: code not on PATH; install $vsix by hand"
fi
