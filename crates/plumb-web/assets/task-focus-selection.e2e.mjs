import assert from 'node:assert/strict';

const port = process.env.PLUMB_CDP_PORT || '9226';
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

async function evaluate(expression) {
  const response = await command('Runtime.evaluate', {
    expression,
    awaitPromise: true,
    returnByValue: true,
  });
  assert.equal(response.result.exceptionDetails, undefined, response.result.exceptionDetails?.text);
  return response.result.result.value;
}


// Fixture: one anonymous focused task "给任博发 CVM 2027 投稿", plus stable
// neighboring tasks in a.plumb. Only run against an isolated test notebook.
try {
  await command('Emulation.setDeviceMetricsOverride', { width: 1280, height: 844, deviceScaleFactor: 1, mobile: false });
  const base = process.env.PLUMB_E2E_URL || 'http://127.0.0.1:3127/tasks';
  for (const filtered of [false, true]) {
    await command('Page.navigate', { url: base + (filtered ? '?cel=focused' : '') });
    const result = await evaluate(`(async () => {
      const waitFor = async (test) => {
        for (let i = 0; i < 160; i++) {
          if (test()) return;
          await new Promise(resolve => setTimeout(resolve, 50));
        }
        throw new Error('Timed out: ' + test.toString());
      };
      await waitFor(() => [...document.querySelectorAll('.task-row')].some(row => row.textContent.includes('给任博发 CVM 2027 投稿')));
      const row = [...document.querySelectorAll('.task-row')].find(row => row.textContent.includes('给任博发 CVM 2027 投稿'));
      const key = row.closest('[data-task-key]').dataset.taskKey;
      row.click();
      await waitFor(() => document.querySelector('.task-detail h1')?.textContent === '给任博发 CVM 2027 投稿');
      const pane = document.querySelector('.task-list-item').parentElement.parentElement;
      if (!${filtered}) {
        document.querySelectorAll('.task-unfocused-group[aria-expanded="false"]').forEach(group => group.click());
        pane.scrollTop = 700;
      }
      const before = pane.scrollTop;
      const anchor = [...document.querySelectorAll('[data-task-key]')].find(row => row.dataset.taskKey !== key && row.getBoundingClientRect().bottom > pane.getBoundingClientRect().top);
      const anchorKey = anchor?.dataset.taskKey;
      const anchorOffset = anchor?.getBoundingClientRect().top;
      const focus = () => document.querySelector('.task-detail .focus-task');
      if (focus().textContent !== 'Unfocus') throw new Error('Fixture must initially be focused');
      focus().click();
      await waitFor(() => focus()?.textContent === 'Focus' && !focus().disabled);
      // Also allow the queued workspace notification to complete its refresh.
      await new Promise(resolve => setTimeout(resolve, 300));
      const selected = new URL(location.href).searchParams.get('selected');
      const title = document.querySelector('.task-detail h1')?.textContent;
      const jump = document.querySelector('.jump-task');
      const updatedAnchor = [...document.querySelectorAll('[data-task-key]')].find(row => row.dataset.taskKey === anchorKey);
      const out = { key, selected, title, before, after: pane.scrollTop,
        anchorShift: updatedAnchor ? updatedAnchor.getBoundingClientRect().top - anchorOffset : null,
        jumpDisabled: jump.getAttribute('aria-disabled'), reason: jump.title,
        rowPresent: [...document.querySelectorAll('[data-task-key]')].some(row => row.dataset.taskKey === key) };
      focus().click();
      await waitFor(() => focus()?.textContent === 'Unfocus' && !focus().disabled);
      out.refocusedSelection = new URL(location.href).searchParams.get('selected');
      return out;
    })()`);
    assert.equal(result.selected, result.key);
    assert.equal(result.refocusedSelection, result.key);
    assert.equal(result.title, '给任博发 CVM 2027 投稿');
    if (filtered) {
      assert.equal(result.rowPresent, false);
      assert.equal(result.jumpDisabled, 'true');
      assert.ok(result.reason);
    } else {
      assert.equal(result.jumpDisabled, 'false');
      assert.ok(result.anchorShift !== null && Math.abs(result.anchorShift) < 3, JSON.stringify(result));
    }
    console.log(JSON.stringify({ filtered, ...result }));
  }
} finally {
  socket.close();
}
