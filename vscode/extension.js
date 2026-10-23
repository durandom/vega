// @ts-check
// vega's VS Code side: actions the `code` CLI cannot do, reachable from
// outside via vscode://vega.vega-vscode/<action>/<absolute path>.
//
// VS Code delivers the URI to the last focused window, so callers (vega
// open) first run `code <repo>` to bring the repo's window to the front.

const vscode = require('vscode');
const { parseRequest } = require('./routes');

/** @param {vscode.ExtensionContext} context */
function activate(context) {
  context.subscriptions.push(vscode.window.registerUriHandler({ handleUri }));
}

/** @param {vscode.Uri} uri */
async function handleUri(uri) {
  const req = parseRequest(uri.path);
  if ('error' in req) {
    void vscode.window.showErrorMessage(`vega: ${req.error}`);
    return;
  }
  const target = vscode.Uri.file(req.fsPath);
  try {
    if (req.action === 'reveal') {
      // revealInExplorer is a no-op while the Explorer view is hidden
      // (microsoft/vscode#197268), so show it first.
      await vscode.commands.executeCommand('workbench.view.explorer');
      await vscode.commands.executeCommand('revealInExplorer', target);
    } else if (req.action === 'diff') {
      await vscode.commands.executeCommand('git.openChange', target);
    } else {
      // The repo's window is focused by the caller; its Source Control view
      // lists every change with per-file diffs.
      await vscode.commands.executeCommand('workbench.view.scm');
    }
  } catch (e) {
    void vscode.window.showErrorMessage(`vega: ${req.action} ${req.fsPath} failed: ${e}`);
  }
}

function deactivate() {}

module.exports = { activate, deactivate };
