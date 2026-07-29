import assert from 'node:assert/strict';
import test from 'node:test';

import {
  addPreset,
  addPresetGroup,
  initialPresets,
  readQueryParameters,
  taskByKey,
  togglePresetValue,
  viewFromPath,
  writeQueryParameters,
} from './query-state.js';

test('view paths cover dynamic and static entry points', () => {
  assert.equal(viewFromPath('/tasks'), 'tasks');
  assert.equal(viewFromPath('/site/tasks/'), 'tasks');
  assert.equal(viewFromPath('/site/tasks/index.html'), 'tasks');
  assert.equal(viewFromPath('/graph'), 'graph');
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
