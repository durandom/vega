// Run against a built vega-browser with Node 22+ on a desktop with CfT installed.
import assert from 'node:assert/strict';
import { execFileSync, spawnSync } from 'node:child_process';
import { mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync } from 'node:fs';
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
const events = [];
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

try {
  for (const project of ['alpha', 'beta']) {
    const cwd = join(scratch, project);
    mkdirSync(cwd);
    execFileSync('git', ['init', '-q'], { cwd });
    execFileSync('git', ['remote', 'add', 'origin', `https://example.invalid/${project}.git`], { cwd });
  }

  const missing = cli('alpha', ['--session', 'first', 'tab', 'new']);
  assert.notEqual(missing.status, 0);
  assert.match(missing.stdout + missing.stderr, /Allow in Incognito/);
  assert.deepEqual(registry().projects, {});
  ok('alpha', ['--session', 'unmarked', 'tab', 'new'], { VEGA_BROWSER_MARK: '0' });
  assert.equal(registry().sessions.unmarked.tabs[1].chrome_tab_id, undefined);
  ok('alpha', ['--session', 'unmarked', 'close']);
  console.log('Missing incognito grant and opt-out passed');

  const { browser } = registry();
  assert.equal(browser.profile, profile);
  socket = new WebSocket(`ws://127.0.0.1:${browser.port}${browser.path}`);
  socket.addEventListener('message', ({ data }) => {
    const message = JSON.parse(data);
    if (message.id) {
      requests.get(message.id)?.(message);
      requests.delete(message.id);
    } else events.push(message);
  });
  await until(() => socket.readyState === WebSocket.OPEN, 'fixture CDP connection');
  console.log((await call('Browser.getVersion')).product);

  // Modify the generated profile only. The production extension never grants itself access.
  await evaluate({ url: 'chrome://extensions/' }, `chrome.developerPrivate && (async () => {
    await chrome.developerPrivate.updateProfileConfiguration({inDeveloperMode:true});
    await chrome.developerPrivate.updateExtensionConfiguration({extensionId:'${extension}', incognitoAccess:true});
    return true;
  })()`);

  ok('alpha', ['--session', 'first', 'tab', 'new']);
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
  await page(params('first'), async sessionId => {
    await call('ServiceWorker.enable', {}, sessionId);
    const versions = () => events.filter(event => event.sessionId === sessionId &&
      event.method === 'ServiceWorker.workerVersionUpdated').flatMap(event => event.params.versions);
    const worker = await until(() => versions().find(version =>
      version.scriptURL === `chrome-extension://${extension}/worker.js` && version.runningStatus === 'running'),
    'running project worker');
    await call('ServiceWorker.stopWorker', { versionId: worker.versionId }, sessionId);
    await until(() => versions().some(version => version.versionId === worker.versionId &&
      version.runningStatus === 'stopped'), 'worker stop');
  });
  ok('alpha', ['--session', 'first', 'tab', 'new']);
  const after = await inspect('first');
  const groupId = before.groups.find(group => group.title === 'alpha · first').id;
  assert.equal(after.tabs.filter(tab => tab.groupId === groupId).length, 2);
  assert.deepEqual(after.groups, before.groups);
  console.log('Two contexts, three titled groups, target mapping, and worker restart passed');

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
  }
} finally {
  if (socket?.readyState === WebSocket.OPEN) {
    try { await call('Browser.close'); } catch {}
    socket.close();
  }
  let browser;
  try { ({ browser } = registry()); } catch {}
  if (browser?.profile === profile) {
    const running = () => spawnSync('ps', ['-ww', '-p', String(browser.pid), '-o', 'command='], {
      encoding: 'utf8',
    }).stdout.includes(`--user-data-dir=${profile} `);
    if (running()) process.kill(browser.pid, 'SIGTERM');
    await until(() => !running(), 'throwaway Chrome shutdown', 240);
  }
  rmSync(scratch, { recursive: true, force: true });
}
