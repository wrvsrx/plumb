// Self-contained browser regression. Run under an explicit hard memory limit.
// Requires a built target/debug/plumb and chromium (or PLUMB_E2E_CHROMIUM).
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { mkdtemp, mkdir, writeFile, rm } from 'node:fs/promises';
import { createServer } from 'node:net';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';

const port = Number(process.env.PLUMB_E2E_PORT || 38939);
const cdpPort = Number(process.env.PLUMB_CDP_PORT || 9239);
const base = `http://127.0.0.1:${port}`;
const delay = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
async function available(port) {
  const server = createServer();
  await new Promise((resolve, reject) => server.once('error', reject).listen(port, '127.0.0.1', resolve));
  await new Promise((resolve) => server.close(resolve));
}
await available(port);
await available(cdpPort);
const temp = await mkdtemp(join(tmpdir(), 'plumb-duration-e2e-'));
const root = join(temp, 'notes');
await mkdir(root);
const children = [];
let socket;
let logs = '';
function start(binary, args, env = process.env) {
  const child = spawn(binary, args, { env, stdio: ['ignore', 'pipe', 'pipe'] });
  child.on('error', (error) => { logs += String(error); });
  for (const stream of [child.stdout, child.stderr]) stream.on('data', (data) => { logs = (logs + data).slice(-8000); });
  children.push(child);
  return child;
}
async function until(check, message) {
  for (let n = 0; n < 160; n += 1) {
    if (await check()) return;
    if (children.some((child) => child.exitCode !== null)) throw new Error(`test process exited: ${logs}`);
    await delay(50);
  }
  throw new Error(`${message}\n${logs}`);
}
try {
  await writeFile(join(root, 'tasks.plumb'), '`+ task\n`= title Project\n`- Alpha\n `+ task\n `@ a\n `= focused 2026-09-01T00:00:00Z--\n`- Zero\n `+ task\n `@ zero\n');
  const event = '`- 2026-10-01T10:00:00Z--11:00 `->{tasks.plumb#a}\n `+ event\n';
  const day = join(root, 'day.plumb');
  await writeFile(day, event);
  start(process.env.PLUMB_E2E_BINARY || fileURLToPath(new URL('../../../target/debug/plumb', import.meta.url)),
    ['site', 'serve', '--root', root, '--port', String(port)], { ...process.env, PLUMB_CACHE_DIR: join(temp, 'cache') });
  await until(async () => { try { return (await fetch(base)).ok; } catch { return false; } }, 'server not ready');
  start(process.env.PLUMB_E2E_CHROMIUM || 'chromium', [
    '--headless', '--disable-gpu', '--no-sandbox', '--disable-dev-shm-usage',
    `--user-data-dir=${join(temp, 'chromium')}`, `--remote-debugging-port=${cdpPort}`, 'about:blank',
  ]);
  let pages;
  await until(async () => {
    try { pages = await fetch(`http://127.0.0.1:${cdpPort}/json`).then((r) => r.json()); return pages.some((p) => p.type === 'page'); } catch { return false; }
  }, 'Chromium not ready');
  socket = new WebSocket(pages.find((p) => p.type === 'page').webSocketDebuggerUrl);
  await new Promise((resolve, reject) => { socket.addEventListener('open', resolve, { once: true }); socket.addEventListener('error', reject, { once: true }); });
  let id = 0;
  const pending = new Map();
  socket.addEventListener('message', ({ data }) => {
    const message = JSON.parse(data);
    if (pending.has(message.id)) { pending.get(message.id)(message); pending.delete(message.id); }
  });
  const command = (method, params = {}) => new Promise((resolve) => {
    const current = ++id;
    pending.set(current, resolve);
    socket.send(JSON.stringify({ id: current, method, params }));
  });
  async function evaluate(expression) {
    const result = await command('Runtime.evaluate', { expression, awaitPromise: true, returnByValue: true });
    assert.equal(result.result?.exceptionDetails, undefined, JSON.stringify(result));
    return result.result.result.value;
  }
  const row = `(title) => [...document.querySelectorAll('.task-list-item')].find(r => r.querySelector('strong')?.textContent === title)`;
  for (const width of [1280, 390]) {
    await writeFile(day, event);
    await command('Emulation.setDeviceMetricsOverride', { width, height: 900, deviceScaleFactor: 1, mobile: width < 600 });
    await command('Page.navigate', { url: `${base}/tasks` });
    await until(() => evaluate(`(${row})('Alpha')?.querySelector('.task-time-spent')?.textContent === 'Time spent: 1h'`), 'initial duration missing');
    await evaluate(`(() => { document.querySelectorAll('.task-unfocused-group[aria-expanded="false"]').forEach(b => b.click()); })()`);
    await until(() => evaluate(`Boolean((${row})('Zero'))`), 'zero task hidden');
    assert.equal(await evaluate(`(${row})('Zero').querySelector('.task-time-spent').textContent`), 'Time spent: 0s');
    assert.equal(await evaluate(`(${row})('Project').querySelector('.task-time-spent').textContent`), 'Time spent: 0s');
    assert.match(await evaluate(`(${row})('Alpha').querySelector('.task-focus-age').textContent`), /focused/);
    assert.equal(await evaluate('document.documentElement.scrollWidth > document.documentElement.clientWidth'), false);
    await evaluate(`(${row})('Alpha').querySelector('.task-row').click()`);
    const detail = `(() => { const term = [...document.querySelectorAll('#task-panel dt')].find(t => t.textContent === 'Time spent'); return term?.nextElementSibling?.textContent; })()`;
    await until(() => evaluate(`${detail} === '1h'`), 'detail duration missing');
    for (const [source, expected] of [
      [event.replace('--11:00', '--12:30'), '2h 30m'],
      [event.replace('--11:00', '--'), '0s'],
      [event.replace('#a', '#missing'), 'Unavailable (incomplete)'],
      [event, '1h'],
    ]) {
      await writeFile(day, source);
      await until(() => evaluate(`${detail} === ${JSON.stringify(expected)}`), 'detail failed to refresh');
      assert.equal(await evaluate(`(${row})('Alpha').querySelector('.task-time-spent').textContent`), `Time spent: ${expected}`);
    }
    assert.equal(await evaluate('document.documentElement.scrollWidth > document.documentElement.clientWidth'), false);
  }
  console.log('Task duration browser checks passed: desktop/mobile, zero, focus separation, watcher refresh, ongoing and incomplete accounting.');
} finally {
  socket?.close();
  for (const child of children.reverse()) {
    if (child.exitCode !== null) continue;
    child.kill('SIGTERM');
    for (let n = 0; n < 40 && child.exitCode === null && child.signalCode === null; n += 1) await delay(50);
    if (child.exitCode === null && child.signalCode === null) {
      child.kill('SIGKILL');
      await new Promise((resolve) => child.once('exit', resolve));
    }
  }
  await rm(temp, { recursive: true, force: true, maxRetries: 3, retryDelay: 100 });
}
