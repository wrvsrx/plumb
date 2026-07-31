import assert from 'node:assert/strict';
import { writeFile } from 'node:fs/promises';

const port = process.env.PLUMB_CDP_PORT || '9223';
const pages = await fetch(`http://127.0.0.1:${port}/json`).then((response) => response.json());
const page = pages.find((candidate) => candidate.type === 'page');
assert.ok(page, 'Chromium page target is available');

const socket = new WebSocket(page.webSocketDebuggerUrl);
await new Promise((resolve, reject) => {
  socket.addEventListener('open', resolve, { once: true });
  socket.addEventListener('error', reject, { once: true });
});
let sequence = 0;
const pending = new Map();
socket.addEventListener('message', (event) => {
  const message = JSON.parse(event.data);
  const callback = pending.get(message.id);
  if (callback) {
    pending.delete(message.id);
    callback(message);
  }
});

function command(method, params = {}) {
  const id = ++sequence;
  socket.send(JSON.stringify({ id, method, params }));
  return new Promise((resolve) => pending.set(id, resolve));
}

await command('Page.navigate', {
  url: process.env.PLUMB_E2E_URL || 'http://127.0.0.1:38922/tasks',
});

async function evaluate(expression) {
  const response = await command('Runtime.evaluate', {
    expression,
    awaitPromise: true,
    returnByValue: true,
  });
  assert.equal(response.result.exceptionDetails, undefined, response.result.exceptionDetails?.text);
  return response.result.result.value;
}

await evaluate(`(async () => {
  const waitFor = async (test) => {
    for (let attempt = 0; attempt < 100; attempt += 1) {
      const value = test();
      if (value) return value;
      await new Promise((resolve) => setTimeout(resolve, 50));
    }
    throw new Error('timed out waiting for browser state');
  };
  await waitFor(() => document.querySelector('.task-row'));
  document.querySelector('#new-task').click();
  const form = await waitFor(() => document.querySelector('.task-form'));
  const title = 'Browser authored task ' + Date.now();
  window.__plumbE2eTaskTitle = title;
  form.elements.title.value = title;
  form.elements.recur.value = 'P1D';
  form.requestSubmit();
  await waitFor(() => /require a due date/i.test(form.querySelector('[data-field="recur"]').textContent));
  form.elements.due.value = '2099-08-15T10:30';
  form.elements.priority.value = '7';
  form.requestSubmit();
  await waitFor(() => !document.querySelector('.task-form') && /Task created/.test(document.querySelector('#notification').textContent));
  const row = await waitFor(() => Array.from(document.querySelectorAll('.task-row')).find((candidate) => candidate.querySelector('strong')?.textContent === title));
  row.click();
  await waitFor(() => document.querySelector('.task-detail .edit-task'));
  document.querySelector('.task-detail .edit-task').click();
  const edit = await waitFor(() => document.querySelector('.task-form'));
  edit.elements.priority.value = '-7';
  const parent = Array.from(edit.elements.parent.options).find((option) => option.value);
  if (parent) {
    edit.elements.parent.value = parent.value;
    edit.elements.parent.dispatchEvent(new Event('change'));
  }
  edit.requestSubmit();
  await waitFor(() => !document.querySelector('.task-form') && /Task updated/.test(document.querySelector('#notification').textContent));
  return {
    notification: document.querySelector('#notification').textContent,
    title: document.querySelector('.task-detail h1')?.textContent,
    priority: Array.from(document.querySelectorAll('.task-fields dd')).map((node) => node.textContent).includes('-7'),
    overflow: document.documentElement.scrollWidth > document.documentElement.clientWidth,
  };
})()`);

const result = await evaluate(`(async () => {
  const snapshot = await fetch('/api/tasks').then((response) => response.json());
  const task = snapshot.tasks.find((candidate) => candidate.title === window.__plumbE2eTaskTitle);
  return {
    notification: document.querySelector('#notification').textContent,
    title: task?.title,
    priority: task?.priority,
    depth: task?.depth,
    overflow: document.documentElement.scrollWidth > document.documentElement.clientWidth,
  };
})()`);
assert.match(result.notification, /Task updated/);
assert.equal(result.title, await evaluate('window.__plumbE2eTaskTitle'));
assert.equal(result.priority, -7);
assert.ok(result.depth > 0, 'edited task was reparented');
assert.equal(result.overflow, false);

await evaluate(`document.querySelector('.task-detail .edit-task').click()`);
await evaluate(`(async () => {
  for (let attempt = 0; attempt < 40 && !document.querySelector('.task-form'); attempt += 1) {
    await new Promise((resolve) => setTimeout(resolve, 25));
  }
  if (!document.querySelector('.task-form')) throw new Error('edit form did not open');
})()`);
assert.equal(await evaluate(`document.querySelector('#task-panel').scrollWidth > document.querySelector('#task-panel').clientWidth`), false);
const desktop = await command('Page.captureScreenshot', { format: 'png' });
await writeFile('/tmp/plumb-task-authoring-desktop.png', Buffer.from(desktop.result.data, 'base64'));
await command('Emulation.setDeviceMetricsOverride', {
  width: 390, height: 844, deviceScaleFactor: 1, mobile: true,
});
await evaluate(`new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve)))`);
assert.equal(await evaluate(`document.documentElement.scrollWidth > document.documentElement.clientWidth`), false);
assert.equal(await evaluate(`document.querySelector('#task-panel').scrollWidth > document.querySelector('#task-panel').clientWidth`), false);
const mobile = await command('Page.captureScreenshot', { format: 'png' });
await writeFile('/tmp/plumb-task-authoring-mobile.png', Buffer.from(mobile.result.data, 'base64'));
socket.close();
console.log('task authoring browser E2E passed');
