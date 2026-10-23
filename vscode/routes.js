// @ts-check
// Routing for vscode://vega.vega-vscode/<action>/<absolute path>.
//
// Pure (no `vscode` import) so `node --test` covers it. The action set is
// fixed on purpose: unlike generic "run any command" URI extensions, other
// apps on this machine can only reveal, diff or show Source Control.

const ACTIONS = ['reveal', 'diff', 'scm'];

/**
 * @param {string} uriPath `vscode.Uri.path`, already percent-decoded once
 * @returns {{action: string, fsPath: string} | {error: string}}
 */
function parseRequest(uriPath) {
  // The file path rides in the URI path rather than a query parameter, so
  // it is decoded exactly once and `+` or `%` in file names survive.
  const m = /^\/([a-z]+)(\/.*)$/.exec(uriPath);
  if (!m) {
    return { error: `expected /<action>/<absolute path>, got ${uriPath}` };
  }
  const [, action, fsPath] = m;
  if (!ACTIONS.includes(action)) {
    return { error: `unknown action "${action}" (known: ${ACTIONS.join(', ')})` };
  }
  if (fsPath.split('/').includes('..')) {
    return { error: `path must not contain "..": ${fsPath}` };
  }
  return { action, fsPath };
}

module.exports = { parseRequest, ACTIONS };
