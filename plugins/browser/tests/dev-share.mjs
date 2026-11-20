// Invoked by dev-groups.mjs --share after the temporary project's group is ready.
import assert from 'node:assert/strict';
import { execFile, execFileSync, spawn } from 'node:child_process';
import { once } from 'node:events';
import { mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { createServer } from 'node:http';
import { createConnection, createServer as tcpServer } from 'node:net';
import { dirname, join } from 'node:path';
import { setTimeout as delay } from 'node:timers/promises';
import { promisify } from 'node:util';

const run = promisify(execFile);
export async function verifyShare({ scratch, binary, env, browser, foreign, adapterPort, stopBrowser }) {
  const root = join(scratch, 'ssh');
  mkdirSync(root);
  const cli = (...args) =>
    run(binary, ['dev', 'share', ...args], {
      cwd: join(scratch, 'alpha'),
      env: { ...env, PATH: root + ':' + env.PATH },
      timeout: 40000,
    });
  let deniedHits = 0;
  const denied = createServer((_req, res) => {
    deniedHits++;
    res.end();
  });
  denied.on('upgrade', (_req, peer) => {
    deniedHits++;
    peer.destroy();
  });
  denied.listen(0, '127.0.0.1');
  await once(denied, 'listening');
  const other = 'http://127.0.0.1:' + denied.address().port;
  const app = createServer((req, res) => {
    if (req.url === '/redirect') {
      res.writeHead(302, { location: other + '/denied' }).end();
      return;
    }
    if (req.url === '/policy') {
      res.writeHead(200, { 'Content-Type': 'text/html' });
      res.end(`<body><script>
        let violations=0; document.title='started';
        document.addEventListener('securitypolicyviolation',e=>{
          if(e.effectiveDirective==='connect-src')violations++;
        });
        (async()=>{
          await fetch('/redirect').catch(()=>{});document.title='redirect completed';
          const sockets=${
        JSON.stringify([other.replace('http:', 'ws:'), 'ws://127.0.0.1:' + browser.port])
      };
          for(const host of sockets) await new Promise(resolve=>{
            const ws=new WebSocket(host+'/devtools/browser/fixture');
            ws.onerror=resolve;ws.onopen=()=>ws.close();
          });
          setTimeout(()=>{document.title='violations '+violations;if(violations>=2)document.body.id='protected'},50);
        })();
      </script></body>`);
      return;
    }
    res.writeHead(200, { 'Content-Type': 'text/html' });
    res.end(
      '<title>Ready</title><input aria-label="Name"><button onclick="document.title=document.querySelector(\'input\').value">Run</button>',
    );
  });
  app.listen(0, '127.0.0.1');
  await once(app, 'listening');
  const origin = 'http://127.0.0.1:' + app.address().port;
  let sshd, exited;
  try {
    for (const name of ['client', 'server']) {
      execFileSync('ssh-keygen', ['-q', '-t', 'ed25519', '-N', '', '-f', join(root, name)]);
    }
    const portProbe = tcpServer();
    portProbe.listen(0, '127.0.0.1');
    await once(portProbe, 'listening');
    const port = portProbe.address().port;
    await new Promise((resolve) => portProbe.close(resolve));
    writeFileSync(
      join(root, 'daemon'),
      [
        'ListenAddress 127.0.0.1',
        'Port ' + port,
        'HostKey ' + root + '/server',
        'AuthorizedKeysFile ' + root + '/client.pub',
        'StrictModes no',
        'PidFile ' + root + '/pid',
        'PasswordAuthentication no',
        'KbdInteractiveAuthentication no',
        'UsePAM no',
      ].join('\n') + '\n',
    );
    writeFileSync(
      join(root, 'known'),
      '[127.0.0.1]:' + port + ' ' + readFileSync(join(root, 'server.pub'), 'utf8'),
    );
    writeFileSync(
      join(root, 'config'),
      [
        'Host fixture',
        'HostName 127.0.0.1',
        'Port ' + port,
        'IdentityFile ' + root + '/client',
        'IdentitiesOnly yes',
        'UserKnownHostsFile ' + root + '/known',
        'GlobalKnownHostsFile /dev/null',
      ].join('\n') + '\n',
    );
    writeFileSync(join(root, 'ssh'), '#!/bin/sh\nexec /usr/bin/ssh -F "' + root + '/config" "$@"\n', {
      mode: 0o700,
    });
    sshd = spawn('/usr/sbin/sshd', ['-D', '-e', '-f', join(root, 'daemon')], { stdio: 'ignore' });
    exited = once(sshd, 'exit');
    await delay(250);
    const start = async (ttl) => {
      const { stdout } = await cli('alpha', '--to', 'fixture', '--origin', origin, '--ttl', ttl);
      const match = /^VEGA_BROWSER_SHARE=(ws:\/\/127\.0\.0\.1:\d+\/share\/[a-f0-9]{64}) /.exec(stdout);
      assert.ok(match, 'Missing capability command');
      return match[1];
    };
    const shim = (cap, ...args) =>
      run(join(dirname(binary), 'agent-browser'), args, {
        env: { ...env, VEGA_BROWSER_SHARE: cap },
        timeout: 20000,
      }).then((r) => r.stdout.trim());
    await assert.rejects(
      cli('alpha', '--to', 'fixture', '--origin', 'http://127.0.0.1:' + adapterPort),
      (e) => e.stdout.includes('Invalid or stale share configuration'),
    );
    let cap = await start('90s');
    const pending = createConnection(Number(new URL(cap).port), '127.0.0.1');
    try {
      await once(pending, 'connect');
      const expired = once(pending, 'close');
      assert.equal(await shim(cap, 'get', 'url'), 'about:blank');
      await Promise.race([
        expired,
        delay(4000).then(() => {
          throw Error('Incomplete handshake did not expire');
        }),
      ]);
    } finally {
      pending.destroy();
    }
    assert.equal(JSON.parse((await cli('status')).stdout).shares.length, 1);
    await shim(cap, 'open', origin + '/policy');
    try {
      await shim(cap, 'wait', '#protected');
    } catch (e) {
      console.error('Policy stage', await shim(cap, 'get', 'title'), 'denied listener hits', deniedHits);
      throw e;
    }
    assert.equal(deniedHits, 0);
    assert.equal(await shim(cap, 'open', origin + '/nested/../'), origin + '/');
    await shim(cap, 'wait', 'button');
    const snap = await shim(cap, 'snapshot', '-i');
    const field = /(@e\d+) textbox/.exec(snap)?.[1], button = /(@e\d+) button/.exec(snap)?.[1];
    assert.ok(field && button, 'Missing clickable refs');
    await shim(cap, 'fill', field, 'fixture');
    await shim(cap, 'click', button);
    assert.equal(await shim(cap, 'get', 'title'), 'fixture');
    assert.equal(await shim(cap, 'get', 'url'), origin + '/');
    await shim(cap, 'press', 'Tab');
    await shim(cap, 'scroll', 'down');
    await shim(cap, 'screenshot', join(scratch, 'shot.png'));
    assert.equal(readFileSync(join(scratch, 'shot.png')).subarray(1, 4).toString(), 'PNG');
    await shim(cap, 'tab', 'new', origin);
    const list = JSON.parse(await shim(cap, 'tab', 'list'));
    assert.equal(list.length, 2);
    assert.ok(list.every((t) => t.tabId === 't1' || t.tabId === 't2'));
    await shim(cap, 'tab', 't1');
    const forbidden = 'http://127.0.0.1:' + browser.port;
    for (
      const args of [
        ['open', forbidden + '/json'],
        ['tab', 'new', forbidden + browser.path],
        ['tab', foreign],
        ['eval', '1'],
        ['state', 'save'],
        ['--cdp', '9223', 'snapshot'],
      ]
    ) {
      await assert.rejects(shim(cap, ...args), (e) => e.stdout.includes('browser share'));
    }
    const raw = new WebSocket(cap);
    await once(raw, 'open');
    raw.send(JSON.stringify({ id: 1, method: 'Target.getTargets' }));
    const [event] = await once(raw, 'message');
    assert.equal(JSON.parse(event.data).error.code, -32601);
    raw.close();
    const held = new WebSocket(cap);
    await once(held, 'open');
    const closed = once(held, 'close');
    await cli('--stop', 'alpha');
    await closed;
    await assert.rejects(shim(cap, 'snapshot'));
    cap = await start('1s');
    await delay(1500);
    await assert.rejects(shim(cap, 'snapshot'));
    assert.equal(JSON.parse((await cli('status')).stdout).shares.length, 0);
    cap = await start('60s');
    await stopBrowser();
    await delay(1400);
    await assert.rejects(shim(cap, 'snapshot'));
    console.log(
      'Share passed through a disposable loopback sshd: two scoped tabs, remote UI commands, raw method denial, stop, TTL and browser exit.',
    );
  } finally {
    try {
      await cli('--stop', 'alpha');
    } catch {}
    if (sshd?.pid && sshd.exitCode === null && sshd.signalCode === null) {
      sshd.kill();
      await exited;
    }
    for (const server of [app, denied]) {
      server.closeAllConnections();
      await new Promise((resolve) => server.close(resolve));
    }
  }
}
