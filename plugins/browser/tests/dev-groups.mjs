// Run against a built vega-browser with Node 22+ on a desktop with CfT installed.
import assert from 'node:assert/strict';
import { execFileSync, spawnSync } from 'node:child_process';
import { mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, writeFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { setTimeout as delay } from 'node:timers/promises';

const binary = resolve(process.argv[2] ?? 'target/debug/vega-browser');
// A short socket directory is required for the pinned daemon on macOS.
const scratch = mkdtempSync(join(realpathSync('/tmp'), 'vg-'));
const profile = join(scratch, 'data/vega/browser/dev-cft');
const registry = () => JSON.parse(readFileSync(join(profile, 'vega-dev.json')));
const env = Object.fromEntries(Object.entries(process.env).filter(([key]) =>
  !key.startsWith('AGENT_BROWSER_') && key !== 'VEGA_BROWSER_MARK'));
Object.assign(env, {
  XDG_DATA_HOME: join(scratch, 'data'),
  XDG_STATE_HOME: join(scratch, 'state'),
  XDG_CONFIG_HOME: join(scratch, 'config'),
});
const extension = 'ddhpkjdjaccmkefgalpmnbnpibiimapm';
const control = `chrome-extension://${extension}/control.html`;

function cli(project, args, overrides) {
  return spawnSync(binary, ['dev', ...args], {
    cwd: join(scratch, project), env: { ...env, ...overrides }, encoding: 'utf8', timeout: 30000,
  });
}

function ok(project, args, overrides) {
  const result = cli(project, args, overrides);
  if (result.error) throw result.error;
  assert.equal(result.status, 0, result.stdout + result.stderr);
  return result.stdout;
}

let socket;
let nextId = 0;
const requests = new Map();
function call(method, params = {}, sessionId) {
  const id = ++nextId;
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => {
      requests.delete(id);
      reject(Error(`Timed out: ${method}`));
    }, 5000);
    requests.set(id, ({ error, result }) => {
      clearTimeout(timer);
      if (error) reject(Error(`${method}: ${error.message}`));
      else resolve(result);
    });
    socket.send(JSON.stringify({ id, method, params, sessionId }));
  });
}

async function until(read, what, attempts = 40) {
  for (let attempt = 0; attempt < attempts; attempt++) {
    const result = await read();
    if (result) return result;
    await delay(50);
  }
  throw Error(`Timed out: ${what}`);
}

async function page(params, action) {
  const { targetId } = await call('Target.createTarget', params);
  try {
    const { sessionId } = await call('Target.attachToTarget', { targetId, flatten: true });
    return await action(sessionId);
  } finally {
    try {
      await call('Target.closeTarget', { targetId });
    } catch (error) {
      throw Error(`Could not close fixture page ${params.url}`, { cause: error });
    }
  }
}

const evaluate = (params, expression) => page(params, sessionId => until(async () => {
  const { result } = await call('Runtime.evaluate', {
    expression, awaitPromise: true, returnByValue: true,
  }, sessionId);
  return result.value;
}, params.url));

async function connect(browser) {
  assert.equal(browser.profile, profile);
  socket = new WebSocket(`ws://127.0.0.1:${browser.port}${browser.path}`);
  socket.addEventListener('message', ({ data }) => {
    const message = JSON.parse(data);
    requests.get(message.id)?.(message);
    requests.delete(message.id);
  });
  await until(() => socket.readyState === WebSocket.OPEN, 'fixture CDP connection');
}

async function stop(browser) {
  assert.equal(browser.profile, profile);
  const running = () => spawnSync('ps', ['-ww', '-p', String(browser.pid), '-o', 'command='], {
    encoding: 'utf8',
  }).stdout.includes(`--user-data-dir=${profile} `);
  if (socket?.readyState === WebSocket.OPEN) {
    try { await call('Browser.close'); } catch {}
    socket.close();
  } else if (running()) {
    process.kill(browser.pid, 'SIGTERM');
  }
  await until(() => !running(), 'throwaway Chrome shutdown', 600);
}

