import assert from 'node:assert/strict';
import test from 'node:test';
import { captureTaskContext, reconcileTaskContext, queryTaskContext, taskQueryScope } from './task-context.js';
import { taskListItems } from './task-tree.js';

const task = (key, extra = {}) => ({ key, path: 'a.plumb', documentId: 'a', locator: { kind: 'id', id: key }, revision: '1', ...extra });
const folds = () => ({ documents: new Set(), tasks: new Set(), expandedGroups: new Set() });
const capture = (tasks, selected, collapsed = folds()) => captureTaskContext(tasks, selected, collapsed);

test('recur insertion does not replace a closed instance that exits the filter', () => {
  const old = [task('a'), task('recur'), task('c')];
  const next = [task('new-instance', { prev: 'recur' }), old[0], old[2]];
  assert.equal(reconcileTaskContext(capture(old, 'recur'), next).selected, 'c');
  assert.equal(reconcileTaskContext(capture(old, 'recur'), [...next, old[1]]).selected, 'recur');
});

test('removal falls back through successor, predecessor, ancestor, then no selection', () => {
  const old = [task('parent'), task('a', { parentKey: 'parent' }), task('b', { parentKey: 'parent' }), task('c')];
  for (const [keys, expected] of [[['parent', 'a', 'c'], 'c'], [['parent', 'a'], 'a'], [['parent'], 'parent'], [[], null]]) {
    assert.equal(reconcileTaskContext(capture(old, 'b'), old.filter((t) => keys.includes(t.key))).selected, expected);
  }
});

test('stable identity survives reorder and move, revealing the new ancestor path', () => {
  const old = [task('a'), task('parent')];
  const collapsed = folds(); collapsed.tasks.add('parent');
  const next = [old[1], task('a', { parentKey: 'parent', revision: '2' })];
  const mapped = reconcileTaskContext(capture(old, 'a', collapsed), next);
  assert.equal(mapped.selected, 'a');
  assert.equal(mapped.collapsed.tasks.has('parent'), false);
});

test('new unfocused groups retain the visible frontier but not hidden descendants', () => {
  const old = [task('a'), task('b'), task('child', { parentKey: 'b' })];
  const collapsed = folds(); collapsed.tasks.add('b');
  const next = [task('a', { focused: true }), old[1], old[2]];
  const mapped = reconcileTaskContext(capture(old, 'b', collapsed), next);
  assert.deepEqual(taskListItems(next, mapped.collapsed).filter((i) => i.kind === 'task').map((i) => i.task.key), ['a', 'b']);
  assert.equal(mapped.collapsed.tasks.has('b'), true);
});

test('ordinary refresh respects explicit collapse even when selection is hidden', () => {
  const tasks = [task('a')]; const collapsed = folds(); collapsed.documents.add('a.plumb');
  const mapped = reconcileTaskContext(capture(tasks, 'a', collapsed), tasks);
  assert.equal(mapped.collapsed.documents.has('a.plumb'), true);
});

test('offset identity cannot select or collapse a different node after source changes', () => {
  const old = [task('offset', { locator: { kind: 'offset', offset: 0 } }), task('stable')];
  const collapsed = folds(); collapsed.tasks.add('offset');
  const mapped = reconcileTaskContext(capture(old, 'offset', collapsed), [task('offset', { locator: { kind: 'offset', offset: 0 }, revision: '2' }), old[1]]);
  assert.equal(mapped.selected, 'stable');
  assert.equal(mapped.collapsed.tasks.has('offset'), false);
});

const request = { query: '', presets: ['ready'], filters: [], sort: ['priority'], limit: 100 };
const snapshot = { revision: 1, tasks: Array.from({ length: 250 }, (_, i) => task(String(i))), nextCursor: 'old' };
test('refresh retains loaded extent and document identities in one query', async () => {
  await queryTaskContext(request, snapshot, false, async (query) => {
    assert.equal(query.limit, 250); assert.equal(query.cursor, null);
    assert.deepEqual(query.retainedDocuments, ['a']); return {};
  });
  assert.equal(taskQueryScope(request), taskQueryScope({ ...request, sort: ['due'] }));
  assert.notEqual(taskQueryScope(request), taskQueryScope({ ...request, query: 'new' }));
});

test('stale cursor rebuilds retained prefix plus one page without mixed revisions', async () => {
  for (const fail of [true, false]) {
    const calls = [];
    const result = await queryTaskContext(request, snapshot, true, async (query) => {
      calls.push(query);
      if (calls.length === 1 && fail) throw Object.assign(new Error('stale'), { source: 'cursor' });
      return { tasks: { revision: 2, tasks: [task('fresh')] } };
    });
    assert.equal(calls.length, 2); assert.equal(calls[1].limit, 350);
    assert.equal(result.tasks.tasks.length, 1);
  }
});

test('valid continuation appends once and non-cursor errors remain errors', async () => {
  const result = await queryTaskContext(request, snapshot, true, async () => ({ tasks: { revision: 1, tasks: [task('next')] } }));
  assert.equal(result.tasks.tasks.length, 251);
  await assert.rejects(queryTaskContext(request, snapshot, true, async () => { throw new Error('offline'); }), /offline/);
});

test('filtering stable nodes out and back in preserves manual subtree folding', () => {
  const tasks = [task('parent'), task('child', { parentKey: 'parent' })];
  const collapsed = folds(); collapsed.tasks.add('parent');
  const empty = reconcileTaskContext(capture(tasks, null, collapsed), []);
  const restored = reconcileTaskContext(capture([], null, empty.collapsed), tasks);
  assert.equal(restored.collapsed.tasks.has('parent'), true);
  assert.deepEqual(taskListItems(tasks, restored.collapsed).filter((item) => item.kind === 'task').map((item) => item.task.key), ['parent']);
});

test('continuation uses the limit bound into the refreshed cursor signature', async () => {
  const refreshed = await queryTaskContext(request, snapshot, false, async (query) => ({
    tasks: { ...snapshot, nextCursor: String(query.limit) },
  }));
  let calls = 0;
  const next = await queryTaskContext(request, refreshed.tasks, true, async (query) => {
    calls++;
    assert.equal(String(query.limit), query.cursor);
    return { tasks: { revision: 1, tasks: [task('next')] } };
  });
  assert.equal(calls, 1);
  assert.equal(next.tasks.tasks.length, 251);
});
