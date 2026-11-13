// Self-contained browser regression. Run under an explicit hard memory limit.
// Requires a built target/debug/plumb and chromium (or PLUMB_E2E_CHROMIUM).
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { mkdtemp, mkdir, writeFile, rm } from 'node:fs/promises';
import { createServer } from 'node:net';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';

const port = Number(process.env.PLUMB_E2E_PORT || 38949);
const cdpPort = Number(process.env.PLUMB_CDP_PORT || 9249);
const base = `http://127.0.0.1:${port}`;
const delay = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
async function available(port) {
  const server = createServer();
  await new Promise((resolve, reject) => server.once('error', reject).listen(port, '127.0.0.1', resolve));
  await new Promise((resolve) => server.close(resolve));
}
await available(port);
await available(cdpPort);
const temp = await mkdtemp(join(tmpdir(), 'plumb-node-detail-e2e-'));
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
  await writeFile(join(root, 'project.plumb'), '`+ task\n`= title Project\n`= date 2026-11-13\n`= timezone +08:00\n\nDocument body.\n\n`- Rich `!{task}\n `+ task\n `@ target\n `= priority 3\n\n Own `*{body} with `->{other other.plumb} and `->{image image.svg `+{embed}}.\n `- Child\n  `+ task\n  `@ child\n\n  Child body.\n\n`- Sibling\n `+ task\n `@ sibling\n\n Sibling body.\n\n`- 10:00--11:00 `!{Meeting}\n `+ event\n `@ meeting\n\n Event `*{details}.\n');
  await writeFile(join(root, 'hierarchy.plumb'), '`= title Notebook\n\n`# Earlier\n\n`- Top `!{section}\n `()\n  `- Nested\n   `- Nested task\n    `+ task\n    `@ nested\n\n    Nested body.\n');
  await writeFile(join(root, 'other.plumb'), '`= title Ordinary\n\nOther `!{document}.\n');
  await writeFile(join(root, 'image.svg'), '<svg xmlns="http://www.w3.org/2000/svg" width="20" height="20"><rect width="20" height="20"/></svg>');
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
  const graph = await fetch(`${base}/api/graph`).then(r => r.json());
  const project = graph.nodes.find(node => node.title === 'Project').id;
  const ordinary = graph.nodes.find(node => node.title === 'Ordinary').id;
  const loaded = (host) => evaluate(`Boolean(document.querySelector('${host} .note-content:not([aria-busy])'))`);
  const select = async (title) => {
    await until(() => evaluate(`Boolean((${row})(${JSON.stringify(title)}))`), `missing task ${title}`);
    await evaluate(`(${row})(${JSON.stringify(title)}).querySelector('.task-row').click()`);
  };
  for (const width of [1280, 390]) {
    await command('Emulation.setDeviceMetricsOverride', { width, height: 900, deviceScaleFactor: 1, mobile: width < 600 });
    await navigate(`${base}/graph?current=${ordinary}`);
    await until(() => loaded('#note-panel'), 'ordinary document missing');
    assert.equal(await evaluate(`document.querySelector('#note-panel .node-detail').dataset.facet`), '');
    assert.equal(await evaluate(`document.querySelector('#note-panel .note-content strong').textContent`), 'document');
    await navigate(`${base}/graph?current=${project}`);
    await until(() => evaluate(`Boolean(document.querySelector('#note-panel .task-detail .complete-task'))`), 'graph task facet missing');
    const graphBody = await evaluate(`document.querySelector('#note-panel .note-content').innerHTML`);
    const graphFields = await evaluate(`document.querySelector('#note-panel .task-fields').textContent`);
    await navigate(`${base}/tasks`);
    await select('Nested task');
    await until(() => loaded('#task-panel'), 'nested content missing');
    assert.deepEqual(await evaluate(`[...document.querySelectorAll('#task-panel .node-breadcrumbs button')].map(b => b.textContent)`), ['Notebook', 'Top section', 'Nested']);
    assert.equal(await evaluate(`(${row})('Nested task').querySelector('.task-ancestry').textContent`), 'Notebook / Top section / Nested');
    assert.equal(await evaluate(`(${row})('Nested task').querySelector('.task-ancestry').title`), 'Notebook / Top section / Nested');
    assert.equal(await evaluate(`document.querySelector('#task-panel .task-fields').textContent.includes('nested')`), true);
    await evaluate(`Object.defineProperty(navigator, 'clipboard', {configurable: true, value: {writeText: async text => { window.copiedId = text; }}}); [...document.querySelectorAll('#task-panel button')].find(b => b.textContent === 'Copy ID').click()`);
    await until(() => evaluate(`window.copiedId === 'nested'`), 'copy ID missing');
    await evaluate(`document.querySelectorAll('#task-panel .node-breadcrumbs button')[2].click()`);
    await until(() => evaluate(`document.querySelector('#task-panel h1')?.textContent === 'Nested' && !document.querySelector('#task-panel [aria-busy]')`), 'idless section breadcrumb missing');
    assert.equal(await evaluate(`document.querySelector('#task-panel .note-content').textContent.includes('Nested body')`), true);
    await select('Project');
    await until(() => loaded('#task-panel'), 'document task content missing');
    assert.equal(await evaluate(`document.querySelector('#task-panel .note-content').innerHTML`), graphBody);
    assert.equal(await evaluate(`document.querySelector('#task-panel .task-fields').textContent`), graphFields);
    await select('Rich task');
    await until(() => loaded('#task-panel'), 'task content missing');
    assert.equal(await evaluate(`document.querySelector('#task-panel h1 strong').textContent`), 'task');
    assert.equal(await evaluate(`document.querySelector('#task-panel .note-content em').textContent`), 'body');
    assert.equal(await evaluate(`document.querySelector('#task-panel .note-content img').getAttribute('src').startsWith('/resource/')`), true);
    assert.equal(await evaluate(`document.querySelector('#task-panel .note-content').textContent.includes('Sibling')`), false);
    assert.equal(await evaluate(`document.querySelector('#task-panel .note-content').textContent.includes('Document body')`), false);
    await evaluate(`document.querySelector('#task-panel .task-children button').click()`);
    await until(() => evaluate(`document.querySelector('#task-panel .note-content')?.textContent.includes('Child body')`), 'child navigation lost');
    assert.deepEqual(await evaluate(`[...document.querySelectorAll('#task-panel .node-breadcrumbs button')].map(b => b.textContent)`), ['Project', 'Rich task']);
    await evaluate(`document.querySelectorAll('#task-panel .node-breadcrumbs button')[1].click()`);
    await until(() => evaluate(`document.querySelector('#task-panel h1')?.textContent === 'Rich task' && !document.querySelector('#task-panel [aria-busy]')`), 'parent breadcrumb missing');
    await evaluate(`document.querySelector('#task-panel .node-breadcrumbs button').click()`);
    await until(() => evaluate(`document.querySelector('#task-panel .note-content')?.textContent.includes('Document body')`), 'document breadcrumb missing');
    await select('Rich task');
    await until(() => loaded('#task-panel'), 'task content missing again');
    await evaluate(`document.querySelector('#task-panel .note-content a[data-plumb-document]').click()`);
    await until(() => evaluate(`document.querySelector('#note-panel .note-content strong')?.textContent === 'document'`), 'body link did not navigate');
    await navigate(`${base}/agenda`);
    await until(() => evaluate(`Boolean(document.querySelector('.event-row'))`), 'agenda event missing');
    await evaluate(`document.querySelector('.event-row').click()`);
    await until(() => loaded('#event-panel'), 'event content missing');
    assert.equal(await evaluate(`document.querySelector('#event-panel h1').textContent`), 'Meeting');
    assert.equal(await evaluate(`document.querySelector('#event-panel h1 strong').textContent`), 'Meeting');
    assert.equal(await evaluate(`document.querySelector('#event-panel .note-content em').textContent`), 'details');
    assert.equal(await evaluate(`document.querySelector('#event-panel .note-content').textContent.includes('Own')`), false);
    assert.equal(await evaluate('document.documentElement.scrollWidth > document.documentElement.clientWidth'), false);
  }
  // Hold a real response, switch selection, then release it out of order.
  await navigate(`${base}/tasks`);
  await select('Sibling');
  await until(() => loaded('#task-panel'), 'sibling content missing');
  const holdNext = `(() => {
    const original = window.fetch;
    window.fetch = async (...args) => {
      const response = await original(...args);
      if (String(args[0]).includes('/api/node/') && String(args[0]).includes('taskId=target')) {
        window.fetch = original;
        await new Promise(resolve => { window.releaseDetail = resolve; });
      }
      return response;
    };
  })()`;
  await evaluate(holdNext);
  await select('Rich task');
  await until(() => evaluate(`typeof window.releaseDetail === 'function'`), 'request was not held');
  await select('Sibling');
  await until(() => loaded('#task-panel'), 'new selection did not load');
  await evaluate(`window.releaseDetail()`);
  await delay(200);
  assert.equal(await evaluate(`document.querySelector('#task-panel h1').textContent`), 'Sibling');
  assert.equal(await evaluate(`document.querySelector('#task-panel .note-content').textContent.includes('Own')`), false);
  // A late result cannot replace a form the user has started filling.
  await evaluate(`delete window.releaseDetail`);
  await evaluate(holdNext);
  await select('Rich task');
  await until(() => evaluate(`typeof window.releaseDetail === 'function'`), 'second request was not held');
  await evaluate(`document.querySelector('#task-panel .edit-task').click()`);
  await until(() => evaluate(`Boolean(document.querySelector('#task-panel form input[name="title"]'))`), 'task editor missing');
  await evaluate(`document.querySelector('#task-panel input[name="title"]').value = 'Unsaved edit'; window.releaseDetail()`);
  await delay(200);
  assert.equal(await evaluate(`document.querySelector('#task-panel input[name="title"]').value`), 'Unsaved edit');
  // Content failures stay visible; never substitute the containing document.
  await evaluate(`(() => { const original = window.fetch; window.fetch = (...args) => {
    if (String(args[0]).includes('/api/node/')) { window.fetch = original; return Promise.resolve(new Response('stale revision', {status: 409})); }
    return original(...args);
  }; })()`);
  await select('Sibling');
  await until(() => evaluate(`document.querySelector('#task-panel .node-content-error')?.textContent.includes('stale revision')`), 'content error was hidden');
  // A task opened from Graph uses that identity when entering its editor.
  await navigate(`${base}/graph?current=${project}`);
  await until(() => evaluate(`Boolean(document.querySelector('#note-panel .edit-task'))`), 'graph edit action missing');
  await evaluate(`document.querySelector('#note-panel .edit-task').click()`);
  await until(() => evaluate(`document.querySelector('#task-panel input[name="title"]')?.value === 'Project'`), 'graph task action targeted another task');
  console.log('Unified node detail checks passed: Graph/Tasks parity, rich task/event content, scope, child/link navigation, desktop/mobile, stale responses, editor preservation and error states.');

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
