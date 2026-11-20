// Run with Node 22+ and an explicit Chrome for Testing executable.
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { once } from 'node:events';
import { mkdtemp, readFile, rm } from 'node:fs/promises';
import { createServer } from 'node:http';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { setTimeout as delay } from 'node:timers/promises';

const exe = process.argv[2];
if (!exe) throw Error('Pass the path to a Chrome for Testing executable');
const profile = await mkdtemp(join(tmpdir(), 'vega-share-origins-'));
const servers = [];
let chrome, exited, socket;
async function serve(handler) {
  const server = createServer(handler);
  servers.push(server);
  server.listen(0, '127.0.0.1');
  await once(server, 'listening');
  return 'http://127.0.0.1:' + server.address().port;
}
async function until(read, description) {
  for (let i = 0; i < 200; i++) {
    const value = await read();
    if (value) return value;
    await delay(25);
  }
  throw Error('Timed out waiting for ' + description);
}
try {
  let deniedHits = 0;
  const other = await serve((_, res) => {
    deniedHits++;
    res.end();
  });
  const app = await serve((req, res) => {
    if (req.url === '/redirect') {
      res.writeHead(302, { location: other + '/denied' }).end();
    } else {
      const headers = { 'Content-Type': 'text/html' };
      if (req.url === '/protected') {
        headers['Content-Security-Policy'] = "connect-src 'self'";
      }
      res.writeHead(200, headers).end(
        '<title>Ready</title><button onclick="document.title=\'Clicked\'">Run</button>',
      );
    }
  });
  chrome = spawn(exe, [
    '--user-data-dir=' + profile,
    '--remote-debugging-port=0',
    '--remote-debugging-address=127.0.0.1',
    '--headless=new',
    '--no-first-run',
    '--disable-background-networking',
    '--use-mock-keychain',
    'about:blank',
  ], { stdio: 'ignore' });
  exited = once(chrome, 'exit');
  const [port, path] = await until(async () => {
    try {
      const lines = (await readFile(join(profile, 'DevToolsActivePort'), 'utf8')).trim().split('\n');
      if (lines[1]?.startsWith('/devtools/browser/')) return lines;
    } catch (e) {
      if (e.code !== 'ENOENT') throw e;
    }
  }, 'disposable browser');
  const debuggerOrigin = 'http://127.0.0.1:' + port;
  socket = new WebSocket(debuggerOrigin.replace('http:', 'ws:') + path);
  await once(socket, 'open');
  let next = 0;
  const pending = new Map(), denied = new Set(), inspected = new Set();
  let interceptionError, forbiddenHandshake = false;
  function call(method, params = {}, sessionId) {
    const id = ++next;
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        pending.delete(id);
        reject(Error('Timeout: ' + method));
      }, 5000);
      pending.set(id, (msg) => {
        clearTimeout(timer);
        if (msg.error) reject(Error(method + ': ' + msg.error.message));
        else resolve(msg.result);
      });
      socket.send(JSON.stringify({ id, method, params, sessionId }));
    });
  }
  socket.onmessage = ({ data }) => {
    const msg = JSON.parse(data);
    if (msg.method === 'Network.webSocketFrameError' && /403/.test(msg.params.errorMessage)) {
      forbiddenHandshake = true;
    }
    if (pending.has(msg.id)) {
      pending.get(msg.id)(msg);
      pending.delete(msg.id);
    } else if (msg.method === 'Fetch.requestPaused') {
      const { request, requestId } = msg.params;
      inspected.add(request.url);
      const allowed = new URL(request.url).origin === app;
      if (!allowed) denied.add(request.url);
      const decision = allowed
        ? call('Fetch.continueRequest', { requestId }, msg.sessionId)
        : call('Fetch.failRequest', { requestId, errorReason: 'BlockedByClient' }, msg.sessionId);
      decision.catch((e) => {
        interceptionError = e;
      });
    }
  };
  const { browserContextId } = await call('Target.createBrowserContext');
  const { targetId } = await call('Target.createTarget', { browserContextId, url: 'about:blank' });
  const { sessionId } = await call('Target.attachToTarget', { targetId, flatten: true });
  const page = (method, params) => call(method, params, sessionId);
  await page('Network.enable');
  await page('Network.setBypassServiceWorker', { bypass: true });
  await page('Fetch.enable', { patterns: [{ urlPattern: '*', requestStage: 'Request' }] });
  await page('Page.navigate', { url: app });
  const evaluate = async (expression) =>
    (await page('Runtime.evaluate', { expression, returnByValue: true, awaitPromise: true })).result
      .value;
  await until(() => evaluate("document.querySelector('button') !== null"), 'app button');
  await evaluate("document.querySelector('button').click()");
  assert.equal(await evaluate('document.title'), 'Clicked');
  const tree = await page('Accessibility.getFullAXTree');
  assert.ok(tree.nodes.some((node) => node.role?.value === 'button' && node.name?.value === 'Run'));
  for (
    const url of [
      other + '/resource',
      app + '/redirect',
      debuggerOrigin + '/json',
      debuggerOrigin + '/devtools/browser/fixture',
    ]
  ) {
    await evaluate('fetch(' + JSON.stringify(url) + ', {mode:"no-cors"}).then(()=>true,()=>false)');
    const expected = url === app + '/redirect' ? other + '/denied' : url;
    assert.ok(denied.has(expected), 'Missing interception for ' + expected);
  }
  assert.equal(deniedHits, 0);
  // Observe coverage on the approved server. Reject the upgrade there so
  // the probe never establishes a persistent WebSocket.
  let sawUpgrade = false;
  servers[1].once('upgrade', (_request, peer) => {
    sawUpgrade = true;
    peer.end('HTTP/1.1 403 Forbidden\r\nConnection: close\r\n\r\n');
  });
  const upgradeUrl = app.replace('http:', 'ws:') + '/fixture-socket';
  await evaluate(
    'new Promise(resolve => { const ws = new WebSocket(' +
      JSON.stringify(upgradeUrl) + '); ws.onerror = () => resolve(true); })',
  );
  console.log(
    'Approved-origin WebSocket probe: server received upgrade=%s, Fetch paused it=%s',
    sawUpgrade,
    inspected.has(upgradeUrl) || inspected.has(app + '/fixture-socket'),
  );
  // Test the disposable debugger's own Origin admission with no document CSP.
  // This client sends no debugger messages, including on unexpected success.
  const debugWs = debuggerOrigin.replace('http:', 'ws:') + path;
  const rejected = await evaluate(`new Promise(resolve => {
    const ws = new WebSocket(${JSON.stringify(debugWs)});
    ws.onopen = () => { ws.close(); resolve(false); };
    ws.onerror = () => resolve(true);
  })`);
  assert.equal(rejected, true);
  await until(() => forbiddenHandshake, 'debugger HTTP 403');
  console.log('Disposable debugger rejected the unprotected page Origin with HTTP 403.');
  // A response policy provides a pre-handshake decision in the renderer.
  await page('Page.navigate', { url: app + '/protected' });
  await until(
    () => evaluate("location.pathname === '/protected' && document.readyState === 'complete'"),
    'protected fixture',
  );
  let deniedUpgrades = 0;
  servers[0].on('upgrade', (_request, peer) => {
    deniedUpgrades++;
    peer.destroy();
  });
  for (const destination of [other.replace('http:', 'ws:') + '/fixture', debugWs]) {
    const decision = await evaluate(`new Promise(resolve => {
      document.addEventListener('securitypolicyviolation', event => resolve(event.effectiveDirective),
        { once: true });
      new WebSocket(${JSON.stringify(destination)}).onerror = () => {};
    })`);
    assert.equal(decision, 'connect-src');
  }
  assert.equal(deniedUpgrades, 0);
  console.log('Response CSP blocked both denied WebSockets before the upgrade.');
  if (interceptionError) throw interceptionError;
  console.log('Disposable CfT passed click, accessibility snapshot and four HTTP origin checks.');
} finally {
  socket?.close();
  if (chrome?.pid && chrome.exitCode === null && chrome.signalCode === null) {
    chrome.kill('SIGTERM');
    const kill = setTimeout(() => chrome.kill('SIGKILL'), 3000);
    try {
      await exited;
    } finally {
      clearTimeout(kill);
    }
  }
  for (const server of servers) {
    server.closeAllConnections();
    await new Promise((resolve) => server.close(resolve));
  }
  await rm(profile, { recursive: true, force: true });
}
