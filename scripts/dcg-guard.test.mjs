import assert from 'node:assert/strict';
import { test } from 'node:test';
import { mkdtempSync, writeFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

// Load the actual TypeScript extension through Node's type stripping; Pi also
// erases its ExtensionAPI type import. No agent or destructive shell is invoked.
async function handler(binary = process.env.DCG_BIN) {
  if (binary) process.env.DCG_BIN = binary;
  else delete process.env.DCG_BIN;
  const { default: extension } = await import(`../config/pi/extensions/dcg-guard.ts?bin=${encodeURIComponent(binary ?? 'real')}`);
  let call;
  extension({ on(event, fn) { assert.equal(event, 'tool_call'); call = fn; } });
  return (command, toolName = 'bash') => call({ toolName, input: { command } });
}

test('Pi guard blocks real destructive commands and allows read-only commands', async (t) => {
  const { spawnSync } = await import('node:child_process');
  process.env.DCG_NO_SELF_HEAL = '1';
  const probe = spawnSync(process.env.DCG_BIN ?? 'dcg', ['--version'], { encoding: 'utf8' });
  if (probe.error || !/(?:^|\s)0\.(?:1[6-9]|[2-9][0-9])\./.test(probe.stdout)) return t.skip('dcg >=0.16.0 not installed');
  const call = await handler();
  assert.equal((await call('git reset --hard')).block, true);
  assert.equal(await call('ls'), undefined);
  assert.equal(await call('git reset --hard', 'read'), undefined);
  assert.equal(await call('  '), undefined);
});

test('Pi guard fails closed on spawn failure, errors, signals and unreadable denials', async () => {
  const dir = mkdtempSync(join(tmpdir(), 'vega-dcg-test-'));
  try {
    assert.equal((await (await handler(join(dir, 'missing')))('ls')).block, true);
    for (const [name, body] of [
      ['config-error', 'exit 3'], ['indeterminate', 'exit 2'],
      ['signal', 'kill -TERM $$'], ['deny-without-json', 'echo invalid; exit 1'],
      ['deny', `echo '{"reason":"guard denied","rule_id":"reset-hard"}'; exit 1`],
    ]) {
      const bin = join(dir, name);
      writeFileSync(bin, `#!/bin/sh\n${body}\n`, { mode: 0o755 });
      const verdict = await (await handler(bin))('ls');
      assert.equal(verdict.block, true, name);
      if (name === 'deny') assert.match(verdict.reason, /guard denied.*reset-hard/);
    }
  } finally {
    delete process.env.DCG_BIN;
    rmSync(dir, { recursive: true, force: true });
  }
});
