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

// Fixture: focused grandchild Focused under Parent / Branch in a.plumb;
// unfocused sibling Quiet with child Quiet child, root Loose in the same file;
// and an entirely unfocused b.plumb containing Other file. Serve this isolated
// fixture on the explicit PLUMB_E2E_URL test port; this test mutates Quiet child.
try {
  for (const width of [1280, 390]) {
    await command('Emulation.setDeviceMetricsOverride', { width, height: 844, deviceScaleFactor: 1, mobile: width < 900 });
    await command('Page.navigate', { url: process.env.PLUMB_E2E_URL || 'http://127.0.0.1:38927/tasks' });
    const result = await evaluate(`(async () => {
      const waitFor = async (test) => {
        for (let i = 0; i < 100; i++) {
          if (test()) return;
          await new Promise((resolve) => setTimeout(resolve, 50));
        }
        throw new Error('Timed out waiting for focus groups');
      };
      const rows = () => [...document.querySelectorAll('.task-list-item')];
      const titles = () => rows().map((row) => row.querySelector('strong').textContent);
      const row = (title) => rows().find((item) => item.querySelector('strong').textContent === title);
      const group = (part) => [...document.querySelectorAll('.task-unfocused-group')].find((item) => item.dataset.groupKey.includes(part));
      const file = () => document.querySelector('.task-document-toggle');
      await waitFor(() => row('Focused'));
      const initial = titles();
      const noNext = !document.querySelector('#task-mode-next');
      const badge = file().querySelector('.task-focused-count').textContent;
      const groupCount = document.querySelectorAll('.task-unfocused-group').length;
      file().click();
      const collapsedBadge = file().querySelector('.task-focused-count').textContent;
      const collapsedRows = rows().length;
      file().click();
      // Expand all automatic groups. A subtree is shown as a whole, not another
      // series of automatically folded unfocused descendants.
      for (let i = 0; i < 10; i++) {
        const closed = document.querySelector('.task-unfocused-group[aria-expanded="false"]');
        if (!closed) break;
        closed.click();
      }
      const expanded = titles();
      row('Quiet child').querySelector('.task-row').click();
      await waitFor(() => document.querySelector('#task-panel .focus-task'));
      document.querySelector('#task-panel .focus-task').click();
      await waitFor(() => row('Quiet child')?.classList.contains('focused') && !document.querySelector('#task-panel .focus-task')?.disabled);
      const focused = titles();
      const newBadge = file().querySelector('.task-focused-count').textContent;
      const filesStayedOpen = group('files')?.getAttribute('aria-expanded');
      document.querySelector('#task-panel .focus-task').click();
      await waitFor(() => row('Quiet child') && !row('Quiet child').classList.contains('focused') && !document.querySelector('#task-panel .focus-task')?.disabled);
      const restored = titles();
      const history = document.querySelector('#task-panel .focus-interval-list');
      const overflow = document.documentElement.scrollWidth > document.documentElement.clientWidth;
      return { initial, noNext, badge, groupCount, collapsedBadge, collapsedRows, expanded, focused, newBadge, filesStayedOpen, restored, history: Boolean(history), overflow };
    })()`);
    assert.deepEqual(result.initial, ['Parent', 'Branch', 'Focused']);
    assert.equal(result.noNext, true);
    assert.equal(result.badge, '1 focused');
    assert.equal(result.collapsedBadge, '1 focused');
    assert.equal(result.collapsedRows, 0);
    assert.equal(result.groupCount, 3);
    assert.deepEqual(result.expanded, ['Parent', 'Branch', 'Focused', 'Quiet', 'Quiet child', 'Loose', 'Other file']);
    assert.ok(result.focused.includes('Quiet child'));
    assert.equal(result.newBadge, '2 focused');
    assert.equal(result.filesStayedOpen, 'true');
    assert.deepEqual(result.restored, result.expanded);
    assert.equal(result.history, true);
    assert.equal(result.overflow, false);
  }
  console.log('task focus groups browser E2E passed (desktop and mobile)');
} finally {
  socket.close();
}
