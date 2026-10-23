// @ts-check
const test = require('node:test');
const assert = require('node:assert/strict');
const { parseRequest } = require('./routes');

test('routes known actions to an absolute path', () => {
  assert.deepEqual(parseRequest('/reveal/Users/me/repo/plugins/open'), {
    action: 'reveal',
    fsPath: '/Users/me/repo/plugins/open',
  });
  assert.deepEqual(parseRequest('/diff/tmp/a b+c%.rs'), {
    action: 'diff',
    fsPath: '/tmp/a b+c%.rs',
  });
  assert.deepEqual(parseRequest('/scm/src/repo'), { action: 'scm', fsPath: '/src/repo' });
});

test('rejects unknown actions, missing paths and traversal', () => {
  assert.match(parseRequest('/exec/tmp/x').error, /unknown action "exec"/);
  assert.match(parseRequest('/reveal').error, /expected/);
  assert.match(parseRequest('/reveal/tmp/../etc').error, /\.\./);
});