try {
  for (const project of ['alpha', 'beta']) {
    const cwd = join(scratch, project);
    mkdirSync(cwd);
    execFileSync('git', ['init', '-q'], { cwd });
    execFileSync('git', ['remote', 'add', 'origin', `https://example.invalid/${project}.git`], { cwd });
  }

  ok('alpha', ['--session', 'unmarked', 'tab', 'new'], { VEGA_BROWSER_MARK: '0' });
  assert.equal(registry().sessions.unmarked.tabs[1].chrome_tab_id, undefined);
  const oldContext = Object.values(registry().projects)[0].context;

  const { browser } = registry();
  await connect(browser);
  console.log((await call('Browser.getVersion')).product);

  const access = () => evaluate({ url: control, hidden: true, background: true },
    'chrome.extension && chrome.extension.isAllowedIncognitoAccess().then(allowed => ({allowed}))');
  assert.deepEqual(await access(), { allowed: false });
  ok('alpha', ['--session', 'first', 'tab', 'new']);
  assert.deepEqual(await access(), { allowed: true });
  assert.equal(Object.values(registry().projects)[0].context, oldContext);
  ok('alpha', ['--session', 'unmarked', 'close']);
  ok('alpha', ['--session', 'second', 'tab', 'new']);
  ok('beta', ['--session', 'third', 'tab', 'new']);
  const data = registry();
  const project = name => data.projects[data.sessions[name].project];
  const params = name => ({ url: control, browserContextId: project(name).context });
  const inspect = name => evaluate(params(name), `chrome.tabGroups &&
    Promise.all([chrome.tabGroups.query({}), chrome.tabs.query({})]).then(([groups,tabs]) => ({
      groups, tabs: tabs.map(({id,windowId,groupId}) => ({id,windowId,groupId}))
    }))`);
  assert.notEqual(project('first').context, project('third').context);
  assert.notEqual(project('first').window, project('third').window);
  assert.equal(project('first').window, project('second').window);
  const { targetInfos } = await call('Target.getTargets');
  assert.equal(targetInfos.filter(info => info.url === control).length, 0);
  assert.equal(targetInfos.some(info => info.url.startsWith('chrome://extensions')), false);
  assert.equal(targetInfos.some(info => info.type === 'service_worker' &&
    info.url.startsWith(`chrome-extension://${extension}/`)), false);
  for (const [name, label, colour] of [
    ['first', 'alpha', 'pink'], ['second', 'alpha', 'pink'], ['third', 'beta', 'purple'],
  ]) {
    const tab = data.sessions[name].tabs[1];
    const info = targetInfos.find(info => info.targetId === tab.target);
    assert.equal(info.browserContextId, project(name).context);
    assert.equal(info.url, 'about:blank');
    const { groups, tabs } = await inspect(name);
    const mapped = tabs.find(candidate => candidate.id === tab.chrome_tab_id);
    assert.equal(mapped.windowId, project(name).window);
    const group = groups.find(group => group.id === mapped.groupId);
    assert.equal(group.title, `${label} · ${name}`);
    assert.equal(group.color, colour);
    assert.equal((await call('Browser.getWindowForTarget', { targetId: tab.target })).windowId, mapped.windowId);
  }

  const before = await inspect('first');
  const script = join(profile, 'vega-extension/control.js');
  const original = readFileSync(script, 'utf8');
  try {
    const stalled = original.replace('await groupTabs(', 'await new Promise(() => {}); await groupTabs(');
    assert.notEqual(stalled, original);
    writeFileSync(script, stalled);
    const started = performance.now();
    const failure = cli('alpha', ['--session', 'first', 'tab', 'new', '--json']);
    assert.equal(failure.error, undefined);
    assert.notEqual(failure.status, 0);
    assert.match(failure.stdout + failure.stderr, /dev_tab_allocation_timeout/);
    assert.ok(performance.now() - started < 7000, 'allocation and cleanup exceeded 7 seconds');
    const { targetInfos: remaining } = await call('Target.getTargets');
    assert.deepEqual(remaining.filter(info => info.url.startsWith('about:blank#vega-') ||
      info.url === control), []);
  } finally {
    writeFileSync(script, original);
  }
  ok('alpha', ['--session', 'first', 'tab', 'new']);
  const after = await inspect('first');
  const groupId = before.groups.find(group => group.title === 'alpha · first').id;
  assert.equal(after.tabs.filter(tab => tab.groupId === groupId).length, 2);
  assert.deepEqual(after.groups, before.groups);
  ok('alpha', ['--session', 'first', 'open', 'data:text/html,<title>Fixture</title>']);
  const denied = cli('alpha', ['--session', 'first', 'open', 'chrome://extensions/']);
  assert.notEqual(denied.status, 0);
  assert.match(ok('alpha', ['--session', 'first', 'get', 'title']), /Fixture/);
  console.log('Automatic grant in a pre-existing context, grouping without a worker, timeout cleanup, and scoped Chrome settings denial passed');

  if (process.argv.includes('--share')) {
    const { verifyShare } = await import('./dev-share.mjs');
    await verifyShare({ scratch, binary, env, browser, foreign: data.sessions.third.tabs[1].target,
      adapterPort: data.sessions.first.tabs[1].port,
      stopBrowser: () => call('Browser.close') });
  } else {
    ok('alpha', ['--session', 'first', 'close']);
    ok('alpha', ['--session', 'second', 'close']);
    ok('beta', ['--session', 'third', 'close']);
    assert.deepEqual(registry().projects, {});
    await stop(browser);
    ok('alpha', ['--session', 'restarted', 'tab', 'new'], { VEGA_BROWSER_MARK: '0' });
    const restarted = registry().browser;
    assert.notEqual(restarted.path, browser.path);
    await connect(restarted);
    assert.deepEqual(await access(), { allowed: true });
    ok('alpha', ['--session', 'first', 'tab', 'new']);
    ok('alpha', ['--session', 'first', 'close']);
    ok('alpha', ['--session', 'restarted', 'close']);
    console.log('Grant persisted across browser restart');
  }
} catch (error) {
  console.error(error);
  throw error;
} finally {
  let browser;
  try { ({ browser } = registry()); } catch {}
  if (browser?.profile === profile) await stop(browser);
  rmSync(scratch, { recursive: true, force: true });
}
