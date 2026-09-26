// Self-contained integration test. Build target/debug/plumb first.
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { once } from 'node:events';
import { mkdtemp, mkdir, writeFile, unlink, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { resolve, join } from 'node:path';
import net from 'node:net';

const port = Number(process.env.PLUMB_CONTEXT_TEST_PORT || 38947);
const cdpPort = Number(process.env.PLUMB_CONTEXT_CDP_PORT || 9237);
const origin = `http://127.0.0.1:${port}`;
const temporary = await mkdtemp(join(tmpdir(), 'plumb-task-context-'));
const children = [];
let socket;
const delay = (ms) => new Promise((done) => setTimeout(done, ms));
async function waitFor(test) {
  for (let i = 0; i < 200; i += 1) {
    const result = await test();
    if (result) return result;
    await delay(50);
  }
  throw new Error('Timed out');
}
async function checkPort(value) {
  const server = net.createServer();
  server.listen(value, '127.0.0.1');
  await once(server, 'listening');
  await new Promise((done) => server.close(done));
}
function start(command, args, env = process.env) {
  const child = spawn(command, args, { env, stdio: ['ignore', 'ignore', 'pipe'] });
  let stderr = '';
  child.stderr.on('data', (chunk) => { stderr += chunk; });
  child.on('error', (error) => { stderr += error.message; });
  child.testError = () => stderr;
  children.push(child);
  return child;
}
try {
  await checkPort(port); await checkPort(cdpPort);
  const root = join(temporary, 'workspace'); await mkdir(root);
  for (let i = 0; i < 260; i += 1) {
    const source = i === 120
      ? '`- Parent\n `+ task\n `@ parent\n `= priority 1\n `- Routine\n  `+ task\n  `@ recur\n  `= due 2026-09-24T09:00:00+08:00\n  `= recur P1D\n `- Neighbor\n  `+ task\n  `@ neighbor\n'
      : `\`- Task ${i}\n \`+ task\n \`@ task-${i}\n \`= priority 1\n`;
    await writeFile(join(root, `${String(i).padStart(3, '0')}.plumb`), source);
  }
  const server = start(resolve('target/debug/plumb'), ['site', 'serve', '--root', root, '--port', String(port)], {
    ...process.env, PLUMB_CACHE_DIR: join(temporary, 'cache'),
  });
  await waitFor(async () => {
    if (server.exitCode !== null) throw new Error(server.testError());
    return fetch(`${origin}/tasks`).then((r) => r.ok).catch(() => false);
  });
  const browser = start(process.env.CHROMIUM || 'chromium', [
    '--headless', '--no-sandbox', '--disable-gpu', '--disable-dev-shm-usage',
    `--remote-debugging-port=${cdpPort}`, `--user-data-dir=${join(temporary, 'browser')}`, 'about:blank',
  ]);
  const pages = await waitFor(async () => {
    if (browser.exitCode !== null) throw new Error(browser.testError());
    return fetch(`http://127.0.0.1:${cdpPort}/json`).then((r) => r.json()).catch(() => null);
  });
  socket = new WebSocket(pages.find((page) => page.type === 'page').webSocketDebuggerUrl);
  await once(socket, 'open');
  let sequence = 0; const pending = new Map();
  socket.addEventListener('message', ({ data }) => {
    const message = JSON.parse(data);
    if (pending.has(message.id)) { pending.get(message.id)(message); pending.delete(message.id); }
  });
  function command(method, params = {}) {
    const id = ++sequence;
    socket.send(JSON.stringify({ id, method, params }));
    return new Promise((done) => pending.set(id, done));
  }
  async function evaluate(expression) {
    const response = await command('Runtime.evaluate', { expression, awaitPromise: true, returnByValue: true });
    assert.equal(response.error, undefined, JSON.stringify(response.error));
    assert.equal(response.result.exceptionDetails, undefined, JSON.stringify(response.result.exceptionDetails));
    return response.result.result.value;
  }
  await command('Emulation.setDeviceMetricsOverride', { width: 1200, height: 800, deviceScaleFactor: 1, mobile: false });
  await command('Page.navigate', { url: `${origin}/tasks` });
  await waitFor(() => evaluate('Boolean(document.querySelector(".load-more-tasks"))'));
  const result = await evaluate(`(async () => {
    const wait = async (test) => {
      for (let i = 0; i < 200; i++) { if (test()) return; await new Promise(r => setTimeout(r, 50)); }
      throw new Error('browser state timed out');
    };
    const rows = () => [...document.querySelectorAll('.task-list-item')];
    const row = (id) => rows().find(r => r.dataset.taskKey.endsWith(':' + id));
    const select = (id) => row(id).querySelector('.task-row').click();
    window.contextTest = { wait, rows, row, select };
    select('task-0');
    const listPane = document.querySelector('.task-list-pane');
    listPane.scrollTop = listPane.scrollHeight;
    const pageScroll = listPane.scrollTop;
    document.querySelector('.load-more-tasks').click();
    await wait(() => row('recur'));
    if (Math.abs(listPane.scrollTop - pageScroll) > 1) throw new Error('Load more pulled the viewport back to the old selection');
    const loaded = rows().length;
    row('recur').scrollIntoView({ block: 'center' });
    select('recur');
    document.querySelector('.complete-task').click();
    await wait(() => !row('recur') && document.querySelector('.task-detail h1')?.textContent === 'Neighbor');
    const after = rows().length;
    if (after < loaded) throw new Error('refresh lost loaded extent');
    if (!new URL(location.href).searchParams.get('selected').endsWith(':neighbor')) throw new Error('URL selection disagrees');
    if (!row('neighbor').classList.contains('selected')) throw new Error('row selection disagrees');
    const bounds = row('neighbor').getBoundingClientRect();
    const pane = document.querySelector('.task-list-pane').getBoundingClientRect();
    if (bounds.top < pane.top - 1 || bounds.bottom > pane.bottom + 1) throw new Error('mapped focus not visible ' + JSON.stringify({ row: bounds.toJSON(), pane: pane.toJSON(), scroll: document.querySelector('.task-list-pane').scrollTop }));
    // Keep an unchanged neighboring document on screen as the selected tree moves.
    row('task-121').scrollIntoView({ block: 'start' });
    const neighborTop = () => row('task-121').getBoundingClientRect().top;
    const readingTop = neighborTop();
    document.querySelector('.focus-task').click();
    await wait(() => row('neighbor').classList.contains('focused'));
    if (rows().length < after) throw new Error('focus groups swallowed the old frontier');
    if (Math.abs(neighborTop() - readingTop) > 1) throw new Error('Focus moved the reading context ' + readingTop + ' -> ' + neighborTop());
    select('parent');
    const jump = [...document.querySelectorAll('.task-children button')].find(button => button.textContent.includes('Neighbor'));
    if (!jump) throw new Error('missing child navigation');
    jump.click();
    const targetBounds = row('neighbor').getBoundingClientRect();
    const targetPane = listPane.getBoundingClientRect();
    if (targetBounds.top < targetPane.top - 1 || targetBounds.bottom > targetPane.bottom + 1) throw new Error('explicit navigation did not reveal target');
    row('task-121').scrollIntoView({ block: 'start' });
    document.querySelector('.focus-task').click();
    await wait(() => !row('neighbor').classList.contains('focused'));
    if (Math.abs(neighborTop() - readingTop) > 1) throw new Error('Unfocus moved the reading context');
    // Reorder this document beyond the old prefix; retention must cover its new position.
    row('task-151').scrollIntoView({ block: 'start' });
    const priorityTop = row('task-151').getBoundingClientRect().top;
    select('task-150');
    document.querySelector('.task-property-value[data-property="priority"]').click();
    const form = document.querySelector('.task-property-editor');
    form.elements.value.value = '-100'; form.requestSubmit();
    await wait(() => document.querySelector('.task-property-value[data-property="priority"]')?.textContent === '-100');
    if (!row('task-150') || rows().length < 260) throw new Error('reordered selection was paged away ' + JSON.stringify({ count: rows().length, present: !!row('task-150'), summary: document.querySelector('#task-summary').textContent, selected: document.querySelector('.task-detail h1')?.textContent, notice: document.querySelector('#notification').textContent }));
    if (Math.abs(row('task-151').getBoundingClientRect().top - priorityTop) > 1) throw new Error('Priority edit moved the reading context');
    return { loaded, after, selected: document.querySelector('.task-detail h1').textContent };
  })()`);
  assert.equal(result.selected, 'Task 150');
  console.log('Pagination, recur completion, selection, grouping and reorder:', result);
  // An external deletion goes through the same coordinator as local mutations.
  await unlink(join(root, '150.plumb'));
  await waitFor(() => evaluate('!contextTest.row("task-150")'));
  assert.equal(await evaluate('document.querySelector(".task-detail h1")?.textContent === "Task 150"'), false);
  // User selection while a mutation is awaiting its response wins over the old handler.
  await evaluate(`(() => {
    const original = window.fetch;
    window.fetch = async (...args) => {
      const response = await original(...args);
      if (String(args[0]).endsWith('/focus')) {
        window.focusResponsePending = true;
        await new Promise(r => setTimeout(r, 700));
      }
      return response;
    };
    contextTest.select('task-259');
    document.querySelector('.focus-task').click();
  })()`);
  await waitFor(() => evaluate('Boolean(window.focusResponsePending)'));
  await evaluate("contextTest.select('task-220')");
  await waitFor(() => evaluate("contextTest.row('task-259')?.classList.contains('focused')"));
  assert.equal(await evaluate('document.querySelector(".task-detail h1").textContent'), 'Task 220');
  const selection = await evaluate('new URL(location.href).searchParams.get("selected")');
  for (const width of [390, 1200]) {
    await command('Emulation.setDeviceMetricsOverride', { width, height: 800, deviceScaleFactor: 1, mobile: width === 390 });
    await evaluate('new Promise(r => requestAnimationFrame(() => requestAnimationFrame(r)))');
    assert.equal(await evaluate('new URL(location.href).searchParams.get("selected")'), selection);
    assert.equal(await evaluate('document.querySelector(".task-detail h1").textContent'), 'Task 220');
  }
  console.log('In-flight selection and viewport resize passed');
  // Delayed old responses cannot replace a newer search result.
  await evaluate(`(() => {
    const original = window.fetch;
    window.fetch = async (...args) => {
      const response = await original(...args);
      if (String(args[0]).includes('/api/query') && JSON.parse(args[1]?.body || '{}').query === 'Task 1') {
        await new Promise(r => setTimeout(r, 900));
      }
      return response;
    };
    const search = document.querySelector('#task-search');
    search.value = 'Task 1'; search.dispatchEvent(new Event('input'));
  })()`);
  await delay(350);
  await evaluate(`(() => { const input = document.querySelector('#task-search'); input.value = 'Task 259'; input.dispatchEvent(new Event('input')); })()`);
  await waitFor(() => evaluate('Boolean(contextTest.row("task-259")) && contextTest.rows().length === 1'));
  await delay(1000);
  assert.equal(await evaluate('contextTest.rows().length'), 1);
  assert.equal(await evaluate('Boolean(contextTest.row("task-259"))'), true);
  console.log('External deletion and stale response exclusion passed');
  await evaluate(`(async () => {
    const { wait } = contextTest;
    const selected = new URL(location.href).searchParams.get('selected');
    document.querySelector('#new-task').click();
    await wait(() => document.querySelector('.task-form [name="title"]') === document.activeElement);
    const form = document.querySelector('.task-form');
    form.elements.title.value = 'Task 259';
    form.requestSubmit();
    await wait(() => !document.querySelector('.task-form') && /Task created/.test(document.querySelector('#notification').textContent));
    if (new URL(location.href).searchParams.get('selected') !== selected) throw new Error('same-title insertion stole selection');
  })()`);
  console.log('Same-title insertion preserves selection');
} finally {
  socket?.close();
  for (const child of children.reverse()) {
    if (child.exitCode === null && child.signalCode === null) {
      const exited = once(child, 'exit'); child.kill('SIGTERM');
      await Promise.race([exited, delay(3000)]);
      if (child.exitCode === null && child.signalCode === null) { child.kill('SIGKILL'); await exited; }
    }
  }
  await rm(temporary, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 });
}
