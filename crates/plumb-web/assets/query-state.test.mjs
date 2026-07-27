import assert from 'node:assert/strict';
import test from 'node:test';

import {
  addPreset,
  readQueryParameters,
  taskByKey,
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
    presets: ['ready', 'wait-time'],
    query: 'release notes',
    filter: 'due < now',
    sort: 'relevance',
    selected: 'task-key',
    current: 'document-id',
    depth: '3',
    direction: 'outgoing',
    kinds: ['link', 'task-depends'],
  };
  assert.deepEqual(readQueryParameters(writeQueryParameters(original)), original);
});

test('a grouped preset replaces its peer while ungrouped presets accumulate', () => {
  const registry = [
    { id: 'ready', group: 'state' },
    { id: 'waiting', group: 'state' },
    { id: 'wait-time', group: 'wait' },
    { id: 'has-due', group: null },
  ];
  assert.deepEqual(addPreset(['ready', 'wait-time'], registry[1], registry), ['wait-time', 'waiting']);
  assert.deepEqual(addPreset(['waiting'], registry[3], registry), ['waiting', 'has-due']);
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
