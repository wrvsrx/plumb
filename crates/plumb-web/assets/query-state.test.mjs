import assert from 'node:assert/strict';
import test from 'node:test';

import {
  addPreset,
  addPresetGroup,
  initialPresets,
  readQueryParameters,
  readyTaskQueryRequest,
  readyTasksFromSnapshot,
  sortTaskTrees,
  taskByKey,
  togglePresetValue,
  viewFromPath,
  writeQueryParameters,
} from './query-state.js';

test('view paths cover dynamic and static entry points', () => {
  assert.equal(viewFromPath('/tasks'), 'tasks');
  assert.equal(viewFromPath('/site/tasks/'), 'tasks');
  assert.equal(viewFromPath('/site/tasks/index.html'), 'tasks');
  assert.equal(viewFromPath('/agenda'), 'agenda');
  assert.equal(viewFromPath('/graph'), 'graph');
});

test('agenda ready tasks ignore task-page query state', () => {
  assert.deepEqual(readyTaskQueryRequest(), {
    view: 'tasks',
    query: '',
    presets: ['ready'],
    filters: [],
    sort: 'source',
    limit: null,
    traversal: {},
  });
  assert.deepEqual(readyTasksFromSnapshot({
    tasks: [
      { key: 'ready', state: 'ready' },
      { key: 'done', state: 'done' },
    ],
    complete: true,
  }), {
    tasks: [{ key: 'ready', state: 'ready' }],
    complete: true,
  });
});

test('query parameters round trip repeated filters and traversal state', () => {
  const original = {
    presets: ['ready', 'done'],
    presetsSpecified: true,
    query: 'release notes',
    filters: ['due < now', 'actionable'],
    sort: 'relevance',
    selected: 'task-key',
    current: 'document-id',
    depth: '3',
    direction: 'outgoing',
    kinds: ['link', 'task-depends'],
  };
  assert.deepEqual(readQueryParameters(writeQueryParameters(original)), original);
});

test('priority sort round trips through task URLs', () => {
  const query = readQueryParameters('', 'tasks');
  assert.equal(query.sort, 'priority');
  assert.equal(writeQueryParameters(query).get('sort'), 'priority');
});

test('query parameters distinguish default presets from an explicit empty selection', () => {
  const defaults = readQueryParameters('');
  assert.deepEqual(defaults.presets, []);
  assert.equal(defaults.presetsSpecified, false);
  assert.deepEqual(initialPresets('tasks', defaults), ['ready']);
  assert.deepEqual(initialPresets('graph', defaults), []);

  const explicitEmpty = readQueryParameters(writeQueryParameters({
    ...defaults,
    presetsSpecified: true,
  }));
  assert.deepEqual(explicitEmpty.presets, []);
  assert.equal(explicitEmpty.presetsSpecified, true);
  assert.deepEqual(initialPresets('tasks', explicitEmpty), []);
});

test('grouped and ungrouped preset selections accumulate without duplicates', () => {
  const registry = [
    { id: 'ready', group: 'state' },
    { id: 'waiting', group: 'state' },
    { id: 'connected', group: 'connection' },
    { id: 'has-tasks', group: null },
  ];
  assert.deepEqual(addPreset(['ready', 'connected'], registry[1]), ['ready', 'connected', 'waiting']);
  assert.deepEqual(addPreset(['waiting'], registry[3]), ['waiting', 'has-tasks']);
  assert.deepEqual(addPreset(['waiting'], registry[1]), ['waiting']);
  assert.deepEqual(addPresetGroup([], 'state', registry), ['ready']);
  assert.deepEqual(addPresetGroup(['waiting'], 'state', registry), ['waiting']);
  assert.deepEqual(togglePresetValue(['ready'], registry[1], registry), ['ready', 'waiting']);
  assert.deepEqual(togglePresetValue(['ready', 'waiting'], registry[0], registry), ['waiting']);
  assert.deepEqual(togglePresetValue(['waiting'], registry[1], registry), ['waiting']);
});

test('selected task details survive when a query no longer retains the task', () => {
  const completed = { key: 'notes.plumb:ship', state: 'done' };
  const snapshot = {
    tasks: [{ key: 'notes.plumb:next', state: 'ready' }],
    allTasks: [completed, { key: 'notes.plumb:next', state: 'ready' }],
  };

  assert.equal(taskByKey(snapshot, completed.key), completed);
  assert.equal(taskByKey(snapshot, 'missing'), null);
});

test('task sorting aggregates descendants and keeps document trees contiguous', () => {
  const task = (key, path, start, depth, priority = null, due = null) => ({
    key, path, depth, priority, due, location: { start },
  });
  const tasks = [
    task('a-deferred', 'a.plumb', 0, 0, -5, '2099-03-01T00:00:00Z'),
    task('a-promoted', 'a.plumb', 10, 0, -10),
    task('a-urgent', 'a.plumb', 20, 1, 30),
    task('a-early', 'a.plumb', 30, 0, null, '2099-01-01T00:15:00-01:00'),
    task('b-high', 'b.plumb', 0, 0, 20, '2099-01-01T01:00:00+02:00'),
    task('b-other', 'b.plumb', 10, 0, 15),
    task('c-negative', 'c.plumb', 0, 0, -1),
    task('d-default', 'd.plumb', 0, 0),
  ];
  assert.deepEqual(
    sortTaskTrees(tasks, 'priority').map((item) => item.key),
    ['a-promoted', 'a-urgent', 'a-early', 'a-deferred', 'b-high', 'b-other', 'c-negative', 'd-default'],
  );
  assert.deepEqual(
    sortTaskTrees(tasks, 'due').map((item) => item.key),
    ['b-high', 'b-other', 'a-early', 'a-deferred', 'a-promoted', 'a-urgent', 'c-negative', 'd-default'],
  );
});
