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

try {
  await command('Page.navigate', {
    url: process.env.PLUMB_E2E_URL || 'http://127.0.0.1:38926/tasks',
  });

  const result = await evaluate(`(async () => {
    const waitFor = async (test) => {
      for (let attempt = 0; attempt < 100; attempt += 1) {
        const value = test();
        if (value) return value;
        await new Promise((resolve) => setTimeout(resolve, 50));
      }
      throw new Error('timed out waiting for browser state');
    };
    const listItems = () => Array.from(document.querySelectorAll('.task-list-item'));
    const keyOf = (item) => item.querySelector('.task-identity small').textContent;
    const itemByKey = (key) => listItems().find((item) => keyOf(item) === key);
    const disclosureItem = () => listItems().find((item) => item.querySelector('.task-disclosure:not(.task-disclosure-empty)'));
    const groupToggle = (path) => Array.from(document.querySelectorAll('.task-document-toggle'))
      .find((toggle) => toggle.querySelector('.task-document-path').textContent === path);
    const groupItems = (path) => {
      const children = Array.from(document.querySelector('#task-list').children);
      const start = children.indexOf(groupToggle(path)) + 1;
      const end = children.findIndex((child, index) => index >= start && child.classList.contains('task-document-toggle'));
      return children.slice(start, end === -1 ? undefined : end).filter((child) => child.classList.contains('task-list-item'));
    };
    // Clicking re-renders the list, so every DOM reference is re-queried by key.
    const foldTask = (key) => itemByKey(key).querySelector('.task-disclosure').click();
    const foldGroup = (path) => groupToggle(path).click();

    await waitFor(() => document.querySelector('.task-row'));
    const groups = Array.from(document.querySelectorAll('.task-document-toggle')).length;
    const parent = await waitFor(disclosureItem);
    const parentKey = keyOf(parent);
    const expandedBefore = listItems().length;

    foldTask(parentKey);
    const folded = await waitFor(() => itemByKey(parentKey)?.classList.contains('collapsed') && itemByKey(parentKey));
    const hiddenCount = Number(folded.querySelector('.task-hidden-count').textContent.match(/\\d+/)[0]);
    const expandedWhileFolded = listItems().length;
    const disclosureState = folded.querySelector('.task-disclosure').getAttribute('aria-expanded');
    const disclosureLabel = folded.querySelector('.task-disclosure').getAttribute('aria-label');

    foldTask(parentKey);
    await waitFor(() => listItems().length === expandedBefore);
    const restored = listItems().length;

    const path = document.querySelector('.task-document-path').textContent;
    const groupTotal = groupItems(path).length;
    foldGroup(path);
    await waitFor(() => groupItems(path).length === 0);
    const foldedGroupLabel = groupToggle(path).querySelector('.task-document-count').textContent;
    const foldedGroupState = groupToggle(path).getAttribute('aria-expanded');
    const foldedGroupCount = groupItems(path).length;
    foldGroup(path);
    await waitFor(() => groupItems(path).length === groupTotal);
    const unfoldedGroupState = groupToggle(path).getAttribute('aria-expanded');

    // Selecting a hidden subtask must unfold the ancestors that hide it.
    itemByKey(parentKey).querySelector('.task-row').click();
    await waitFor(() => itemByKey(parentKey)?.classList.contains('selected') && document.querySelector('.task-children button'));
    foldTask(parentKey);
    await waitFor(() => itemByKey(parentKey)?.classList.contains('collapsed'));
    const visibleBeforeReveal = listItems().length;
    const detailVisible = Boolean(document.querySelector('.task-children button'));
    document.querySelector('.task-children button').click();
    await waitFor(() => !document.querySelector('.task-list-item.collapsed'));
    const selectedKey = keyOf(document.querySelector('.task-list-item.selected'));
    const visibleAfterReveal = listItems().length;

    return {
      groups,
      parentKey,
      expandedBefore,
      expandedWhileFolded,
      hiddenCount,
      disclosureState,
      disclosureLabel,
      restored,
      groupTotal,
      foldedGroupLabel,
      foldedGroupState,
      foldedGroupCount,
      unfoldedGroupState,
      visibleBeforeReveal,
      detailVisible,
      visibleAfterReveal,
      selectedKey,
      overflow: document.documentElement.scrollWidth > document.documentElement.clientWidth,
    };
  })()`);

  assert.ok(result.groups >= 1, 'task list renders document groups');
  assert.ok(result.expandedWhileFolded < result.expandedBefore, 'folding a task hides its subtree rows');
  assert.ok(result.hiddenCount > 0, 'folded task reports hidden subtasks');
  assert.equal(result.disclosureState, 'false');
  assert.match(result.disclosureLabel, /Expand .* hidden subtask/);
  assert.equal(result.restored, result.expandedBefore);
  assert.ok(result.groupTotal > 0, 'document group renders tasks');
  assert.match(result.foldedGroupLabel, /hidden/);
  assert.equal(result.foldedGroupState, 'false');
  assert.equal(result.foldedGroupCount, 0, 'folding a document hides all of its rows');
  assert.equal(result.unfoldedGroupState, 'true');
  assert.equal(result.detailVisible, true, 'folding the selected subtree keeps its detail panel');
  assert.ok(result.visibleAfterReveal > result.visibleBeforeReveal, 'selecting a hidden subtask unfolds its ancestors');
  assert.notEqual(result.selectedKey, result.parentKey);
  assert.equal(result.overflow, false);

  socket.close();
  console.log('task folding browser E2E passed');
} catch (error) {
  socket.close();
  throw error;
}
