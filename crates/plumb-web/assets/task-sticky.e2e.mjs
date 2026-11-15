// Self-contained browser regression. Run under an explicit hard memory limit.
// Requires a built target/debug/plumb and chromium (or PLUMB_E2E_CHROMIUM).
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { mkdtemp, mkdir, writeFile, rm } from 'node:fs/promises';
import { createServer } from 'node:net';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';

const port = Number(process.env.PLUMB_E2E_PORT || 38959);
const cdpPort = Number(process.env.PLUMB_CDP_PORT || 9259);
const base = `http://127.0.0.1:${port}`;
const delay = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
async function available(port) {
  const server = createServer();
  await new Promise((resolve, reject) => server.once('error', reject).listen(port, '127.0.0.1', resolve));
  await new Promise((resolve) => server.close(resolve));
}
await available(port);
await available(cdpPort);
const temp = await mkdtemp(join(tmpdir(), 'plumb-task-sticky-e2e-'));
const root = join(temp, 'notes');
await mkdir(root);
const children = [];
let socket;
let logs = '';
let inspect = async () => '';

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
  throw new Error(`${message}\n${logs}\n${await inspect()}`);
}
try {
  const branch = (title, prefix, count = 35) => '`- ' + title + '\n' + Array.from({length: count}, (_, i) =>
    ' `- ' + prefix + ' ' + i + '\n  `+ task\n  `@ ' + prefix.toLowerCase() + i + '\n').join('');
  const source = '`= title Notebook\n\n' + branch('First company', 'Alpha') + branch('Second company', 'Beta');
  await writeFile(join(root, 'notebook.plumb'), source);
  await writeFile(join(root, 'deep.plumb'), '`= title Deep notebook\n\n`- Country\n `- Department\n  `- Team\n   `- Parent task\n    `+ task\n    `@ parent\n' + Array.from({length: 35}, (_, i) => '    `- Deep child ' + i + '\n     `+ task\n     `@ deep' + i + '\n').join(''));
  await writeFile(join(root, 'document.plumb'), '`= title Document task\n`+ task\n' + Array.from({length: 35}, (_, i) => '`- Document child ' + i + '\n `+ task\n `@ doc' + i + '\n').join(''));
  await writeFile(join(root, 'zzz.plumb'), '`= title Last notebook\n\n' + branch('Last company', 'Last'));
  start(process.env.PLUMB_E2E_BINARY || fileURLToPath(new URL('../../../target/debug/plumb', import.meta.url)),
    ['site', 'serve', '--root', root, '--port', String(port)], { ...process.env, PLUMB_CACHE_DIR: join(temp, 'cache') });
  await until(async () => { try { return (await fetch(base)).ok; } catch { return false; } }, 'server not ready');
  start(process.env.PLUMB_E2E_CHROMIUM || 'chromium', [
    '--headless', '--disable-gpu', '--no-sandbox', '--disable-dev-shm-usage', '--disable-features=BackForwardCache',
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
    if (message.method === 'Runtime.exceptionThrown') logs += JSON.stringify(message.params.exceptionDetails);
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
  await command('Runtime.enable');
  inspect = () => evaluate(`JSON.stringify({url: location.href, text: document.querySelector('#note-panel')?.innerText, notification: document.querySelector('#notification')?.innerText, resources: performance.getEntriesByType('resource').map(r => r.name)})`);
  async function navigate(url) {
    await command('Page.navigate', { url });
    const target = new URL(url);
    await until(() => evaluate(`location.pathname === ${JSON.stringify(target.pathname)} && ${target.searchParams.has('current') ? `new URLSearchParams(location.search).get('current') === ${JSON.stringify(target.searchParams.get('current'))} &&` : ''} document.readyState === 'complete'`), 'navigation did not complete');
  }
  const row = `(title) => [...document.querySelectorAll('.task-list-item')].find(r => r.querySelector('strong')?.textContent === title)`;

  const sticky = `document.querySelector('#task-sticky-context')`;
  const pane = `document.querySelector('.task-list-pane')`;
  const labels = () => evaluate(`[...${sticky}.querySelectorAll(':scope > button')].map(b => b.textContent)`);
  const visible = () => evaluate(`${sticky}.classList.contains('is-visible')`);
  const settle = () => evaluate('new Promise(r => requestAnimationFrame(() => requestAnimationFrame(r)))');
  async function scrollTo(title, offset = 40) {
    await evaluate(`(() => { const row = (${row})(${JSON.stringify(title)}), pane = ${pane}; pane.scrollTop += row.getBoundingClientRect().top - pane.getBoundingClientRect().top - ${offset}; })()`);
    await settle();
  }
  for (const width of [1280, 390]) {
    await command('Emulation.setDeviceMetricsOverride', { width, height: 740, deviceScaleFactor: 1, mobile: width < 600 });
    await navigate(`${base}/tasks`);
    await until(() => evaluate(`document.querySelectorAll('.task-list-item').length > 100`), 'fixture tasks missing');
    await settle();
    assert.equal(await visible(), false, 'no context at list start');
    await scrollTo('Alpha 12');
    assert.deepEqual(await labels(), ['Notebook', 'First company']);
    assert.equal(await visible(), true);
    assert.equal(await evaluate(`Math.abs(${sticky}.getBoundingClientRect().top - ${pane}.getBoundingClientRect().top) < 1`), true);
    const before = await evaluate('location.href');
    await evaluate(`${sticky}.lastElementChild.click()`);
    await settle();
    assert.equal(await evaluate('location.href'), before, 'ancestor jump must not select or navigate');
    assert.equal(await evaluate(`document.activeElement.textContent`), 'First company');
    assert.equal(await evaluate(`document.activeElement.getBoundingClientRect().top >= ${pane}.getBoundingClientRect().top + 39`), true, 'jump target below sticky bar');
    await scrollTo('Beta 12');
    assert.deepEqual(await labels(), ['Notebook', 'Second company']);
    // Repeated scrolling cannot trigger query traffic.
    await evaluate(`window.stickyRequests = 0; window.originalFetch = window.fetch; window.fetch = (...args) => { window.stickyRequests++; return window.originalFetch(...args); };`);
    for (const title of ['Alpha 10', 'Beta 10', 'Alpha 20']) await scrollTo(title);
    assert.equal(await evaluate('window.stickyRequests'), 0);
    await evaluate('window.fetch = window.originalFetch');
    const contextBeforeAppend = await labels();
    await evaluate(`document.querySelector('.load-more-tasks').click()`);
    await until(() => evaluate(`Boolean((${row})('Last 12'))`), 'continuation missing');
    await settle();
    assert.deepEqual(await labels(), contextBeforeAppend, 'append changed reading context');
    await scrollTo('Last 12');
    assert.deepEqual(await labels(), ['Last notebook', 'Last company']);
    await scrollTo('Alpha 20');
    // A collapsed branch must not leave its old sticky descendants visible.
    await evaluate(`${sticky}.lastElementChild.click()`);
    await evaluate(`document.activeElement.closest('.task-document-group').querySelector('.task-disclosure').click()`);
    await settle();
    assert.equal(await evaluate(`${sticky}.textContent.includes('First company') && ${sticky}.classList.contains('is-visible')`), false);
    await navigate(`${base}/tasks`);
    await until(() => evaluate(`Boolean((${row})('Deep child 12'))`), 'deep branch missing');
    await scrollTo('Deep child 12');
    assert.deepEqual(await labels(), ['Deep notebook', 'Parent task']);
    assert.equal(await evaluate(`${sticky}.getBoundingClientRect().height`), 40);
    assert.equal(await evaluate(`${sticky}.scrollWidth > ${sticky}.clientWidth`), false);
    if (process.env.PLUMB_E2E_SCREENSHOTS) {
      await mkdir(process.env.PLUMB_E2E_SCREENSHOTS, { recursive: true });
      const shot = await command('Page.captureScreenshot', { format: 'png' });
      await writeFile(join(process.env.PLUMB_E2E_SCREENSHOTS, `task-sticky-${width}.png`), Buffer.from(shot.result.data, 'base64'));
    }
    await evaluate(`${sticky}.querySelector('summary').focus()`);
    await command('Input.dispatchKeyEvent', { type: 'keyDown', key: ' ', code: 'Space', windowsVirtualKeyCode: 32 });
    await command('Input.dispatchKeyEvent', { type: 'keyUp', key: ' ', code: 'Space', windowsVirtualKeyCode: 32 });
    assert.equal(await evaluate(`${sticky}.querySelector('details').open`), true, await evaluate(`JSON.stringify({active:document.activeElement.outerHTML, scroll:${pane}.scrollTop, nav:${sticky}.outerHTML})`));
    assert.deepEqual(await evaluate(`[...${sticky}.querySelectorAll('.task-sticky-ancestors button')].map(b => b.textContent)`), ['Country', 'Department', 'Team']);
    await command('Input.dispatchKeyEvent', { type: 'keyDown', key: 'Escape', code: 'Escape', windowsVirtualKeyCode: 27 });
    assert.equal(await evaluate(`${sticky}.querySelector('details').open`), false);
    await command('Input.dispatchKeyEvent', { type: 'keyDown', key: 'Enter', code: 'Enter', windowsVirtualKeyCode: 13, text: '\r' });
    await command('Input.dispatchKeyEvent', { type: 'keyUp', key: 'Enter', code: 'Enter', windowsVirtualKeyCode: 13 });
    assert.equal(await evaluate(`${sticky}.querySelector('details').open`), true);
    await evaluate(`${sticky}.querySelector('.task-sticky-ancestors button').click()`);
    await settle();
    assert.equal(await evaluate('document.activeElement.textContent'), 'Country');
    await scrollTo('Document child 12');
    assert.deepEqual(await labels(), ['Document task']);
    // Resizing and changing views must recompute geometry without changing selection.
    await command('Emulation.setDeviceMetricsOverride', { width: width === 390 ? 1280 : 390, height: 740, deviceScaleFactor: 1, mobile: width !== 390 });
    await settle();
    assert.deepEqual(await labels(), ['Document task']);
    assert.equal(await evaluate('document.documentElement.scrollWidth > document.documentElement.clientWidth'), false);
    await evaluate(`${pane}.scrollTop = 0`);
    await settle();
    assert.equal(await visible(), false);
    await navigate(`${base}/tasks?q=NoSuchTaskAnywhere`);
    await until(() => evaluate(`!document.querySelector('#task-empty').hidden`), 'empty search did not complete');
    await settle();
    assert.equal(await visible(), false);
    // The same module receives a new DOM and index after live source changes.
    await navigate(`${base}/tasks`);
    await until(() => evaluate(`Boolean((${row})('Alpha 12'))`), 'return to task list failed');
    await scrollTo('Alpha 12');
    await writeFile(join(root, 'notebook.plumb'), source.replace('First company', 'Renamed company'));
    await until(() => evaluate(`${sticky}.textContent.includes('Renamed company')`), 'live revision kept old context');
    assert.equal(await evaluate(`${sticky}.textContent.includes('First company')`), false);
    const longTitle = 'Long ancestor '.repeat(30).trim();
    await writeFile(join(root, 'notebook.plumb'), source.replace('First company', longTitle));
    await until(() => evaluate(`${sticky}.querySelector(':scope > button:last-child')?.title === ${JSON.stringify(longTitle)}`), 'long ancestor did not refresh');
    assert.equal(await evaluate(`${sticky}.scrollWidth > ${sticky}.clientWidth`), false, 'long title overflow');
    assert.equal(await evaluate(`${sticky}.getBoundingClientRect().height`), 40);
    await writeFile(join(root, 'notebook.plumb'), source);
    await until(() => evaluate(`${sticky}.textContent.includes('First company')`), 'live revision did not restore context');
    await evaluate(`(${row})('Alpha 12').querySelector('.task-row').click()`);
    await until(() => evaluate(`document.querySelector('#task-panel h1')?.textContent === 'Alpha 12'`), 'selection did not load');
    await scrollTo('Beta 12');
    assert.deepEqual(await labels(), ['Notebook', 'Second company'], 'sticky context followed selection');
    assert.equal(await evaluate(`document.querySelector('#task-panel h1').textContent`), 'Alpha 12');
    await evaluate(`document.querySelector('#task-panel .jump-task').click()`);
    await settle();
    assert.equal(await evaluate(`(${row})('Alpha 12').getBoundingClientRect().top >= ${pane}.getBoundingClientRect().top + 39`), true, 'manual task jump hidden by sticky context');
  }
  console.log('Sticky task context browser checks passed: branch/document transitions, compact ancestry, keyboard overflow, ancestor jump, folds, empty search, resize, live revisions and zero scroll queries.');

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
