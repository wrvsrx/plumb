import assert from 'node:assert/strict';
import test from 'node:test';

import {
  addPreset,
  addPresetGroup,
  addSortKey,
  initialPresets,
  normalizeSortKeys,
  moveSortKey,
  readQueryParameters,
  taskByKey,
  togglePresetValue,
  viewFromPath,
  writeQueryParameters,
} from './query-state.js';

test('view paths cover routed entry points', () => {
  assert.equal(viewFromPath('/tasks'), 'tasks');
  assert.equal(viewFromPath('/site/tasks/'), 'tasks');
  assert.equal(viewFromPath('/agenda'), 'agenda');
  assert.equal(viewFromPath('/graph'), 'graph');
});

test('task sort keys add and reorder without duplication', () => {
  assert.deepEqual(addSortKey(['priority'], 'due'), ['priority', 'due']);
  assert.deepEqual(addSortKey(['priority', 'due'], 'priority'), ['priority', 'due']);
  assert.deepEqual(moveSortKey(['priority', 'due', 'relevance'], 'relevance', 'priority'), ['relevance', 'priority', 'due']);
  assert.deepEqual(moveSortKey(['priority'], 'missing', 'priority'), ['priority']);
});

test('query parameters round trip repeated filters and traversal state', () => {
  const original = {
    presets: ['ready', 'done'],
    presetsSpecified: true,
    query: 'release notes',
    filters: ['due < now', 'actionable'],
    sort: ['relevance'],
    sortsSpecified: true,
    selected: 'task-key',
    current: 'document-id',
    depth: '3',
    direction: 'outgoing',
    kinds: ['link', 'task-depends'],
  };
  assert.deepEqual(readQueryParameters(writeQueryParameters(original)), original);
});

test('ordered task sorts distinguish defaults, explicit empty, and duplicates', () => {
  const query = readQueryParameters('', 'tasks');
  assert.deepEqual(query.sort, ['priority']);
  assert.equal(query.sortsSpecified, false);
  assert.deepEqual(readQueryParameters('?sort=', 'tasks').sort, []);
  assert.deepEqual(readQueryParameters('?sort=due&sort=priority&sort=due', 'tasks').sort, ['due', 'priority']);
  assert.deepEqual(normalizeSortKeys(['relevance', 'bogus', 'relevance', 'due']), ['relevance', 'due']);
  const empty = writeQueryParameters({ ...query, sort: [], sortsSpecified: true });
  assert.equal(empty.toString(), 'sort=');
  assert.deepEqual(writeQueryParameters({ ...query, sort: ['due', 'priority'] }).getAll('sort'), ['due', 'priority']);
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
